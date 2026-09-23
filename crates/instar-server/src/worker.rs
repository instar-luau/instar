use std::{
    collections::BTreeMap,
    io,
    path::{Path, PathBuf},
    sync::Arc,
};

use instar_core::analysis::Editor;
use serde_json::{Value, json};
use tower_lsp_server::ls_types::{CompletionItemTag, DiagnosticSeverity, DocumentHighlightKind};

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
    pub(crate) diagnostics: BTreeMap<PathBuf, Vec<Value>>,
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
            diagnostics: BTreeMap::new(),
            workspace: crate::workspace::Workspace::default(),
        }
    }
}

impl Worker {
    pub(crate) fn update(&mut self, snapshot: &crate::Snapshot) -> io::Result<()> {
        if self.revision == snapshot.revision {
            return Ok(());
        }

        self.workspace.update(snapshot)?;

        for path in self
            .documents
            .keys()
            .filter(|path| !snapshot.documents.contains_key(*path))
        {
            self.editor.set_source(path, None)?;
        }

        for (path, document) in &snapshot.documents {
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
        self.documents.clone_from(&snapshot.documents);
        self.folders = snapshot.folders.iter().cloned().collect();
        self.indexed = false;
        self.diagnostics.clear();

        for diagnostic in self.editor.check()? {
            let path = &diagnostic.location.module.source;

            let Some(document) = self.documents.get(path) else {
                continue;
            };

            let mut item = json!({"range": document.native_range(diagnostic.location.range),
                "severity": if diagnostic.error { DiagnosticSeverity::ERROR } else { DiagnosticSeverity::WARNING }, "source": "instar", "message": diagnostic.message});

            if let Some((related, message)) = diagnostic.related
                && let Ok(target) = self.document(&related.module.source)
            {
                item["relatedInformation"] = json!([{"location": {"uri": target.uri,
                    "range": target.native_range(related.range)}, "message": message}]);
            }

            self.diagnostics.entry(path.clone()).or_default().push(item);
        }

        for items in self.diagnostics.values_mut() {
            items.sort_by_key(Value::to_string);
            items.dedup();
        }

        self.revision = snapshot.revision;

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

    fn target(&mut self, name: &str, range: [u32; 4]) -> io::Result<Value> {
        let path = self.editor.source_path(name)?;
        let document = self.document(&path)?;

        Ok(json!({"uri": document.uri, "range": document.native_range(range)}))
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

    fn rename(&mut self, path: &Path, line: u32, column: u32, name: &str) -> io::Result<Value> {
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
    ) -> io::Result<Value> {
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

                edits.push(json!({"range": range, "newText": replacement}));
            }

            changes.push(
                json!({"textDocument": {"uri": document.uri, "version": version}, "edits": edits}),
            );
        }

        Ok(json!({"documentChanges": changes}))
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

    pub(crate) fn query(&mut self, path: &Path, query: &Query) -> io::Result<Value> {
        if matches!(query, Query::Implementation(..)) {
            self.index_workspace()?;
        }

        let document = self.document(path)?;

        match *query {
            Query::Hover(line, column) => self.hover(&document, line, column),
            Query::Completion(line, column) => self.completion(&document, line, column),

            Query::Hints(range, types, parameters, requires) => {
                self.hints(&document, range, types, parameters, requires)
            }

            Query::Actions(range) => self.actions(&document, range),

            Query::Signature(line, column) => {
                let result = self.editor.query(path, |checker, host, name| {
                    checker.signature_help(host, name, line, column)
                })?;

                let Some(result) = result else {
                    return Ok(Value::Null);
                };

                let mut signature = json!({"label": result.label, "parameters": result.parameters.into_iter().map(|label| json!({"label": label})).collect::<Vec<_>>()});

                let offset =
                    document.offset(document.native_range([line, column, line, column]).start)?;

                if let Some(callee) = document.features().callee(offset) {
                    let (line, column) = document.byte_position(document.position(callee))?;

                    if let Some(docs) = self.source_documentation(path, line, column)? {
                        signature["documentation"] = json!({"kind": "markdown", "value": docs});
                    }
                }

                Ok(
                    json!({"signatures": [signature], "activeSignature": 0, "activeParameter": result.active_parameter}),
                )
            }

            Query::Definition(line, column)
            | Query::Declaration(line, column)
            | Query::Implementation(line, column)
            | Query::TypeDefinition(line, column) => self.navigation(path, query, line, column),

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

                targets.sort_by_key(Value::to_string);
                targets.dedup();

                Ok(json!(targets))
            }

            Query::Prepare(line, column) => {
                let result = self.rename_target(path, line, column)?;

                Ok(result.map_or(Value::Null, |(_, target)| json!({"range": document.native_range(target.selection), "placeholder": target.name})))
            }

            Query::Rename(line, column, ref name) => self.rename(path, line, column, name),

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
                        highlights.push(
                            json!({"range": document.native_range(reference.range), "kind": DocumentHighlightKind::TEXT}),
                        );
                    }
                }

                Ok(json!(highlights))
            }
        }
    }

    fn navigation(
        &mut self,
        path: &Path,
        query: &Query,
        line: u32,
        column: u32,
    ) -> io::Result<Value> {
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

        targets.sort_by_key(Value::to_string);
        targets.dedup();

        Ok(json!(targets))
    }

    fn hover(&mut self, document: &Document, line: u32, column: u32) -> io::Result<Value> {
        let path = &document.path;

        let result = self.editor.query(path, |checker, host, name| {
            checker.hover(host, name, line, column)
        })?;

        let Some(result) = result else {
            return Ok(Value::Null);
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

        let mut hover = json!({"contents": {"kind": "markdown", "value": text}});

        if let Some(range) = result.range {
            hover["range"] = json!(document.native_range(range));
        }

        Ok(hover)
    }

    fn completion(&mut self, document: &Document, line: u32, column: u32) -> io::Result<Value> {
        let path = &document.path;

        let results = self.editor.query(path, |checker, host, name| {
            checker.completion(host, name, line, column)
        })?;

        let mut items = Vec::new();

        for result in &results {
            // The bridge completion enum is zero-based; LSP's corresponding enum starts at one.
            let mut item = json!({"label": result.name, "detail": result.detail,
                        "kind": result.kind as u32 + 1});

            let docs = match self.documentation(path, &result.documentation_symbol)? {
                Some(docs) => Some(docs),

                None => match &result.definition {
                    Some((module, range)) => self.declaration_documentation(module, *range)?,
                    None => None,
                },
            };

            if let Some(docs) = docs {
                item["documentation"] = json!({"kind": "markdown", "value": docs});
            }

            if result.deprecated {
                item["tags"] = json!([CompletionItemTag::DEPRECATED]);
            }

            let insert = if result.insert.is_empty() {
                &result.name
            } else {
                &result.insert
            };

            if let Some(range) = result.range {
                item["textEdit"] =
                    json!({"range": document.native_range(range), "newText": insert});
            } else {
                item["insertText"] = json!(insert);
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

        Ok(json!({"isIncomplete": incomplete, "items": items}))
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
    ) -> io::Result<Value> {
        if !types && !parameters {
            return Ok(json!([]));
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

            result.push(json!({"position": position, "label": label, "kind": if parameter { 2 } else { 1 }, "paddingRight": parameter}));
        }

        Ok(json!(result))
    }

    fn actions(
        &mut self,
        document: &Document,
        requested: tower_lsp_server::ls_types::Range,
    ) -> io::Result<Value> {
        let mut actions = Vec::new();

        for diagnostic in self
            .diagnostics
            .get(&document.path)
            .cloned()
            .unwrap_or_default()
        {
            let range: tower_lsp_server::ls_types::Range =
                serde_json::from_value(diagnostic["range"].clone())?;

            if range.end < requested.start
                || range.start > requested.end
                || !diagnostic["message"]
                    .as_str()
                    .is_some_and(|message| message.contains("Unknown global"))
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
                if item["label"].as_str() != Some(name) {
                    continue;
                }

                let mut edits = item["additionalTextEdits"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default();

                edits.push(item["textEdit"].clone());
                actions.push(json!({"title": format!("Import {name} from {}", item["detail"].as_str().unwrap_or("")), "kind": "quickfix", "diagnostics": [diagnostic], "edit": {"documentChanges": [{"textDocument": {"uri": document.uri, "version": document.version}, "edits": edits}]}}));
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

            actions.push(json!({"title": "Remove unused binding", "kind": "quickfix", "edit": {"documentChanges": [{"textDocument": {"uri": document.uri, "version": document.version}, "edits": [{"range": document.range(local.statement.start, local.statement.end), "newText": replacement}]}]}}));
        }

        Ok(json!(actions))
    }
}
