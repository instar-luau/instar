use std::{path::PathBuf, sync::Arc};

use instar_project::{
    ChangeBatch, InputIdentity, InputVersion, Project, ProjectChange, ProjectSnapshot,
    SourceChange, SourceOrigin,
};

use instar_shared::{Options, SourceIdentity, SourceSnapshot, SourceVersion};

use tower_lsp_server::{
    gen_lsp_types::{
        DidChangeTextDocumentParams, DidCloseTextDocumentParams, DidOpenTextDocumentParams,
        DidSaveTextDocumentParams, TextDocumentContentChangeEvent, Uri,
    },
    jsonrpc::{Error, Result},
};

use super::state::State;
use crate::{position, session::failure};

const RETAINED_DECLARATIONS: usize = 256;

pub(crate) struct Declaration {
    pub(crate) source: SourceSnapshot,
    pub(crate) origin: SourceOrigin,
    pub(crate) open: bool,
}

#[derive(Clone)]
pub(crate) struct Document {
    pub(crate) source: SourceIdentity,
    pub(crate) version: i32,
    pub(crate) revision: SourceVersion,
    pub(crate) synchronized: bool,
    pub(crate) writable: bool,
}

impl State {
    pub(crate) fn open(
        &mut self,
        project: &Project,
        parameters: DidOpenTextDocumentParams,
    ) -> Result<()> {
        let document = parameters.text_document;

        if self.documents.contains_key(&document.uri) {
            return Err(Error::invalid_params("document is already open"));
        }

        if let Some(declaration) = self.declarations.get_mut(&document.uri) {
            if declaration.open {
                return Err(Error::invalid_params("document is already open"));
            }

            if declaration.source.text.as_ref() != document.text {
                return Err(Error::invalid_params("definition content does not match"));
            }

            declaration.open = true;
            self.declaration_order.retain(|uri| uri != &document.uri);

            return Ok(());
        }

        if definition(&document.uri) {
            return Err(Error::invalid_params("definition URI has expired"));
        }

        let source = project.source_identity();

        let context = match document.uri.scheme() {
            "file" => Some(path(&document.uri)?),

            "untitled" | "instar" => self
                .workspace_folders
                .first()
                .map(|root| root.join(format!("document-{}.luau", source.get()))),

            _ => return Err(Error::invalid_params("unsupported document URI scheme")),
        };

        let snapshot = project.snapshot();

        if document.uri.scheme() == "instar"
            && snapshot
                .source_for_address(document.uri.as_str())
                .is_some_and(|source| source.text.as_ref() != document.text)
        {
            return Err(Error::invalid_params("definition content does not match"));
        }

        let snapshot = project
            .change(ChangeBatch {
                changes: vec![ProjectChange::Source(SourceChange::Open {
                    identity: source,
                    origin: SourceOrigin::Document {
                        address: document.uri.as_str().to_owned(),
                        context,
                    },
                    text: Arc::from(document.text),
                })],
            })
            .map_err(|error| failure(&error))?;

        let revision = snapshot
            .source(source)
            .ok_or_else(Error::internal_error)?
            .version;

        self.documents.insert(
            document.uri.clone(),
            Document {
                source,
                version: document.version,
                revision,
                synchronized: true,
                writable: document.uri.scheme() != "instar",
            },
        );

        Ok(())
    }

    pub(crate) fn change(
        &mut self,
        project: &Project,
        parameters: DidChangeTextDocumentParams,
    ) -> Result<()> {
        let uri = &parameters.text_document.text_document_identifier.uri;

        if definition(uri) {
            return Err(Error::invalid_params("document is read-only"));
        }

        let document = self
            .documents
            .get_mut(uri)
            .ok_or_else(|| Error::invalid_params("document is not open"))?;

        if !document.writable {
            return Err(Error::invalid_params("document is read-only"));
        }

        if parameters.text_document.version <= document.version {
            document.synchronized = false;

            return Err(Error::invalid_params(
                "document version did not increase; full-content resynchronization required",
            ));
        }

        let snapshot = project.snapshot();

        let source = snapshot
            .source(document.source)
            .ok_or_else(Error::content_modified)?;

        if source.version != document.revision {
            return Err(Error::content_modified());
        }

        let mut text = source.text.to_string();
        let mut synchronized = document.synchronized;

        let result = (|| {
            for change in parameters.content_changes {
                match change {
                    TextDocumentContentChangeEvent::TextDocumentContentChangeWholeDocument(
                        change,
                    ) => {
                        text = change.text;
                        synchronized = true;
                    }

                    TextDocumentContentChangeEvent::TextDocumentContentChangePartial(change) => {
                        if !synchronized {
                            return Err(Error::invalid_params(
                                "incremental change requires full-content resynchronization",
                            ));
                        }

                        position::replace(&mut text, change.range, &change.text, &self.encoding)?;
                    }
                }
            }

            if !synchronized {
                return Err(Error::invalid_params("document requires full contents"));
            }

            project
                .change(ChangeBatch {
                    changes: vec![ProjectChange::Source(SourceChange::Replace {
                        expected: document.revision,
                        text: Arc::from(text),
                    })],
                })
                .map_err(|error| failure(&error))
        })();

        match result {
            Ok(snapshot) => {
                document.revision = snapshot
                    .source(document.source)
                    .ok_or_else(Error::internal_error)?
                    .version;

                document.version = parameters.text_document.version;
                document.synchronized = true;

                Ok(())
            }

            Err(error) => {
                document.synchronized = false;

                Err(error)
            }
        }
    }

