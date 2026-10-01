use std::{
    collections::BTreeMap,
    io,
    path::{Path, PathBuf},
    sync::Arc,
};

use instar_core::analysis::Editor;
use serde_json::Value;
use tower_lsp_server::ls_types::{self as lsp, Diagnostic};

use crate::document::{Document, uri};

pub(crate) enum Query {
    Hover(u32, u32),
    Completion(u32, u32),
    Signature(u32, u32),
    Definition(u32, u32),
    Declaration(u32, u32),
    Implementation(u32, u32),
    TypeDefinition(u32, u32),
    References(u32, u32, bool),
    Prepare(u32, u32),
    Rename(u32, u32, String),
    Highlights(u32, u32),
    Hints(tower_lsp_server::ls_types::Range, bool, bool, bool),
    Actions(tower_lsp_server::ls_types::Range),
}

pub(crate) struct Worker {
    editor: Editor,
    documents: BTreeMap<PathBuf, Arc<Document>>,
    epoch: u64,
    folders: Vec<PathBuf>,
    indexed: bool,
    pub(crate) revision: u64,
    diagnostics_revision: u64,
    pub(crate) diagnostics: BTreeMap<PathBuf, Vec<Diagnostic>>,
    workspace: crate::workspace::Workspace,
}

impl Default for Worker {
    fn default() -> Self {
        Self {
            editor: Editor::default(),
            documents: BTreeMap::new(),
            epoch: 0,
            folders: Vec::new(),
            indexed: false,
            revision: u64::MAX,
            diagnostics_revision: u64::MAX,
            diagnostics: BTreeMap::new(),
            workspace: crate::workspace::Workspace::default(),
        }
    }
}

impl Worker {
    pub(crate) fn take_asset_warnings(&mut self) -> Vec<String> {
        self.editor.take_asset_warnings()
    }

    pub(crate) fn update(
        &mut self,
        snapshot: &crate::Snapshot,
        diagnostics: bool,
    ) -> io::Result<()> {
        if self.revision == snapshot.revision
            && (!diagnostics || self.diagnostics_revision == snapshot.revision)
        {
            return Ok(());
        }

        if self.revision != snapshot.revision {
            self.workspace.update(snapshot)?;

            for path in self.documents.keys().filter(|path| {
                !snapshot.documents.contains_key(*path) || snapshot.preparing.contains_key(*path)
            }) {
                self.editor.set_source(path, None)?;
            }

            for (path, document) in &snapshot.documents {
                if snapshot.preparing.contains_key(path) {
                    continue;
                }

                if self
                    .documents
                    .get(path)
                    .is_none_or(|old| !Arc::ptr_eq(old, document))
                {
                    self.editor.set_source(path, Some(&document.text))?;
                }
            }

            if self.epoch != snapshot.epoch {
                self.editor.refresh();
            }

            self.epoch = snapshot.epoch;

            self.documents = snapshot
                .documents
                .iter()
                .filter(|(path, _)| !snapshot.preparing.contains_key(*path))
                .map(|(path, document)| (path.clone(), Arc::clone(document)))
                .collect();

            self.folders = snapshot.folders.iter().cloned().collect();
            self.indexed = false;
            self.diagnostics_revision = u64::MAX;
            self.diagnostics.clear();
        }

        if !diagnostics {
            self.editor.check_for_query()?;
            self.revision = snapshot.revision;

            return Ok(());
        }

        self.check_diagnostics(snapshot.revision)?;
        self.revision = snapshot.revision;

        Ok(())
    }

    fn check_diagnostics(&mut self, revision: u64) -> io::Result<()> {
        if self.diagnostics_revision == revision {
            return Ok(());
        }

        let mut findings = self.editor.check()?;
        findings.retain(|finding| self.documents.contains_key(&finding.location.module.source));
        self.diagnostics = crate::document::diagnostics(findings, |path| self.document(path))?;
        self.diagnostics_revision = revision;

        Ok(())
    }

    fn index_workspace(&mut self) -> io::Result<()> {
        if self.indexed {
            return Ok(());
        }

        let paths = self.workspace.files(&|| false)?;

        self.editor.index(&paths)?;
        self.indexed = true;

        Ok(())
    }

    fn document(&mut self, path: &Path) -> io::Result<Arc<Document>> {
        if let Some(document) = self.documents.get(path) {
            return Ok(Arc::clone(document));
        }

        Ok(Arc::new(Document::new(
            uri(path)?,
            0,
            self.editor.source(path)?.to_string(),
        )?))
    }

