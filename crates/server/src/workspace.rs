use std::{
    collections::{BTreeMap, BTreeSet},
    env, io,
    path::PathBuf,
    sync::Arc,
};

use instar_core::{project::Change, snapshot::Snapshot};
use tempfile::TempDir;

use tower_lsp_server::{
    gen_lsp_types::{
        ClientCapabilities, DiagnosticClientCapabilities, DidChangeTextDocumentParams,
        DidCloseTextDocumentParams, DidOpenTextDocumentParams, DidSaveTextDocumentParams,
        DocumentSymbolClientCapabilities, FoldingRangeClientCapabilities, MarkupKind,
        PositionEncodingKind, SignatureHelpClientCapabilities, TextDocumentContentChangeEvent, Uri,
        WorkspaceSymbolClientCapabilities,
    },
    jsonrpc::{Error, Result},
};

use crate::{
    completion::Completions, hierarchy::Hierarchies, hints::Hints, position, tokens::Tokens,
};

struct Document {
    version: i32,
    synchronized: bool,
    uri: Uri,
    directory: Option<TempDir>,
}

struct Definition {
    source: Uri,
    text: Arc<str>,
}

pub(crate) struct Workspace {
    pub(crate) snapshot: Snapshot,
    pub(crate) roots: BTreeSet<PathBuf>,
    pub(crate) encoding: PositionEncodingKind,
    pub(crate) hover_format: MarkupKind,
    pub(crate) symbol_capabilities: DocumentSymbolClientCapabilities,
    pub(crate) search_capabilities: WorkspaceSymbolClientCapabilities,
    pub(crate) completions: Completions,
    pub(crate) signature_capabilities: SignatureHelpClientCapabilities,
    pub(crate) versioned_edits: bool,
    pub(crate) tokens: Tokens,
    pub(crate) hints: Hints,
    pub(crate) hierarchies: Hierarchies,
    pub(crate) fold_capabilities: FoldingRangeClientCapabilities,
    pub(crate) link_tooltips: bool,
    pub(crate) diagnostic_capabilities: Option<DiagnosticClientCapabilities>,
    pub(crate) diagnostic_refresh: bool,
    documents: BTreeMap<PathBuf, Document>,
    definitions: BTreeMap<Uri, Definition>,
}

impl Default for Workspace {
    fn default() -> Self {
        Self {
            snapshot: Snapshot::default(),
            roots: BTreeSet::new(),
            encoding: PositionEncodingKind::UTF16,
            hover_format: MarkupKind::PlainText,
            symbol_capabilities: DocumentSymbolClientCapabilities::default(),
            search_capabilities: WorkspaceSymbolClientCapabilities::default(),
            completions: Completions::default(),
            signature_capabilities: SignatureHelpClientCapabilities::default(),
            versioned_edits: false,
            tokens: Tokens::default(),
            hints: Hints::default(),
            hierarchies: Hierarchies::default(),
            fold_capabilities: FoldingRangeClientCapabilities::default(),
            link_tooltips: false,
            diagnostic_capabilities: None,
            diagnostic_refresh: false,
            documents: BTreeMap::new(),
            definitions: BTreeMap::new(),
        }
    }
}

