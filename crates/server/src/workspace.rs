use std::{
    collections::{BTreeMap, BTreeSet},
    io,
    path::PathBuf,
};

use instar_core::{project::Change, snapshot::Snapshot};

use tower_lsp_server::{
    gen_lsp_types::{
        DidChangeTextDocumentParams, DidCloseTextDocumentParams, DidOpenTextDocumentParams,
        DidSaveTextDocumentParams, PositionEncodingKind, TextDocumentContentChangeEvent, Uri,
    },
    jsonrpc::{Error, Result},
};

use crate::position;

struct Document {
    version: i32,
    synchronized: bool,
}

pub(crate) struct Workspace {
    pub(crate) snapshot: Snapshot,
    pub(crate) roots: BTreeSet<PathBuf>,
    pub(crate) encoding: PositionEncodingKind,
    documents: BTreeMap<PathBuf, Document>,
}

impl Default for Workspace {
    fn default() -> Self {
        Self {
            snapshot: Snapshot::default(),
            roots: BTreeSet::new(),
            encoding: PositionEncodingKind::UTF16,
            documents: BTreeMap::new(),
        }
    }
}

impl Workspace {
    pub(crate) fn open(&mut self, parameters: DidOpenTextDocumentParams) -> Result<()> {
        let document = parameters.text_document;
        let path = path(&document.uri)?;

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
            },
        );

        Ok(())
    }

    pub(crate) fn change(&mut self, parameters: DidChangeTextDocumentParams) -> Result<()> {
        let path = path(&parameters.text_document.text_document_identifier.uri)?;

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
        let path = path(&document.uri)?;

        if self.documents.remove(&path).is_none() {
            return Err(Error::invalid_params("document is not open"));
        }

        self.snapshot
            .change(Change::Close(path))
            .map_err(|error| failure(&error))
    }

    pub(crate) fn save(&mut self, parameters: DidSaveTextDocumentParams) -> Result<()> {
        let document = parameters.text_document;
        let path = path(&document.uri)?;

        if !self.documents.contains_key(&path) {
            return Err(Error::invalid_params("document is not open"));
        }

        self.snapshot
            .change(Change::Disk(path))
            .map_err(|error| failure(&error))
    }

    pub(crate) fn capture(&self, uri: &Uri) -> Result<(Snapshot, PathBuf, PositionEncodingKind)> {
        let path = path(uri)?;

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