    fn target(&mut self, name: &str, range: [u32; 4]) -> io::Result<lsp::Location> {
        let path = self.editor.source_path(name)?;
        let document = self.document(&path)?;

        Ok(lsp::Location {
            uri: document.uri.clone(),
            range: document.native_range(range),
        })
    }

    fn editable(&self, path: &Path) -> bool {
        !path
            .file_name()
            .is_some_and(|name| name.to_string_lossy().ends_with(".d.luau"))
            && (self.documents.contains_key(path)
                || self.folders.iter().any(|root| path.starts_with(root)))
    }

    fn reference_target(
        &mut self,
        path: &Path,
        line: u32,
        column: u32,
    ) -> io::Result<(Option<instar_bridge::ReferenceTarget>, bool)> {
        let targets = self.editor.query_all(path, |checker, host, module| {
            checker.reference_target(host, module, line, column)
        })?;

        let mut selected: Option<instar_bridge::ReferenceTarget> = None;
        let mut uncertain = false;

        for target in targets {
            if let Some(target) = target {
                if let Some(previous) = &selected {
                    uncertain |= previous.name != target.name
                        || previous.local != target.local
                        || previous.property != target.property;
                } else {
                    selected = Some(target);
                }
            } else {
                uncertain = true;
            }
        }

        Ok((selected, uncertain))
    }

    fn candidate_identities(
        &mut self,
        selected: &Path,
        target: &instar_bridge::ReferenceTarget,
        new_name: Option<&str>,
    ) -> io::Result<Vec<String>> {
        let mut names = vec![target.name.as_str()];

        if let Some(new_name) = new_name {
            names.push(new_name);
        }

        let mut candidates = self
            .workspace
            .syntax_candidates(&names, target.property, &|| false)?
            .into_iter()
            .collect::<std::collections::BTreeSet<_>>();

        candidates.insert(selected.to_owned());

        let workspace = self
            .workspace
            .files(&|| false)?
            .into_iter()
            .collect::<std::collections::BTreeSet<_>>();

        self.editor
            .index(&candidates.iter().cloned().collect::<Vec<_>>())?;

        Ok(self
            .editor
            .module_identities()
            .into_iter()
            .filter(|(_, path)| candidates.contains(path) || !workspace.contains(path))
            .map(|(name, _)| name)
            .collect())
    }

    fn rename_target(
        &mut self,
        path: &Path,
        line: u32,
        column: u32,
    ) -> io::Result<Option<(PathBuf, instar_bridge::RenameTarget)>> {
        let targets = self.editor.query_all(path, |checker, host, name| {
            checker.rename_target(host, name, line, column)
        })?;

        let mut selected: Option<(PathBuf, instar_bridge::RenameTarget)> = None;

        for target in targets {
            let Some(target) = target else {
                return Ok(None);
            };

            let definition = self.editor.source_path(&target.path)?;

            if !self.editable(&definition) {
                return Ok(None);
            }

            if let Some((previous_path, previous)) = &selected {
                if previous_path != &definition
                    || previous.definition != target.definition
                    || previous.selection != target.selection
                    || previous.kind != target.kind
                    || previous.name != target.name
                {
                    return Err(io::Error::other(
                        "rename target differs between place contexts",
                    ));
                }
            } else {
                selected = Some((definition, target));
            }
        }

        Ok(selected)
    }

    fn rename(
        &mut self,
        path: &Path,
        line: u32,
        column: u32,
        name: &str,
    ) -> io::Result<lsp::WorkspaceEdit> {
        let (reference, uncertain) = self.reference_target(path, line, column)?;

        if uncertain {
            self.index_workspace()?;
        }

        let (definition, target) = self
            .rename_target(path, line, column)?
            .ok_or_else(|| io::Error::other("this symbol cannot be renamed"))?;

        let candidates = if uncertain {
            None
        } else if let Some(reference) = reference {
            if reference.local {
                None
            } else {
                Some(self.candidate_identities(&definition, &reference, Some(name))?)
            }
        } else {
            self.index_workspace()?;

            None
        };

        let results = self
            .editor
            .query_all(&definition, |checker, host, module| {
                let resolved = checker
                    .rename_target(host, module, target.definition[0], target.definition[1])?
                    .ok_or_else(|| io::Error::other("rename declaration cannot be resolved"))?;

                if resolved.path != module
                    || resolved.name != target.name
                    || resolved.definition != target.definition
                    || resolved.kind != target.kind
                {
                    return Err(io::Error::other(
                        "rename declaration differs between place contexts",
                    ));
                }

                checker.rename(
                    host,
                    module,
                    target.definition[0],
                    target.definition[1],
                    name,
                    candidates.as_deref(),
                )
            })?;

        let mut ranges = BTreeMap::<PathBuf, Vec<[u32; 4]>>::new();

        for result in results.into_iter().flatten() {
            let path = self.editor.source_path(&result.path)?;

            if !self.editable(&path) {
                return Err(io::Error::other(
                    "rename would edit a source outside the workspace",
                ));
            }

            ranges.entry(path).or_default().push(result.range);
        }

        if !ranges
            .get(&definition)
            .is_some_and(|ranges| ranges.contains(&target.definition))
        {
            return Err(io::Error::other("native rename omitted its declaration"));
        }

        self.rename_edits(ranges, &target.name, name)
    }