    pub(crate) fn close(
        &mut self,
        project: &Project,
        parameters: DidCloseTextDocumentParams,
    ) -> Result<()> {
        let uri = parameters.text_document.uri;

        if let Some(declaration) = self.declarations.get_mut(&uri) {
            if !declaration.open {
                return Err(Error::invalid_params("document is not open"));
            }

            declaration.open = false;
            self.declaration_order.push_back(uri);
            self.trim_declarations();

            return Ok(());
        }

        if definition(&uri) {
            return Err(Error::invalid_params("definition URI has expired"));
        }

        let document = self
            .documents
            .get(&uri)
            .ok_or_else(|| Error::invalid_params("document is not open"))?;

        let revision = document.revision;

        project
            .change(ChangeBatch {
                changes: vec![ProjectChange::Source(SourceChange::Close {
                    expected: revision,
                })],
            })
            .map_err(|error| failure(&error))?;

        self.documents.remove(&uri);
        self.identities.forget(revision);

        if !self.capabilities.pull_diagnostics() {
            self.published.insert(uri);
        }

        Ok(())
    }

    pub(crate) fn save(
        &mut self,
        project: &Project,
        parameters: DidSaveTextDocumentParams,
        options: &Options,
    ) -> Result<()> {
        let uri = parameters.text_document.uri;

        let document = self
            .documents
            .get(&uri)
            .ok_or_else(|| Error::invalid_params("document is not open"))?;

        if !document.writable || uri.scheme() != "file" {
            return Err(Error::invalid_params("document cannot be saved"));
        }

        let (_, source) = self.capture(project, &uri, options)?;

        if parameters
            .text
            .is_some_and(|text| text != source.text.as_ref())
        {
            return Err(Error::invalid_params(
                "saved content differs from synchronized content",
            ));
        }

        Ok(())
    }

    pub(crate) fn renamed(
        &mut self,
        project: &Project,
        movements: &[(PathBuf, PathBuf)],
        mut changes: Vec<ProjectChange>,
    ) -> Result<()> {
        let snapshot = project.snapshot();
        let mut documents = self.documents.clone();
        let mut retired = Vec::new();

        for (old, new) in movements {
            let moved = self
                .documents
                .iter()
                .filter_map(|(uri, document)| {
                    path(uri)
                        .ok()
                        .filter(|source| source.starts_with(old))
                        .map(|source| (uri.clone(), document.clone(), source))
                })
                .collect::<Vec<_>>();

            for (uri, mut document, source) in moved {
                let relative = source
                    .strip_prefix(old)
                    .map_err(|_| Error::internal_error())?;

                let target = if relative.as_os_str().is_empty() {
                    new.clone()
                } else {
                    new.join(relative)
                };

                let target_uri = Uri::from_file_path(&target)
                    .map_err(|()| Error::invalid_params("invalid rename target"))?;

                if documents.contains_key(&target_uri) && target_uri != uri {
                    return Err(Error::invalid_params("rename target is already open"));
                }

                let contents = snapshot
                    .source(document.source)
                    .ok_or_else(Error::content_modified)?
                    .text
                    .clone();

                retired.push(document.revision);

                changes.push(ProjectChange::Source(SourceChange::Remove {
                    expected: document.revision,
                }));

                let identity = project.source_identity();

                changes.push(ProjectChange::Source(SourceChange::Open {
                    identity,
                    origin: SourceOrigin::Document {
                        address: target_uri.as_str().to_owned(),
                        context: Some(target),
                    },
                    text: contents,
                }));

                document.source = identity;

                document.revision = SourceVersion {
                    identity,
                    revision: 1,
                };

                documents.remove(&uri);
                documents.insert(target_uri, document);
            }
        }

        project
            .change(ChangeBatch { changes })
            .map_err(|error| failure(&error))?;

        self.documents = documents;

        for source in retired {
            self.identities.forget(source);
        }

        Ok(())
    }

