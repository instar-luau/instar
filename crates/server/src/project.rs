use std::{
    collections::{BTreeMap, BTreeSet, HashMap, VecDeque},
    fs,
    path::{Path, PathBuf},
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

use instar_bridge::Frontend;

use instar_core::{
    analysis::Edit as SourceEdit,
    lint,
    workspace::{Access, Purpose, SymbolOutcome, Workspace},
};

use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::Value;

use tower_lsp_server::{
    gen_lsp_types::{
        DocumentChange, Edit, OptionalVersionedTextDocumentIdentifier, SemanticToken,
        TextDocumentEdit, TextDocumentIdentifier, TextEdit, Uri, WorkspaceEdit,
    },
    jsonrpc::{Error, Result},
};

use super::{
    configuration::Configuration,
    document::{self, Document, Encoding},
    error::failure,
};

static GENERATION: AtomicU64 = AtomicU64::new(0);
static REVISION: AtomicU64 = AtomicU64::new(1);

type Tokens = VecDeque<(String, Vec<SemanticToken>)>;

pub(super) struct Project {
    pub(super) workspace: Rc<Workspace>,
    pub(super) documents: BTreeMap<String, Document>,
    pub(super) borrowed: BTreeMap<String, Document>,
    pub(super) configuration: Configuration,
    pub(super) capabilities: Arc<Value>,
    pub(super) encoding: Encoding,
    pub(super) generation: u64,
    pub(super) invalidation: u64,
    pub(super) observed: BTreeMap<String, Arc<str>>,
    pub(super) tokens: BTreeMap<String, Tokens>,
    pub(super) published: BTreeSet<String>,
    pub(super) should_index: bool,
    pub(super) nested: BTreeSet<PathBuf>,
    native_dependencies: BTreeSet<PathBuf>,
    native_reload_dependencies: BTreeSet<PathBuf>,
    revision: u64,
}

impl Project {
    pub(super) fn new(
        root: &Path,
        configuration: Configuration,
        capabilities: Arc<Value>,
        encoding: Encoding,
    ) -> Result<Self> {
        let workspace = Workspace::new(root, &[], &[]).map_err(failure)?;
        let generation = generation()?;

        Ok(Self {
            workspace: Rc::new(workspace),
            documents: BTreeMap::new(),
            borrowed: BTreeMap::new(),
            configuration,
            capabilities,
            encoding,
            generation,
            invalidation: generation,
            observed: BTreeMap::new(),
            tokens: BTreeMap::new(),
            published: BTreeSet::new(),
            should_index: true,
            nested: BTreeSet::new(),
            native_dependencies: BTreeSet::new(),
            native_reload_dependencies: BTreeSet::new(),
            revision: 0,
        })
    }

    pub(super) fn module(&self, uri: &Uri) -> Result<String> {
        let path = document::path(uri)?;

        if path.try_exists().map_err(failure)? {
            self.workspace.module(&path).map_err(failure)
        } else {
            self.workspace.normalize(&path).map_err(failure)
        }
    }

    pub(super) fn snapshot(&mut self, module: &str) -> Result<Document> {
        if let Some(document) = self
            .documents
            .get(module)
            .or_else(|| self.borrowed.get(module))
        {
            return Ok(document.clone());
        }

        let source = self
            .workspace
            .source(module)
            .map_err(failure)?
            .ok_or_else(|| {
                Error::invalid_params(format!("Source is unavailable for `{module}`."))
            })?;

        let revision = self.advance()?;
        let document = Document::new(document::uri(Path::new(module))?, source, None, revision);
        self.documents.insert(module.to_owned(), document.clone());

        Ok(document)
    }

    pub(super) fn adopt(&mut self, module: String, document: Document) {
        self.revision = self.revision.max(document.revision);
        self.borrowed.remove(&module);
        self.documents.insert(module, document);
    }

    pub(super) fn is_owner(&self, module: &str) -> bool {
        let path = Path::new(module);

        !self.nested.iter().any(|root| path.starts_with(root))
            && (self.should_index && path.starts_with(self.workspace.directory())
                || self
                    .documents
                    .get(module)
                    .is_some_and(|document| document.version.is_some()))
    }

    pub(super) fn index(&self) -> Result<Vec<String>> {
        let mut modules: BTreeSet<_> =
            if self.should_index && self.workspace.directory().try_exists().map_err(failure)? {
                self.workspace
                    .select(&[], Purpose::Index, Access::Read)
                    .map_err(failure)?
                    .into_keys()
                    .collect()
            } else {
                BTreeSet::new()
            };

        modules.extend(
            self.documents
                .iter()
                .filter(|(_, document)| document.version.is_some())
                .map(|(module, _)| module.clone()),
        );

        Ok(modules
            .into_iter()
            .filter(|module| self.is_owner(module))
            .collect())
    }

    pub(super) fn findings(&self, module: &str, source: &str) -> Result<Vec<lint::Finding>> {
        let settings = self.workspace.linting(module).map_err(failure)?;
        let symbols = self.workspace.symbols(module).map_err(failure)?;

        let symbols = match symbols {
            SymbolOutcome::Ready(symbols) => symbols,
            SymbolOutcome::Invalid(invalid) => return Err(failure(invalid.error)),
        };

        lint::lint(source, &settings, &symbols).map_err(failure)
    }

    pub(super) fn has_capability(&self, pointer: &str) -> bool {
        self.capabilities.pointer(pointer).and_then(Value::as_bool) == Some(true)
    }

    pub(super) fn data<ValueType: Serialize>(
        &mut self,
        module: &str,
        value: ValueType,
    ) -> Result<Value> {
        let document = self.snapshot(module)?;

        serde_json::to_value(Resolve {
            module: module.to_owned(),
            revision: document.revision,
            generation: self.generation,
            value: serde_json::to_value(value).map_err(failure)?,
        })
        .map_err(failure)
    }

    pub(super) fn resolve<ValueType: DeserializeOwned>(
        &self,
        data: Option<Value>,
    ) -> Result<(String, ValueType)> {
        let resolve: Resolve = serde_json::from_value(
            data.ok_or_else(|| Error::invalid_params("Resolve data is missing."))?,
        )
        .map_err(|error| Error::invalid_params(error.to_string()))?;

        let document = self
            .documents
            .get(&resolve.module)
            .or_else(|| self.borrowed.get(&resolve.module))
            .ok_or_else(Error::content_modified)?;

        if resolve.generation != self.generation || resolve.revision != document.revision {
            return Err(Error::content_modified());
        }

        Ok((
            resolve.module,
            serde_json::from_value(resolve.value)
                .map_err(|error| Error::invalid_params(error.to_string()))?,
        ))
    }

    pub(super) fn edits(
        &mut self,
        changes: BTreeMap<String, Vec<SourceEdit>>,
    ) -> Result<WorkspaceEdit> {
        let mut documents = Vec::new();
        let mut unversioned = HashMap::new();

        for (module, mut edits) in changes {
            let document = self.snapshot(&module)?;

            if document.version.is_none()
                && fs::read_to_string(&module).map_err(failure)?.as_str() != document.text.as_ref()
            {
                return Err(Error::content_modified());
            }

            edits.sort_by_key(|edit| (edit.range.start, edit.range.end));

            if edits.windows(2).any(|pair| {
                pair[0].range.end > pair[1].range.start
                    || pair[0].range.is_empty() && pair[0].range.start == pair[1].range.start
            }) {
                return Err(Error::invalid_params("Workspace edits overlap."));
            }

            let edits: Vec<_> = edits
                .into_iter()
                .map(|edit| {
                    Ok(TextEdit {
                        range: document.range(edit.range, self.encoding)?,
                        new_text: edit.replacement,
                    })
                })
                .collect::<Result<_>>()?;

            if self.has_capability("/workspace/workspaceEdit/documentChanges") {
                documents.push(DocumentChange::TextDocumentEdit(TextDocumentEdit {
                    text_document: OptionalVersionedTextDocumentIdentifier {
                        version: document.version,
                        text_document_identifier: TextDocumentIdentifier { uri: document.uri },
                    },
                    edits: edits.into_iter().map(Edit::TextEdit).collect(),
                }));
            } else {
                unversioned.insert(document.uri, edits);
            }
        }

        Ok(WorkspaceEdit {
            changes: (!unversioned.is_empty()).then_some(unversioned),
            document_changes: (!documents.is_empty()).then_some(documents),
            change_annotations: None,
        })
    }

    pub(super) fn advance(&mut self) -> Result<u64> {
        self.revision = REVISION
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |revision| {
                revision.checked_add(1)
            })
            .map_err(|_revision| Error::internal_error())?;

        Ok(self.revision)
    }

    pub(super) fn changed(&mut self) -> Result<()> {
        self.generation = generation()?;

        Ok(())
    }

    pub(super) fn invalidate(&mut self, module: &str) {
        self.workspace.invalidate(module);
        self.observed.remove(module);

        if self
            .documents
            .get(module)
            .is_some_and(|document| document.version.is_none())
        {
            self.documents.remove(module);
        }
    }

    pub(super) fn reload(&mut self) -> Result<()> {
        Rc::get_mut(&mut self.workspace)
            .ok_or_else(|| failure("Cannot reload a borrowed workspace."))?
            .reload()
            .map_err(failure)?;

        self.documents
            .retain(|_, document| document.version.is_some());

        self.tokens.clear();

        self.changed()?;
        self.invalidation = self.generation;
        self.observed.clear();
        self.native_dependencies.clear();
        self.native_reload_dependencies.clear();

        Ok(())
    }

    pub(super) fn dependencies(&self) -> BTreeSet<PathBuf> {
        let mut dependencies = self.workspace.dependencies();
        dependencies.extend(self.native_dependencies.iter().cloned());

        dependencies
    }

    pub(super) fn reload_dependencies(&self) -> BTreeSet<PathBuf> {
        let mut dependencies = self.workspace.reload_dependencies();
        dependencies.extend(self.native_reload_dependencies.iter().cloned());

        dependencies
    }

    pub(super) fn capture(&self) -> Snapshot {
        Snapshot {
            root: self.workspace.directory().to_owned(),
            documents: self
                .documents
                .iter()
                .filter(|(_, document)| document.version.is_some())
                .map(|(module, document)| (module.clone(), document.clone()))
                .collect(),
            borrowed: self.borrowed.clone(),
            configuration: self.configuration.clone(),
            capabilities: Arc::clone(&self.capabilities),
            encoding: self.encoding,
            generation: self.generation,
            invalidation: self.invalidation,
            published: self.published.clone(),
            should_index: self.should_index,
            nested: self.nested.clone(),
        }
    }

    pub(super) fn apply(&mut self, snapshot: Snapshot, frontend: &mut Frontend<'_>) -> Result<()> {
        let previous: BTreeMap<_, _> = self
            .documents
            .iter()
            .filter(|(_, document)| document.version.is_some())
            .chain(self.borrowed.iter())
            .map(|(module, document)| (module.clone(), Arc::clone(&document.text)))
            .collect();

        let sources: BTreeMap<_, _> = snapshot
            .documents
            .iter()
            .chain(snapshot.borrowed.iter())
            .map(|(module, document)| (module.clone(), Arc::clone(&document.text)))
            .collect();

        let should_refresh = previous.keys().ne(sources.keys());

        for module in previous
            .keys()
            .filter(|module| !sources.contains_key(*module))
        {
            self.workspace.remove_source(module);
            self.documents.remove(module);
            frontend.invalidate(module).map_err(failure)?;
        }

        for (module, source) in &sources {
            if previous.get(module) == Some(source) {
                continue;
            }

            let has_changed = self.workspace.snapshot(module).as_ref() != Some(source);

            let identity = self
                .workspace
                .set_source(Path::new(module), Arc::clone(source))
                .map_err(failure)?;

            if identity != *module {
                return Err(Error::content_modified());
            }

            self.documents.remove(module);

            if has_changed {
                frontend.invalidate(module).map_err(failure)?;
            }
        }

        self.documents.retain(|module, document| {
            document.version.is_none() && !snapshot.borrowed.contains_key(module)
        });

        self.documents.extend(snapshot.documents);
        self.borrowed = snapshot.borrowed;
        self.configuration = snapshot.configuration;
        self.capabilities = snapshot.capabilities;
        self.encoding = snapshot.encoding;

        self.published = snapshot.published;

        self.generation = snapshot.generation;
        self.invalidation = snapshot.invalidation;
        self.should_index = snapshot.should_index;
        self.nested = snapshot.nested;

        if should_refresh {
            frontend.refresh().map_err(failure)?;
        }

        Ok(())
    }

    pub(super) fn feedback(&self) -> Feedback {
        let dependencies = self.workspace.dependencies();

        let observed = dependencies
            .iter()
            .filter_map(|path| {
                let module = path.to_str()?;

                Some((module.to_owned(), self.workspace.snapshot(module)?))
            })
            .collect();

        Feedback {
            dependencies,
            reload_dependencies: self.workspace.reload_dependencies(),
            observed,
            published: self.published.clone(),
        }
    }

    pub(super) fn accept(&mut self, feedback: Feedback, should_publish: bool) {
        self.native_dependencies.extend(feedback.dependencies);

        self.native_reload_dependencies
            .extend(feedback.reload_dependencies);

        self.observed.extend(feedback.observed);

        if should_publish {
            self.published = feedback.published;
        }
    }
}