    fn rename_edits(
        &mut self,
        ranges: BTreeMap<PathBuf, Vec<[u32; 4]>>,
        old_name: &str,
        new_name: &str,
    ) -> io::Result<lsp::WorkspaceEdit> {
        let mut changes = Vec::new();

        for (path, mut ranges) in ranges {
            ranges.sort_unstable();
            ranges.dedup();

            if ranges
                .windows(2)
                .any(|pair| (pair[0][2], pair[0][3]) > (pair[1][0], pair[1][1]))
            {
                return Err(io::Error::other(
                    "native rename returned overlapping occurrences",
                ));
            }

            let document = self.document(&path)?;
            let version = self.documents.get(&path).map(|document| document.version);

            if version.is_none() && std::fs::read_to_string(&path)? != document.text {
                return Err(io::Error::other("a source changed on disk during rename"));
            }

            let mut edits = Vec::with_capacity(ranges.len());

            for range in ranges {
                let range = document.native_range(range);

                let spelling =
                    &document.text[document.offset(range.start)?..document.offset(range.end)?];

                let replacement = if spelling == old_name {
                    new_name.to_owned()
                } else if instar_core::string_value(spelling.as_bytes())
                    .is_ok_and(|value| value == old_name)
                {
                    format!("\"{new_name}\"")
                } else {
                    return Err(io::Error::other(
                        "native rename occurrence does not match its source",
                    ));
                };

                edits.push(lsp::OneOf::Left(lsp::TextEdit {
                    range,
                    new_text: replacement,
                }));
            }

            changes.push(lsp::TextDocumentEdit {
                text_document: lsp::OptionalVersionedTextDocumentIdentifier {
                    uri: document.uri.clone(),
                    version,
                },
                edits,
            });
        }

        Ok(lsp::WorkspaceEdit {
            document_changes: Some(lsp::DocumentChanges::Edits(changes)),
            ..lsp::WorkspaceEdit::default()
        })
    }

    fn documentation(&mut self, path: &Path, symbol: &str) -> io::Result<Option<String>> {
        if symbol.is_empty() {
            return Ok(None);
        }

        let Some(docs) = self.editor.documentation(path, symbol)? else {
            return Ok(None);
        };

        let Some(value) = docs.get(symbol) else {
            return Ok(None);
        };

        let mut text = value
            .as_str()
            .or_else(|| value.get("documentation").and_then(Value::as_str))
            .unwrap_or_default()
            .to_owned();

        if text.trim().is_empty() {
            text.clear();
        }

        if let Some(sample) = value
            .get("code_sample")
            .and_then(Value::as_str)
            .filter(|sample| !sample.trim().is_empty())
        {
            if !text.is_empty() {
                text.push_str("\n\n---\n\n");
            }

            let fence = sample
                .split(|c| c != '`')
                .map(str::len)
                .max()
                .unwrap_or(0)
                .saturating_add(1)
                .max(3);

            text.extend(std::iter::repeat_n('`', fence));
            text.push_str("luau\n");
            text.push_str(sample);
            text.push('\n');
            text.extend(std::iter::repeat_n('`', fence));
        }

        if let Some(link) = value
            .get("link")
            .and_then(Value::as_str)
            .filter(|link| !link.is_empty())
        {
            if !text.is_empty() {
                text.push_str("\n\n---\n\n");
            }

            text.push_str("[Learn more](");
            text.push_str(link);
            text.push(')');
        }

        Ok((!text.is_empty()).then_some(text))
    }