impl Workspace {
    pub(crate) fn configure(&mut self, capabilities: &ClientCapabilities) {
        if let Some(document) = &capabilities.text_document {
            self.diagnostic_capabilities
                .clone_from(&document.diagnostic);

            if let Some(format) = document
                .hover
                .as_ref()
                .and_then(|hover| hover.content_format.as_ref())
                .and_then(|formats| {
                    formats.iter().find(|format| {
                        matches!(format, MarkupKind::PlainText | MarkupKind::Markdown)
                    })
                })
            {
                self.hover_format = format.clone();
            }

            if let Some(symbols) = &document.document_symbol {
                self.symbol_capabilities = symbols.clone();
            }

            if let Some(completion) = &document.completion {
                self.completions.capabilities = completion.clone();
            }

            if let Some(signatures) = &document.signature_help {
                self.signature_capabilities = signatures.clone();
            }

            if let Some(folding) = &document.folding_range {
                self.fold_capabilities.clone_from(folding);
            }

            self.link_tooltips = document
                .document_link
                .as_ref()
                .and_then(|links| links.tooltip_support)
                .unwrap_or(false);

            self.tokens
                .capabilities
                .clone_from(&document.semantic_tokens);

            self.hints.deferred_tooltips = document
                .inlay_hint
                .as_ref()
                .and_then(|capabilities| capabilities.resolve_support.as_ref())
                .is_some_and(|resolve| {
                    resolve
                        .properties
                        .iter()
                        .any(|property| property == "tooltip")
                });
        }

        if let Some(symbols) = capabilities
            .workspace
            .as_ref()
            .and_then(|workspace| workspace.symbol.as_ref())
        {
            self.search_capabilities = symbols.clone();
        }

        self.diagnostic_refresh = capabilities
            .workspace
            .as_ref()
            .and_then(|workspace| workspace.diagnostics.as_ref())
            .and_then(|diagnostics| diagnostics.refresh_support)
            .unwrap_or(false);

        self.versioned_edits = capabilities
            .workspace
            .as_ref()
            .and_then(|workspace| workspace.workspace_edit.as_ref())
            .and_then(|edit| edit.document_changes)
            .unwrap_or(false);
    }

    pub(crate) fn resolve(&self, uri: &Uri) -> Result<PathBuf> {
        self.documents
            .iter()
            .find(|(_, document)| &document.uri == uri)
            .map_or_else(|| path(uri), |(path, _)| Ok(path.clone()))
    }

    pub(crate) fn editable(&self, uri: &Uri) -> Result<()> {
        if uri.scheme() == "instar" {
            return Err(Error::invalid_params("document is read-only"));
        }

        self.resolve(uri).map(|_| ())
    }

    pub(crate) fn content(&self, uri: &Uri) -> Result<String> {
        if let Some(definition) = self.definitions.get(uri) {
            return Ok(definition.text.to_string());
        }

        let path = self.resolve(uri)?;

        self.snapshot
            .source(&path)
            .map(ToString::to_string)
            .ok_or_else(|| Error::invalid_params("document is not open"))
    }

    pub(crate) fn documents(&self) -> BTreeMap<PathBuf, i32> {
        self.documents
            .iter()
            .filter(|(_, document)| document.synchronized)
            .map(|(path, document)| (path.clone(), document.version))
            .collect()
    }

    pub(crate) fn locations(&self) -> BTreeMap<PathBuf, Uri> {
        self.documents
            .iter()
            .filter(|(_, document)| document.synchronized)
            .map(|(path, document)| (path.clone(), document.uri.clone()))
            .collect()
    }

    pub(crate) fn index_roots(&self) -> Vec<PathBuf> {
        self.roots
            .iter()
            .cloned()
            .chain(
                self.documents
                    .iter()
                    .filter(|(_, document)| {
                        document.synchronized && document.uri.scheme() != "instar"
                    })
                    .map(|(path, _)| path.clone()),
            )
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect()
    }

    pub(crate) fn definition(&mut self, source: Uri, text: Arc<str>) -> Result<Uri> {
        if let Some((uri, _)) = self
            .definitions
            .iter()
            .find(|(_, definition)| definition.source == source && definition.text == text)
        {
            return Ok(uri.clone());
        }

        let mut index = self.definitions.len();

        let uri = loop {
            let uri = format!("instar://definition/{index}/{}", source.as_str())
                .parse::<Uri>()
                .map_err(|error| Error::invalid_params(error.to_string()))?;

            if !self.definitions.contains_key(&uri)
                && !self.documents.values().any(|document| document.uri == uri)
            {
                break uri;
            }

            index += 1;
        };

        self.definitions
            .insert(uri.clone(), Definition { source, text });

        Ok(uri)
    }

