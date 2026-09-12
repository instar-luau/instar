use super::{EditorEntry, Request, Response, documentation, failure, moved, path, workspace};
use crate::{
    analysis,
    project::Configuration,
    source::{PositionEncoding, SourceStore},
};
use line_index::LineCol;
use std::{collections::BTreeMap, path::PathBuf};
use tokio::sync::oneshot;
use tower_lsp_server::ls_types as protocol;
use tower_lsp_server::{
    jsonrpc::{Error, Result},
    ls_types::{
        Diagnostic, DiagnosticSeverity, Position, PublishDiagnosticsParams, Range, TextEdit, Uri,
    },
};

struct Document {
    uri: Uri,
    version: i32,
}

#[derive(Default)]
pub(super) struct State {
    pub(super) sources: SourceStore,
    session: analysis::Session,
    documents: BTreeMap<PathBuf, Document>,
    published: BTreeMap<PathBuf, Uri>,
    roots: std::collections::BTreeSet<PathBuf>,
    files: Option<Vec<PathBuf>>,
    pub(super) index: super::index::Index,
    pub(super) settings: super::hints::Settings,
    pub(super) progress: Option<tokio::sync::mpsc::UnboundedSender<(usize, usize)>>,
}

impl State {
    fn move_files(&mut self, parameters: protocol::RenameFilesParams) -> Result<Response> {
        self.session.refresh();

        for file in parameters.files {
            let old = path(&file.old_uri.parse().map_err(failure)?)?;
            let new: Uri = file.new_uri.parse().map_err(failure)?;
            let mut moves = Vec::new();

            for (path, document) in &self.documents {
                if let Some(uri) = moved(&document.uri, &old, &new)? {
                    moves.push((path.clone(), uri, document.version));
                }
            }

            for (original, uri, version) in moves {
                let source = self.sources.read(&original).map_err(failure)?;
                let target = path(&uri)?;

                if target == original {
                    if let Some(document) = self.documents.get_mut(&original) {
                        document.uri = uri;
                    }

                    continue;
                }

                if !self.sources.is_open(&target).map_err(failure)? {
                    let updated = self
                        .sources
                        .open_bytes(&target, version, source.bytes().to_vec())
                        .map_err(failure)?;

                    self.documents
                        .insert(updated.path().to_owned(), Document { uri, version });
                }

                self.sources.close(&source).map_err(failure)?;
                self.documents.remove(&original);
            }

            self.roots = self
                .roots
                .iter()
                .map(|root| {
                    let uri = Uri::from_file_path(root)
                        .ok_or_else(|| failure("invalid workspace URI"))?;

                    moved(&uri, &old, &new)?.map_or_else(|| Ok(root.clone()), |uri| path(&uri))
                })
                .collect::<Result<_>>()?;
        }

        Ok(Response::Diagnostics(Vec::new()))
    }

    pub(super) fn environment(
        &mut self,
        path: &std::path::Path,
    ) -> Result<std::sync::Arc<crate::roblox::Environment>> {
        self.session
            .environment(&mut self.sources, path)
            .map_err(failure)
    }

    pub(super) fn workspace(&mut self) -> Result<Vec<PathBuf>> {
        if self.files.is_none() {
            self.files = Some(workspace::files(&self.roots).map_err(failure)?);
        }

        let mut paths = self
            .files
            .iter()
            .flatten()
            .cloned()
            .collect::<std::collections::BTreeSet<_>>();

        paths.extend(self.documents.keys().cloned());

        Ok(paths.into_iter().collect())
    }

    fn range(&mut self, path: &std::path::Path, coordinates: [u32; 4]) -> Result<Range> {
        let source = self.sources.read(path).map_err(failure)?;

        Self::source_range(&source, coordinates)
    }

    pub(super) fn source_range(
        source: &crate::source::Source,
        coordinates: [u32; 4],
    ) -> Result<Range> {
        let position = |line, col| {
            let offset = source
                .offset(LineCol { line, col }, PositionEncoding::Utf8)
                .map_err(failure)?;

            let position = source
                .position(offset, PositionEncoding::Utf16)
                .map_err(failure)?;

            Ok(Position::new(position.line, position.col))
        };

        Ok(Range::new(
            position(coordinates[0], coordinates[1])?,
            position(coordinates[2], coordinates[3])?,
        ))
    }