    pub(crate) fn query(&mut self, path: &Path, query: &Query) -> io::Result<crate::Response> {
        if matches!(query, Query::Implementation(..)) {
            self.index_workspace()?;
        }

        let document = self.document(path)?;

        match *query {
            Query::Hover(line, column) => self
                .hover(&document, line, column)
                .map(crate::Response::Hover),

            Query::Completion(line, column) => self
                .completion(&document, line, column)
                .map(crate::Response::Completion),

            Query::Hints(range, types, parameters, requires) => self
                .hints(&document, range, types, parameters, requires)
                .map(crate::Response::Hints),

            Query::Actions(range) => self.actions(&document, range).map(crate::Response::Actions),

            Query::Signature(line, column) => self
                .signature(&document, line, column)
                .map(crate::Response::Signature),

            Query::Definition(line, column)
            | Query::Declaration(line, column)
            | Query::Implementation(line, column)
            | Query::TypeDefinition(line, column) => self
                .navigation(path, query, line, column)
                .map(crate::Response::Locations),

            Query::References(line, column, include_declaration) => {
                let (target, uncertain) = self.reference_target(path, line, column)?;

                let candidates = if uncertain || target.is_none() {
                    self.index_workspace()?;

                    None
                } else if let Some(target) = target {
                    if target.local {
                        None
                    } else {
                        Some(self.candidate_identities(path, &target, None)?)
                    }
                } else {
                    None
                };

                let results = self.editor.query_all(path, |checker, host, name| {
                    checker.references(host, name, line, column, candidates.as_deref())
                })?;

                let mut targets = Vec::new();

                for result in results.into_iter().flatten() {
                    if (include_declaration || !result.declaration)
                        && let Ok(target) = self.target(&result.path, result.range)
                    {
                        targets.push(target);
                    }
                }

                targets.sort_unstable_by(|left, right| {
                    (left.uri.as_str(), left.range.start, left.range.end).cmp(&(
                        right.uri.as_str(),
                        right.range.start,
                        right.range.end,
                    ))
                });

                targets.dedup();

                Ok(crate::Response::Locations(targets))
            }

            Query::Prepare(line, column) => {
                let result = self.rename_target(path, line, column)?;

                Ok(crate::Response::Prepare(result.map(|(_, target)| {
                    lsp::PrepareRenameResponse::RangeWithPlaceholder {
                        range: document.native_range(target.selection),
                        placeholder: target.name,
                    }
                })))
            }

            Query::Rename(line, column, ref name) => self
                .rename(path, line, column, name)
                .map(crate::Response::Edit),

            Query::Highlights(line, column) => {
                let references = self.editor.query(path, |checker, host, name| {
                    checker.references(host, name, line, column, None)
                })?;

                let mut highlights = Vec::new();

                for reference in references {
                    if self
                        .editor
                        .source_path(&reference.path)
                        .is_ok_and(|target| target == path)
                    {
                        highlights.push(lsp::DocumentHighlight {
                            range: document.native_range(reference.range),
                            kind: Some(lsp::DocumentHighlightKind::TEXT),
                        });
                    }
                }

                Ok(crate::Response::Highlights(highlights))
            }
        }
    }

    fn signature(
        &mut self,
        document: &Document,
        line: u32,
        column: u32,
    ) -> io::Result<Option<lsp::SignatureHelp>> {
        let path = &document.path;

        let result = self.editor.query(path, |checker, host, name| {
            checker.signature_help(host, name, line, column)
        })?;

        let Some(result) = result else {
            return Ok(None);
        };

        let mut signature = lsp::SignatureInformation {
            label: result.label,
            parameters: Some(
                result
                    .parameters
                    .into_iter()
                    .map(|label| lsp::ParameterInformation {
                        label: lsp::ParameterLabel::Simple(label),
                        documentation: None,
                    })
                    .collect(),
            ),
            documentation: None,
            active_parameter: None,
        };

        let offset = document.offset(document.native_range([line, column, line, column]).start)?;

        if let Some(callee) = document.features().callee(offset) {
            let (line, column) = document.byte_position(document.position(callee))?;

            if let Some(docs) = self.source_documentation(path, line, column)? {
                signature.documentation =
                    Some(lsp::Documentation::MarkupContent(lsp::MarkupContent {
                        kind: lsp::MarkupKind::Markdown,
                        value: docs,
                    }));
            }
        }

        Ok(Some(lsp::SignatureHelp {
            signatures: vec![signature],
            active_signature: Some(0),
            active_parameter: result.active_parameter,
        }))
    }