    pub(crate) fn open(&mut self, parameters: DidOpenTextDocumentParams) -> Result<()> {
        let document = parameters.text_document;

        if let Some(definition) = self.definitions.get(&document.uri)
            && document.text.as_str() != definition.text.as_ref()
        {
            return Err(Error::invalid_params("definition content does not match"));
        }

        if self.documents.values().any(|open| open.uri == document.uri) {
            return Err(Error::invalid_params("document is already open"));
        }

        let directory = match document.uri.scheme() {
            "file" => None,

            "untitled" | "instar" => {
                let root = self
                    .roots
                    .first()
                    .cloned()
                    .map_or_else(env::current_dir, Ok)
                    .map_err(|error| failure(&error))?;

                Some(tempfile::tempdir_in(root).map_err(|error| failure(&error))?)
            }

            _ => return Err(Error::invalid_params("unsupported document URI scheme")),
        };

        let path = match &directory {
            Some(directory) => directory.path().join("document.luau"),
            None => path(&document.uri)?,
        };

        if self.documents.contains_key(&path) {
            return Err(Error::invalid_params("document is already open"));
        }

        self.snapshot
            .change(Change::Overlay {
                path: path.clone(),
                text: Some(document.text),
            })
            .map_err(|error| failure(&error))?;

        self.documents.insert(
            path,
            Document {
                version: document.version,
                synchronized: true,
                uri: document.uri,
                directory,
            },
        );

        Ok(())
    }

    pub(crate) fn change(&mut self, parameters: DidChangeTextDocumentParams) -> Result<()> {
        let uri = &parameters.text_document.text_document_identifier.uri;
        self.editable(uri)?;
        let path = self.resolve(uri)?;

        let document = self
            .documents
            .get_mut(&path)
            .ok_or_else(|| Error::invalid_params("document is not open"))?;

        if parameters.text_document.version <= document.version {
            return Err(Error::invalid_params("document version did not increase"));
        }

        document.version = parameters.text_document.version;

        let mut text = self
            .snapshot
            .source(&path)
            .expect("open document overlay")
            .to_string();

        let result = apply(
            &mut text,
            parameters.content_changes,
            &self.encoding,
            document.synchronized,
        );

        document.synchronized = result.is_ok();

        if let Err(error) = result {
            self.snapshot
                .change(Change::Disk(path))
                .map_err(|error| failure(&error))?;

            return Err(error);
        }

        self.snapshot
            .change(Change::Overlay {
                path,
                text: Some(text),
            })
            .map_err(|error| failure(&error))
    }

    pub(crate) fn close(&mut self, parameters: DidCloseTextDocumentParams) -> Result<()> {
        let document = parameters.text_document;
        let path = self.resolve(&document.uri)?;

        if self.documents.remove(&path).is_none() {
            return Err(Error::invalid_params("document is not open"));
        }

        self.tokens.forget(&document.uri);
        self.hints.forget(&document.uri);

        self.snapshot
            .change(Change::Close(path))
            .map_err(|error| failure(&error))
    }

    pub(crate) fn save(&mut self, parameters: DidSaveTextDocumentParams) -> Result<()> {
        let document = parameters.text_document;
        self.editable(&document.uri)?;
        let path = self.resolve(&document.uri)?;

        if !self.documents.contains_key(&path) {
            return Err(Error::invalid_params("document is not open"));
        }

        if self
            .documents
            .get(&path)
            .is_some_and(|document| document.directory.is_some())
        {
            return Err(Error::invalid_params("virtual documents cannot be saved"));
        }

        self.snapshot
            .change(Change::Disk(path))
            .map_err(|error| failure(&error))
    }

    pub(crate) fn capture(&self, uri: &Uri) -> Result<(Snapshot, PathBuf, PositionEncodingKind)> {
        let path = self.resolve(uri)?;

        if self
            .documents
            .get(&path)
            .is_some_and(|document| !document.synchronized)
        {
            return Err(Error::invalid_params(
                "document requires a full-content change after an invalid edit",
            ));
        }

        Ok((self.snapshot.clone(), path, self.encoding.clone()))
    }