    fn operation(
        &mut self,
        path: &std::path::Path,
        position: LineCol,
        operation: &'static str,
    ) -> Result<&'static str> {
        if operation == "references" {
            let scope = self
                .session
                .query(
                    &mut self.sources,
                    &[path.to_owned()],
                    path,
                    position,
                    "scope",
                )
                .map_err(failure)?;

            if matches!(scope.editor, Some(analysis::EditorResult::Entry(entry)) if entry.kind == Some(13))
            {
                return Ok("localReferences");
            }
        }

        Ok(operation)
    }

    pub(super) fn query(
        &mut self,
        parameters: &protocol::TextDocumentPositionParams,
        operation: &'static str,
    ) -> Result<Response> {
        let source = self
            .sources
            .read(&path(&parameters.text_document.uri)?)
            .map_err(failure)?;

        let offset = source
            .offset(
                LineCol {
                    line: parameters.position.line,
                    col: parameters.position.character,
                },
                PositionEncoding::Utf16,
            )
            .map_err(failure)?;

        let position = source
            .position(offset, PositionEncoding::Utf8)
            .map_err(failure)?;

        let operation = self.operation(source.path(), position, operation)?;

        let modules = if matches!(operation, "references" | "implementation") {
            let mut modules = self.workspace()?;

            if !modules.iter().any(|path| path == source.path()) {
                modules.push(source.path().to_owned());
            }

            modules
        } else {
            vec![source.path().to_owned()]
        };

        let report = self
            .session
            .query(
                &mut self.sources,
                &modules,
                source.path(),
                position,
                operation,
            )
            .map_err(failure)?;

        self.entries(report, source.path())
    }

    fn entries(&mut self, report: analysis::Report, path: &std::path::Path) -> Result<Response> {
        let entries = match report.editor {
            Some(analysis::EditorResult::Entries(entries)) => entries,
            Some(analysis::EditorResult::Entry(entry)) => vec![*entry],
            None => Vec::new(),
        };

        let mut results = Vec::new();
        let mut snapshots = BTreeMap::new();

        for mut entry in entries {
            if let Some(error) = &entry.error {
                return Err(failure(error));
            }

            let target = entry.path.as_deref().unwrap_or(path);

            let snapshot = if target.is_absolute() {
                Some(match snapshots.entry(target.to_owned()) {
                    std::collections::btree_map::Entry::Occupied(entry) => entry.into_mut(),

                    std::collections::btree_map::Entry::Vacant(entry) => {
                        entry.insert(self.sources.read(target).map_err(failure)?)
                    }
                })
            } else {
                None
            };

            let location = if let Some(range) = entry.range {
                if let Some(snapshot) = snapshot.as_deref() {
                    let uri = self
                        .documents
                        .get(target)
                        .map(|document| document.uri.clone())
                        .or_else(|| Uri::from_file_path(target))
                        .ok_or_else(|| failure("invalid target URI"))?;

                    Some(protocol::Location::new(
                        uri,
                        Self::source_range(snapshot, range)?,
                    ))
                } else {
                    None
                }
            } else {
                None
            };

            let selection = if let Some(snapshot) = snapshot.as_deref() {
                entry
                    .selection
                    .map(|selection| Self::source_range(snapshot, selection))
                    .transpose()?
            } else {
                None
            };

            let documentation = entry
                .documentation
                .as_ref()
                .and_then(|symbol| {
                    report
                        .documentation
                        .values()
                        .find_map(|documentation| documentation::render(documentation, symbol))
                })
                .or_else(|| {
                    entry
                        .documentation_text
                        .take()
                        .map(|text| documentation::comments(&text))
                        .filter(|text| !text.is_empty())
                });

            results.push(EditorEntry {
                native: entry,
                location,
                selection,
                documentation,
            });
        }

        Ok(Response::Editor(results))
    }

    fn rename(&mut self, parameters: &protocol::RenameParams) -> Result<Response> {
        let name = &parameters.new_name;

        if !super::identifier(name) {
            return Err(Error::invalid_params("expected a Luau identifier"));
        }

        let Response::Editor(entries) =
            self.query(&parameters.text_document_position, "references")?
        else {
            return Err(Error::internal_error());
        };

        if !entries
            .iter()
            .any(|entry| entry.native.declaration == Some(true))
        {
            return Err(Error::invalid_params("symbol has no editable declaration"));
        }

        let mut changes = BTreeMap::<String, Vec<TextEdit>>::new();

        for entry in entries {
            let Some(location) = entry.location else {
                continue;
            };

            changes
                .entry(location.uri.to_string())
                .or_default()
                .push(TextEdit {
                    range: entry.selection.unwrap_or(location.range),
                    new_text: name.clone(),
                });
        }

        let mut documents = Vec::new();

        for (uri, mut edits) in changes {
            let uri: Uri = uri.parse().map_err(failure)?;

            let position = protocol::TextDocumentPositionParams {
                text_document: protocol::TextDocumentIdentifier { uri: uri.clone() },
                position: Position::default(),
            };

            if let Response::Editor(entries) = self.query(&position, "tokens")?
                && entries.iter().any(|entry| {
                    entry.native.name.as_ref() == Some(name)
                        && entry.location.as_ref().is_some_and(|location| {
                            !edits.iter().any(|edit| edit.range == location.range)
                        })
                })
            {
                return Err(Error::invalid_params(
                    "new name conflicts with an existing identifier",
                ));
            }

            edits.sort_by_key(|edit| (edit.range.start.line, edit.range.start.character));
            edits.dedup_by(|left, right| left.range == right.range);
            let source = self.sources.read(&path(&uri)?).map_err(failure)?;

            let version = self
                .documents
                .get(source.path())
                .map(|document| document.version);

            documents.push(protocol::TextDocumentEdit {
                text_document: protocol::OptionalVersionedTextDocumentIdentifier { uri, version },
                edits: edits.into_iter().map(protocol::OneOf::Left).collect(),
            });
        }

        Ok(Response::Rename(protocol::WorkspaceEdit {
            document_changes: Some(protocol::DocumentChanges::Edits(documents)),
            ..protocol::WorkspaceEdit::default()
        }))
    }

    fn changed(&mut self, path: &std::path::Path) {
        self.index.invalidate(path);

        if matches!(
            path.file_name().and_then(|name| name.to_str()),
            Some(".config.luau" | "config.luau")
        ) {
            self.files = None;
            self.index.files.clear();
            self.session.refresh();
        } else {
            self.session.change(path);
        }
    }

    fn close(&mut self, uri: &Uri) -> Result<()> {
        let path = path(uri)?;

        if !self.sources.is_open(&path).map_err(failure)? {
            return Ok(());
        }

        let source = self.sources.read(&path).map_err(failure)?;
        self.sources.close(&source).map_err(failure)?;
        self.documents.remove(source.path());
        self.changed(source.path());

        Ok(())
    }

    fn configure(&mut self, settings: &serde_json::Value) -> Result<()> {
        self.files = None;
        self.index.files.clear();
        self.session.refresh();

        self.settings = if settings.is_null() {
            super::hints::Settings::default()
        } else {
            serde_json::from_value(settings.get("instar").unwrap_or(settings).clone())
                .map_err(failure)?
        };

        Ok(())
    }

    pub(super) fn handle(&mut self, request: Request) -> Result<Response> {
        if matches!(
            request,
            Request::Refresh | Request::Move(_) | Request::Workspace(_, _)
        ) {
            self.files = None;
            self.index.files.clear();
        }

        match request {
            Request::Open(parameters) => {
                let document = parameters.text_document;

                let source = self
                    .sources
                    .open(&path(&document.uri)?, document.version, &document.text)
                    .map_err(failure)?;

                self.changed(source.path());

                self.documents.insert(
                    source.path().to_owned(),
                    Document {
                        uri: document.uri,
                        version: document.version,
                    },
                );
            }

            Request::Change(parameters) => {
                let source = self
                    .sources
                    .read(&path(&parameters.text_document.uri)?)
                    .map_err(failure)?;

                let text = super::sync::apply(&source, parameters.content_changes)?;

                let updated = self
                    .sources
                    .update(&source, parameters.text_document.version, &text)
                    .map_err(failure)?;

                let document = self
                    .documents
                    .get_mut(updated.path())
                    .ok_or_else(|| Error::invalid_params("document is not open"))?;

                document.version = parameters.text_document.version;

                self.changed(updated.path());
            }

            Request::Close(parameters) => self.close(&parameters.text_document.uri)?,

            Request::Refresh => self.session.refresh(),
            Request::Configure(settings) => self.configure(&settings)?,

            Request::Hints(parameters) => {
                return super::hints::hints(self, &parameters).map(Response::Hints);
            }

            Request::Query(parameters, operation) => return self.query(&parameters, operation),
            Request::Rename(parameters) => return self.rename(&parameters),
            Request::Move(parameters) => return self.move_files(parameters),
            Request::Moving(parameters) => return super::renames::edits(self, &parameters),

            Request::Workspace(added, removed) => {
                for uri in removed {
                    self.roots.remove(&path(&uri)?);
                }

                for uri in added {
                    self.roots.insert(path(&uri)?);
                }

                return Ok(Response::Diagnostics(Vec::new()));
            }

            Request::Symbols(query) => return self.symbols(&query),

            Request::Complete(parameters) => {
                return super::imports::complete(self, &parameters).map(Response::Completions);
            }

            Request::Hierarchy(item, incoming) => {
                return super::hierarchy::calls(self, &item, incoming);
            }

            Request::Prepare(parameters) => {
                return super::hierarchy::prepare(self, &parameters).map(Response::Prepared);
            }

            Request::Actions(parameters) => {
                return super::actions::actions(self, parameters).map(Response::Actions);
            }

            Request::Format(parameters) => return self.format(&parameters.text_document.uri),
            Request::Range(parameters) => return super::formatting::range(self, &parameters),

            Request::Resolve(item) => {
                return super::imports::resolve(self, *item)
                    .map(Box::new)
                    .map(Response::Completion);
            }

            Request::Diagnostic(uri) => {
                return self
                    .diagnostics_for(&[path(&uri)?], false)
                    .map(Response::Diagnostics);
            }
        }

        Ok(Response::Diagnostics(Vec::new()))
    }

    pub(super) fn indexed(&mut self) -> Result<()> {
        let pending = self
            .workspace()?
            .into_iter()
            .filter(|path| !self.index.files.contains_key(path))
            .collect::<Vec<_>>();

        let total = pending.len();

        for (completed, path) in pending.into_iter().enumerate() {
            if let Some(progress) = &self.progress {
                progress
                    .send((completed, total))
                    .map_err(|_| Error::request_cancelled())?;
            }

            let uri = Uri::from_file_path(&path).ok_or_else(|| failure("invalid workspace URI"))?;

            let parameters = protocol::TextDocumentPositionParams {
                text_document: protocol::TextDocumentIdentifier { uri },
                position: Position::default(),
            };

            let Response::Editor(symbols) = self.query(&parameters, "symbols")? else {
                return Err(Error::internal_error());
            };

            let Response::Editor(calls) = self.query(&parameters, "calls")? else {
                return Err(Error::internal_error());
            };

            let Response::Editor(links) = self.query(&parameters, "links")? else {
                return Err(Error::internal_error());
            };

            let dependencies = links
                .into_iter()
                .filter_map(|entry| entry.native.name.map(PathBuf::from))
                .collect();

            self.index.files.insert(
                path,
                super::index::File {
                    symbols,
                    calls,
                    dependencies,
                },
            );
        }

        if total != 0
            && let Some(progress) = &self.progress
        {
            progress
                .send((total, total))
                .map_err(|_| Error::request_cancelled())?;
        }

        Ok(())
    }

    fn symbols(&mut self, query: &str) -> Result<Response> {
        self.indexed()?;

        Ok(Response::Editor(
            self.index
                .files
                .values()
                .flat_map(|file| &file.symbols)
                .filter(|entry| {
                    entry
                        .native
                        .name
                        .as_ref()
                        .is_some_and(|name| name.contains(query))
                })
                .cloned()
                .collect(),
        ))
    }

    fn related(
        &mut self,
        information: Vec<analysis::RelatedDiagnostic>,
    ) -> Result<Option<Vec<protocol::DiagnosticRelatedInformation>>> {
        let mut related = Vec::new();

        for information in information {
            if let Some(uri) = self
                .documents
                .get(&information.path)
                .map(|document| document.uri.clone())
                .or_else(|| Uri::from_file_path(&information.path))
            {
                related.push(protocol::DiagnosticRelatedInformation {
                    location: protocol::Location::new(
                        uri,
                        self.range(&information.path, information.range)?,
                    ),
                    message: information.message,
                });
            }
        }

        Ok((!related.is_empty()).then_some(related))
    }

    pub(super) fn publish(&mut self, replies: &mut Vec<oneshot::Sender<Result<Response>>>) {
        if let Some(last) = replies.pop() {
            for reply in replies.drain(..) {
                drop(reply.send(Ok(Response::Diagnostics(Vec::new()))));
            }

            drop(last.send(self.diagnostics().map(Response::Diagnostics)));
        }
    }

    pub(super) fn version(&self, path: &std::path::Path) -> Option<i32> {
        self.documents.get(path).map(|document| document.version)
    }

    pub(super) fn format(&mut self, uri: &Uri) -> Result<Response> {
        let source = self.sources.read(&path(uri)?).map_err(failure)?;
        let configuration = Configuration::discover(source.path(), None).map_err(failure)?;
        let output = configuration.format(source.bytes()).map_err(failure)?;

        if output == source.bytes() {
            return Ok(Response::Edits(None));
        }

        let length = u32::try_from(source.bytes().len()).map_err(failure)?;

        let end = source
            .position(length.into(), PositionEncoding::Utf16)
            .map_err(failure)?;

        Ok(Response::Edits(Some(vec![TextEdit {
            range: Range::new(Position::default(), Position::new(end.line, end.col)),
            new_text: String::from_utf8(output).map_err(failure)?,
        }])))
    }

    fn diagnostics(&mut self) -> Result<Vec<PublishDiagnosticsParams>> {
        self.diagnostics_for(&self.documents.keys().cloned().collect::<Vec<_>>(), true)
    }

    fn report(&mut self, paths: &[PathBuf], publish: bool) -> std::io::Result<analysis::Report> {
        if publish {
            self.session
                .analyze(&mut self.sources, paths, &analysis::Options::default())
        } else {
            self.session.query(
                &mut self.sources,
                paths,
                &paths[0],
                LineCol { line: 0, col: 0 },
                "diagnostics",
            )
        }
    }

    fn diagnostics_for(
        &mut self,
        paths: &[PathBuf],
        publish: bool,
    ) -> Result<Vec<PublishDiagnosticsParams>> {
        let mut diagnostics = paths
            .iter()
            .map(|path| (path.clone(), Vec::new()))
            .collect::<BTreeMap<_, _>>();

        if !paths.is_empty() {
            match self.report(paths, publish) {
                Ok(report) => {
                    for diagnostic in report.diagnostics {
                        let severity = if diagnostic.is_error {
                            DiagnosticSeverity::ERROR
                        } else {
                            DiagnosticSeverity::WARNING
                        };

                        if !diagnostic.path.is_absolute() {
                            for messages in diagnostics.values_mut() {
                                messages.push(Diagnostic {
                                    severity: Some(severity),
                                    message: diagnostic.message.clone(),
                                    ..Diagnostic::default()
                                });
                            }

                            continue;
                        }

                        let range = self.range(
                            &diagnostic.path,
                            [
                                diagnostic.line,
                                diagnostic.column,
                                diagnostic.end_line,
                                diagnostic.end_column,
                            ],
                        )?;

                        let code = diagnostic
                            .message
                            .split_once(':')
                            .map(|(code, _)| protocol::NumberOrString::String(code.to_owned()));

                        let related_information = self.related(diagnostic.related)?;

                        diagnostics
                            .entry(diagnostic.path)
                            .or_default()
                            .push(Diagnostic {
                                range,
                                code,
                                source: Some("instar".into()),
                                related_information,
                                severity: Some(severity),
                                message: diagnostic.message,
                                ..Diagnostic::default()
                            });
                    }
                }

                Err(error) => {
                    for messages in diagnostics.values_mut() {
                        messages.push(Diagnostic {
                            severity: Some(DiagnosticSeverity::ERROR),
                            message: error.to_string(),
                            ..Diagnostic::default()
                        });
                    }
                }
            }
        }

        let mut publications = Vec::new();
        let mut published = BTreeMap::new();

        for (path, diagnostics) in diagnostics {
            let document = self.documents.get(&path);

            let uri = if let Some(document) = document {
                document.uri.clone()
            } else if let Some(uri) = self.published.get(&path) {
                uri.clone()
            } else {
                url::Url::from_file_path(&path)
                    .map_err(|()| Error::invalid_params("invalid document path"))?
                    .as_str()
                    .parse()
                    .map_err(failure)?
            };

            published.insert(path, uri.clone());

            publications.push(PublishDiagnosticsParams {
                uri,
                diagnostics,
                version: document.map(|document| document.version),
            });
        }

        if !publish {
            return Ok(publications);
        }

        let mut cleared = Vec::new();

        for (path, uri) in &self.published {
            if published.get(path) != Some(uri) {
                cleared.push(PublishDiagnosticsParams::new(uri.clone(), Vec::new(), None));
            }
        }

        cleared.extend(publications);
        self.published = published;

        Ok(cleared)
    }
}