    fn navigation(
        &mut self,
        path: &Path,
        query: &Query,
        line: u32,
        column: u32,
    ) -> io::Result<Vec<lsp::Location>> {
        let results = self
            .editor
            .query_all(path, |checker, host, name| match query {
                Query::Definition(..) => checker.definition(host, name, line, column),
                Query::Declaration(..) => checker.declaration(host, name, line, column),
                Query::Implementation(..) => checker.implementation(host, name, line, column),
                _ => checker.type_definition(host, name, line, column),
            })?;

        let mut targets = Vec::new();

        for result in results.into_iter().flatten() {
            if let Ok(target) = self.target(&result.path, result.selection) {
                targets.push(target);
            }
        }

        targets.sort_unstable_by(|left, right| {
            (left.uri.as_str(), left.range.start, left.range.end).cmp(&(
                right.uri.as_str(),
                right.range.start,
                right.range.end,
            ))
        });

        targets.dedup();

        Ok(targets)
    }

    fn hover(
        &mut self,
        document: &Document,
        line: u32,
        column: u32,
    ) -> io::Result<Option<lsp::Hover>> {
        let path = &document.path;

        let result = self.editor.query(path, |checker, host, name| {
            checker.hover(host, name, line, column)
        })?;

        let Some(result) = result else {
            return Ok(None);
        };

        let label = if result.name.is_empty() {
            result.type_
        } else if result.is_type {
            format!("type {} = {}", result.name, result.type_)
        } else {
            format!("{}: {}", result.name, result.type_)
        };

        let mut text = format!("```luau\n{label}\n```");

        let docs = match self.documentation(path, &result.documentation_symbol)? {
            Some(docs) => Some(docs),
            None => self.source_documentation(path, line, column)?,
        };

        if let Some(docs) = docs {
            text.push_str("\n\n---\n\n");
            text.push_str(&docs);
        }

        Ok(Some(lsp::Hover {
            contents: lsp::HoverContents::Markup(lsp::MarkupContent {
                kind: lsp::MarkupKind::Markdown,
                value: text,
            }),
            range: result.range.map(|range| document.native_range(range)),
        }))
    }