    pub(crate) fn capture(
        &self,
        project: &Project,
        uri: &Uri,
        options: &Options,
    ) -> Result<(ProjectSnapshot, SourceSnapshot)> {
        options.check().map_err(|error| failure(&error))?;

        if let Some(declaration) = self.declarations.get(uri) {
            let source = declaration.source.clone();

            let snapshot = ProjectSnapshot::declaration(source.clone(), declaration.origin.clone())
                .map_err(|error| failure(&error))?;

            return Ok((snapshot, source));
        }

        if definition(uri) {
            return Err(Error::invalid_params("definition URI has expired"));
        }

        if !self.documents.contains_key(uri) && uri.scheme() == "file" {
            return project
                .capture(&path(uri)?, options)
                .map_err(|error| failure(&error));
        }

        let snapshot = project.snapshot();

        let source = if let Some(document) = self.documents.get(uri) {
            if !document.synchronized {
                return Err(Error::invalid_params(
                    "document requires full-content resynchronization",
                ));
            }

            let source = snapshot
                .source(document.source)
                .ok_or_else(Error::content_modified)?;

            if source.version != document.revision {
                return Err(Error::content_modified());
            }

            source
        } else {
            snapshot
                .source_for_address(uri.as_str())
                .ok_or_else(|| Error::invalid_params("document is not open"))?
        };

        let source = source.clone();

        Ok((snapshot, source))
    }

    pub(crate) fn editable(&self, uri: &Uri) -> Result<()> {
        if uri.scheme() == "instar"
            || self
                .documents
                .get(uri)
                .is_some_and(|document| !document.writable)
        {
            return Err(Error::invalid_params("document is read-only"));
        }

        Ok(())
    }

    pub(crate) fn current(&self, project: &Project, inputs: &[InputVersion]) -> bool {
        project.current(inputs) && self.documents.values().all(|document| document.synchronized || !inputs.iter().any(|input| matches!(input.identity, InputIdentity::Source(source) | InputIdentity::Configuration(source) | InputIdentity::Sourcemap(source) if source == document.source)))
    }

    pub(crate) fn uri(
        &mut self,
        snapshot: &ProjectSnapshot,
        source: SourceIdentity,
    ) -> Result<Uri> {
        let captured = snapshot
            .source(source)
            .ok_or_else(Error::content_modified)?;

        if snapshot.is_declaration(captured.version) {
            let uri: Uri = format!(
                "instar://definition/{}/{}",
                source.get(),
                captured.version.revision
            )
            .parse()
            .map_err(|error| Error::invalid_params(format!("invalid declaration URI: {error}")))?;

            if self.declarations.get(&uri).is_some_and(|published| {
                published.source != *captured || snapshot.origin(source) != Some(&published.origin)
            }) {
                return Err(Error::content_modified());
            }

            if !self.declarations.contains_key(&uri) {
                let origin = snapshot
                    .origin(source)
                    .cloned()
                    .ok_or_else(Error::content_modified)?;

                self.declarations.insert(
                    uri.clone(),
                    Declaration {
                        source: captured.clone(),
                        origin,
                        open: false,
                    },
                );

                self.declaration_order.push_back(uri.clone());
                self.trim_declarations();
            }

            return Ok(uri);
        }

        if let Some((uri, _)) = self.documents.iter().find(|(_, document)| {
            document.source == source && document.revision == captured.version
        }) {
            return Ok(uri.clone());
        }

        match snapshot
            .origin(source)
            .ok_or_else(Error::content_modified)?
        {
            SourceOrigin::Disk(path) => {
                Uri::from_file_path(path).map_err(|()| Error::invalid_params("invalid source URI"))
            }

            SourceOrigin::Document { address, .. } | SourceOrigin::Virtual { address } => address
                .parse()
                .map_err(|error| Error::invalid_params(format!("invalid source URI: {error}"))),

            SourceOrigin::Stdin { .. } => Err(Error::invalid_params("stdin has no editor URI")),
        }
    }

    fn trim_declarations(&mut self) {
        while self.declaration_order.len() > RETAINED_DECLARATIONS {
            if let Some(uri) = self.declaration_order.pop_front() {
                self.declarations.remove(&uri);
            }
        }
    }
}

fn definition(uri: &Uri) -> bool {
    uri.scheme() == "instar" && uri.host_str() == Some("definition")
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