    pub(crate) fn invalidate(&mut self) {
        let paths = self
            .roots
            .iter()
            .cloned()
            .chain(
                self.documents
                    .keys()
                    .filter_map(|path| path.parent().map(ToOwned::to_owned)),
            )
            .collect::<BTreeSet<_>>();

        for path in paths {
            self.snapshot
                .change(Change::Disk(path))
                .expect("absolute workspace input");
        }
    }

    pub(crate) fn current(&self, snapshot: &Snapshot, inputs: &BTreeSet<PathBuf>) -> bool {
        self.snapshot.current(snapshot, inputs)
            && self.documents.iter().all(|(path, document)| {
                document.synchronized
                    || !inputs
                        .iter()
                        .any(|input| path.starts_with(input) || input.starts_with(path))
            })
    }

    pub(crate) fn renamed(&mut self, old: &Uri, new: &Uri) -> Result<()> {
        let old = path(old)?;
        let new = path(new)?;

        let moved = self
            .documents
            .iter()
            .filter(|(path, document)| document.directory.is_none() && path.starts_with(&old))
            .map(|(path, _)| {
                let relative = path.strip_prefix(&old).expect("renamed document prefix");

                let target = if relative.as_os_str().is_empty() {
                    new.clone()
                } else {
                    new.join(relative)
                };

                let uri = Uri::from_file_path(&target)
                    .map_err(|()| Error::invalid_params("invalid rename target"))?;

                if self.documents.contains_key(&target) && !target.starts_with(&old) {
                    return Err(Error::invalid_params("rename target is already open"));
                }

                Ok((path.clone(), target, uri))
            })
            .collect::<Result<Vec<_>>>()?;

        for (source, target, uri) in moved {
            let mut document = self.documents.remove(&source).expect("renamed document");

            let text = self
                .snapshot
                .source(&source)
                .expect("open document overlay")
                .to_string();

            self.snapshot
                .change(Change::Close(source))
                .map_err(|error| failure(&error))?;

            self.snapshot
                .change(Change::Overlay {
                    path: target.clone(),
                    text: Some(text),
                })
                .map_err(|error| failure(&error))?;

            document.uri = uri;
            self.documents.insert(target, document);
        }

        self.snapshot
            .change(Change::Disk(old))
            .map_err(|error| failure(&error))?;

        self.snapshot
            .change(Change::Disk(new))
            .map_err(|error| failure(&error))
    }
}

pub(crate) fn path(uri: &Uri) -> Result<PathBuf> {
    if uri.scheme() != "file" || uri.query().is_some() || uri.fragment().is_some() {
        return Err(Error::invalid_params(
            "only file document URIs are supported",
        ));
    }

    uri.to_file_path()
        .map_err(|()| Error::invalid_params("invalid file URI"))
}

fn apply(
    text: &mut String,
    changes: Vec<TextDocumentContentChangeEvent>,
    encoding: &PositionEncodingKind,
    synchronized: bool,
) -> Result<()> {
    let mut synchronized = synchronized;

    for change in changes {
        match change {
            TextDocumentContentChangeEvent::TextDocumentContentChangeWholeDocument(change) => {
                *text = change.text;
                synchronized = true;
            }

            TextDocumentContentChangeEvent::TextDocumentContentChangePartial(change) => {
                if !synchronized {
                    return Err(Error::invalid_params(
                        "incremental change requires synchronized contents",
                    ));
                }

                position::replace(text, change.range, &change.text, encoding)?;
            }
        }
    }

    if synchronized {
        Ok(())
    } else {
        Err(Error::invalid_params("document requires full contents"))
    }
}

fn failure(error: &io::Error) -> Error {
    Error::invalid_params(error.to_string())
}