    fn completion(
        &mut self,
        document: &Document,
        line: u32,
        column: u32,
    ) -> io::Result<lsp::CompletionResponse> {
        let path = &document.path;

        let results = self.editor.query(path, |checker, host, name| {
            checker.completion(host, name, line, column)
        })?;

        let mut items = Vec::new();

        for result in &results {
            use instar_bridge::native::EditorCompletionKind as Kind;

            let mut item = lsp::CompletionItem {
                label: result.name.clone(),
                detail: Some(result.detail.clone()),
                kind: Some(match result.kind {
                    Kind::CompletionText => lsp::CompletionItemKind::TEXT,
                    Kind::CompletionMethod => lsp::CompletionItemKind::METHOD,
                    Kind::CompletionFunction => lsp::CompletionItemKind::FUNCTION,
                    Kind::CompletionConstructor => lsp::CompletionItemKind::CONSTRUCTOR,
                    Kind::CompletionField => lsp::CompletionItemKind::FIELD,
                    Kind::CompletionVariable => lsp::CompletionItemKind::VARIABLE,
                    Kind::CompletionClass => lsp::CompletionItemKind::CLASS,
                    Kind::CompletionInterface => lsp::CompletionItemKind::INTERFACE,
                    Kind::CompletionModule => lsp::CompletionItemKind::MODULE,
                    Kind::CompletionProperty => lsp::CompletionItemKind::PROPERTY,
                    Kind::CompletionUnit => lsp::CompletionItemKind::UNIT,
                    Kind::CompletionValue => lsp::CompletionItemKind::VALUE,
                    Kind::CompletionEnum => lsp::CompletionItemKind::ENUM,
                    Kind::CompletionKeyword => lsp::CompletionItemKind::KEYWORD,
                    Kind::CompletionSnippet => lsp::CompletionItemKind::SNIPPET,
                    Kind::CompletionColor => lsp::CompletionItemKind::COLOR,
                    Kind::CompletionFile => lsp::CompletionItemKind::FILE,
                    Kind::CompletionReference => lsp::CompletionItemKind::REFERENCE,
                    Kind::CompletionFolder => lsp::CompletionItemKind::FOLDER,
                    Kind::CompletionEnumMember => lsp::CompletionItemKind::ENUM_MEMBER,
                    Kind::CompletionConstant => lsp::CompletionItemKind::CONSTANT,
                    Kind::CompletionStruct => lsp::CompletionItemKind::STRUCT,
                    Kind::CompletionEvent => lsp::CompletionItemKind::EVENT,
                    Kind::CompletionOperator => lsp::CompletionItemKind::OPERATOR,
                    Kind::CompletionTypeParameter => lsp::CompletionItemKind::TYPE_PARAMETER,
                }),
                ..lsp::CompletionItem::default()
            };

            let docs = match self.documentation(path, &result.documentation_symbol)? {
                Some(docs) => Some(docs),

                None => match &result.definition {
                    Some((module, range)) => self.declaration_documentation(module, *range)?,
                    None => None,
                },
            };

            if let Some(docs) = docs {
                item.documentation = Some(lsp::Documentation::MarkupContent(lsp::MarkupContent {
                    kind: lsp::MarkupKind::Markdown,
                    value: docs,
                }));
            }

            if result.deprecated {
                item.tags = Some(vec![lsp::CompletionItemTag::DEPRECATED]);
            }

            let insert = if result.insert.is_empty() {
                &result.name
            } else {
                &result.insert
            };

            if let Some(range) = result.range {
                item.text_edit = Some(lsp::CompletionTextEdit::Edit(lsp::TextEdit {
                    range: document.native_range(range),
                    new_text: insert.to_owned(),
                }));
            } else {
                item.insert_text = Some(insert.to_owned());
            }

            items.push(item);
        }

        let offset = document.offset(document.native_range([line, column, line, column]).start)?;
        let mut incomplete = false;

        if let Some(site) = crate::workspace::import_site(document, offset) {
            let services = self.editor.services(path)?;
            incomplete = !services.is_empty();

            items.extend(self.workspace.imports(document, site, &services, &|name| {
                results.iter().any(|item| item.name == name)
            })?);
        }

        Ok(lsp::CompletionResponse::List(lsp::CompletionList {
            is_incomplete: incomplete,
            items,
        }))
    }

    fn declaration_documentation(
        &mut self,
        module: &str,
        range: [u32; 4],
    ) -> io::Result<Option<String>> {
        let Ok(path) = self.editor.source_path(module) else {
            return Ok(None);
        };

        let document = self.document(&path)?;
        let offset = document.offset(document.native_range(range).start)?;

        Ok(document.features().documentation(&document, offset))
    }

    fn source_documentation(
        &mut self,
        path: &Path,
        line: u32,
        column: u32,
    ) -> io::Result<Option<String>> {
        let targets = self.editor.query(path, |checker, host, name| {
            checker.definition(host, name, line, column)
        })?;

        for target in targets {
            if let Some(docs) = self.declaration_documentation(&target.path, target.selection)? {
                return Ok(Some(docs));
            }
        }

        Ok(None)
    }

    fn hints(
        &mut self,
        document: &Document,
        requested: tower_lsp_server::ls_types::Range,
        types: bool,
        parameters: bool,
        requires: bool,
    ) -> io::Result<Vec<lsp::InlayHint>> {
        if !types && !parameters {
            return Ok(Vec::new());
        }

        let hints = self.editor.query(&document.path, |checker, host, name| {
            checker.type_hints(host, name)
        })?;

        let mut result = Vec::new();

        for hint in hints {
            let range = document.native_range(hint.range);
            let parameter = !hint.parameter.is_empty();

            let position = if parameter { range.start } else { range.end };

            if position < requested.start
                || position > requested.end
                || (parameter && !parameters)
                || (!parameter && !types)
            {
                continue;
            }

            if !parameter
                && !requires
                && document
                    .features()
                    .require_bindings
                    .iter()
                    .any(|span| document.range(span.start, span.end) == range)
            {
                continue;
            }

            if parameter {
                let offset = document.offset(position)?;
                let tail = &document.text[offset..];

                if tail.starts_with(&hint.parameter)
                    && tail
                        .as_bytes()
                        .get(hint.parameter.len())
                        .is_none_or(|byte| !byte.is_ascii_alphanumeric() && *byte != b'_')
                {
                    continue;
                }
            }

            let label = if parameter {
                format!("{}:", hint.parameter)
            } else {
                format!(": {}", hint.type_)
            };

            result.push(lsp::InlayHint {
                position,
                label: lsp::InlayHintLabel::String(label),
                kind: Some(if parameter {
                    lsp::InlayHintKind::PARAMETER
                } else {
                    lsp::InlayHintKind::TYPE
                }),
                padding_right: Some(parameter),
                padding_left: None,
                text_edits: None,
                tooltip: None,
                data: None,
            });
        }

        Ok(result)
    }