pub(super) struct Snapshot {
    pub(super) root: PathBuf,
    pub(super) documents: BTreeMap<String, Document>,
    pub(super) borrowed: BTreeMap<String, Document>,
    pub(super) configuration: Configuration,
    pub(super) capabilities: Arc<Value>,
    pub(super) encoding: Encoding,
    pub(super) generation: u64,
    pub(super) invalidation: u64,
    pub(super) published: BTreeSet<String>,
    pub(super) should_index: bool,
    pub(super) nested: BTreeSet<PathBuf>,
}

pub(super) struct Feedback {
    pub(super) dependencies: BTreeSet<PathBuf>,
    pub(super) reload_dependencies: BTreeSet<PathBuf>,
    pub(super) observed: BTreeMap<String, Arc<str>>,
    pub(super) published: BTreeSet<String>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Resolve {
    module: String,
    revision: u64,
    generation: u64,
    value: Value,
}

pub(super) fn resolved_uri(data: Option<&Value>) -> Result<Uri> {
    let resolve: Resolve = serde_json::from_value(
        data.cloned()
            .ok_or_else(|| Error::invalid_params("Resolve data is missing."))?,
    )
    .map_err(|error| Error::invalid_params(error.to_string()))?;

    document::uri(Path::new(&resolve.module))
}

fn generation() -> Result<u64> {
    GENERATION
        .try_update(Ordering::Relaxed, Ordering::Relaxed, |generation| {
            generation.checked_add(1)
        })
        .map_err(|_generation| Error::internal_error())
}