    fn actions(
        &mut self,
        document: &Document,
        requested: tower_lsp_server::ls_types::Range,
    ) -> io::Result<lsp::CodeActionResponse> {
        self.check_diagnostics(self.revision)?;

        let mut actions = Vec::new();

        for diagnostic in self
            .diagnostics
            .get(&document.path)
            .cloned()
            .unwrap_or_default()
        {
            let range = diagnostic.range;

            if range.end < requested.start
                || range.start > requested.end
                || !diagnostic.message.contains("Unknown global")
            {
                continue;
            }

            let offset = document.offset(range.end)?;
            let start = document.offset(range.start)?;
            let name = &document.text[start..offset];

            let Some(site) = crate::workspace::import_site(document, offset) else {
                continue;
            };

            let services = self.editor.services(&document.path)?;

            let globals = self.editor.query(&document.path, |checker, _, _| {
                let mut names = std::collections::BTreeSet::new();

                checker.globals(&mut |name| {
                    names.insert(name.to_owned());

                    Ok(())
                })?;

                Ok(names)
            })?;

            for item in self
                .workspace
                .imports(document, site, &services, &|name| globals.contains(name))?
            {
                if item.label != name {
                    continue;
                }

                let Some(lsp::CompletionTextEdit::Edit(edit)) = item.text_edit else {
                    continue;
                };

                let mut edits = item.additional_text_edits.unwrap_or_default();
                edits.push(edit);
                let detail = item.detail.as_deref().unwrap_or("");

                actions.push(lsp::CodeActionOrCommand::CodeAction(lsp::CodeAction {
                    title: format!("Import {name} from {detail}"),
                    kind: Some(lsp::CodeActionKind::QUICKFIX),
                    diagnostics: Some(vec![diagnostic.clone()]),
                    edit: Some(lsp::WorkspaceEdit {
                        document_changes: Some(lsp::DocumentChanges::Edits(vec![
                            lsp::TextDocumentEdit {
                                text_document: lsp::OptionalVersionedTextDocumentIdentifier {
                                    uri: document.uri.clone(),
                                    version: Some(document.version),
                                },
                                edits: edits.into_iter().map(lsp::OneOf::Left).collect(),
                            },
                        ])),
                        ..lsp::WorkspaceEdit::default()
                    }),
                    ..lsp::CodeAction::default()
                }));
            }
        }

        for local in &document.features().locals {
            let range = document.range(local.name.start, local.name.end);

            if range.end < requested.start || range.start > requested.end {
                continue;
            }

            let Some(replacement) = &local.replacement else {
                continue;
            };

            let (line, column) = document.byte_position(range.start)?;

            let references = self.editor.query(&document.path, |checker, host, name| {
                checker.references(host, name, line, column, None)
            })?;

            if references.is_empty() || references.iter().any(|reference| !reference.declaration) {
                continue;
            }

            actions.push(lsp::CodeActionOrCommand::CodeAction(lsp::CodeAction {
                title: "Remove unused binding".into(),
                kind: Some(lsp::CodeActionKind::QUICKFIX),
                edit: Some(lsp::WorkspaceEdit {
                    document_changes: Some(lsp::DocumentChanges::Edits(vec![
                        lsp::TextDocumentEdit {
                            text_document: lsp::OptionalVersionedTextDocumentIdentifier {
                                uri: document.uri.clone(),
                                version: Some(document.version),
                            },
                            edits: vec![lsp::OneOf::Left(lsp::TextEdit {
                                range: document.range(local.statement.start, local.statement.end),
                                new_text: replacement.clone(),
                            })],
                        },
                    ])),
                    ..lsp::WorkspaceEdit::default()
                }),
                ..lsp::CodeAction::default()
            }));
        }

        Ok(actions)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn navigation_preparation_defers_lint_until_diagnostic_publication() {
        let directory =
            std::env::temp_dir().join(format!("instar-worker-lint-{}", std::process::id()));

        std::fs::create_dir_all(&directory).unwrap();

        std::fs::write(
            directory.join("instar.toml"),
            "[lint.luau]\nTableLiteral = true\n",
        )
        .unwrap();

        let path = directory.join("main.luau");

        let mut snapshot = crate::Snapshot {
            revision: 1,
            documents: BTreeMap::from([(
                path.clone(),
                Arc::new(
                    Document::new(
                        uri(&path).unwrap(),
                        1,
                        "local t = { x = 1, x = 2 }\nprint(t.x)\nlocal list = {}\nif #list then print(1) end\n".into(),
                    )
                    .unwrap(),
                ),
            )]),
            ..crate::Snapshot::default()
        };

        let mut worker = Worker::default();

        worker.update(&snapshot, false).unwrap();
        assert!(worker.diagnostics.is_empty());

        worker.update(&snapshot, true).unwrap();

        assert!(
            worker.diagnostics[&path]
                .iter()
                .any(|item| item.message.contains("length_as_condition"))
        );

        assert!(
            worker.diagnostics[&path]
                .iter()
                .any(|item| item.message.contains("TableLiteral"))
        );

        snapshot.revision += 1;

        snapshot.documents.insert(
            path.clone(),
            Arc::new(
                Document::new(
                    uri(&path).unwrap(),
                    2,
                    "local t = { x = 1 }\nprint(t.x)\nlocal list = {}\nif #list > 0 then print(1) end\n".into(),
                )
                .unwrap(),
            ),
        );

        worker.update(&snapshot, false).unwrap();
        assert!(worker.diagnostics.is_empty());
        worker.update(&snapshot, true).unwrap();
        assert!(!worker.diagnostics.contains_key(&path));
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn import_actions_use_current_diagnostics_without_publication() {
        let directory =
            std::env::temp_dir().join(format!("instar-worker-actions-{}", std::process::id()));

        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(directory.join("Widget.luau"), "return {}\n").unwrap();
        let path = directory.join("main.luau");

        let document = Arc::new(
            Document::new(uri(&path).unwrap(), 1, "--!strict\nreturn Widget\n".into()).unwrap(),
        );

        let snapshot = crate::Snapshot {
            revision: 1,
            folders: [directory.clone()].into(),
            documents: BTreeMap::from([(path.clone(), document)]),
            ..crate::Snapshot::default()
        };

        let range = tower_lsp_server::ls_types::Range::new(
            tower_lsp_server::ls_types::Position::new(1, 7),
            tower_lsp_server::ls_types::Position::new(1, 13),
        );

        let mut expected = None;

        for diagnostics in [false, true] {
            let mut worker = Worker::default();
            worker.update(&snapshot, diagnostics).unwrap();

            let crate::Response::Actions(actions) =
                worker.query(&path, &Query::Actions(range)).unwrap()
            else {
                panic!("expected code actions");
            };

            assert!(actions.iter().any(|action| {
                let lsp::CodeActionOrCommand::CodeAction(action) = action else {
                    return false;
                };
                let Some(lsp::DocumentChanges::Edits(changes)) = action
                    .edit
                    .as_ref()
                    .and_then(|edit| edit.document_changes.as_ref())
                else {
                    return false;
                };
                action.title.starts_with("Import Widget from ")
                    && changes[0].text_document.version == Some(1)
                    && changes[0].text_document.uri == uri(&path).unwrap()
                    && changes[0].edits.iter().any(|edit| {
                        matches!(edit, lsp::OneOf::Left(edit) if edit.new_text.contains("require("))
                    })
                    && changes[0].edits.iter().any(|edit| {
                        matches!(edit, lsp::OneOf::Left(edit) if edit.range == range && edit.new_text == "Widget")
                    })
                    && action.diagnostics.as_ref().is_some_and(|items| !items.is_empty())
            }));

            if let Some(expected) = &expected {
                assert_eq!(&actions, expected);
            } else {
                expected = Some(actions);
            }

            let mut changed = snapshot.clone();
            changed.revision += 1;

            changed.documents.insert(
                path.clone(),
                Arc::new(Document::new(uri(&path).unwrap(), 2, "return 1\n".into()).unwrap()),
            );

            worker.update(&changed, false).unwrap();

            let crate::Response::Actions(actions) =
                worker.query(&path, &Query::Actions(range)).unwrap()
            else {
                panic!("expected code actions");
            };

            assert_eq!(actions, lsp::CodeActionResponse::new());
        }

        std::fs::remove_dir_all(directory).unwrap();
    }
}
