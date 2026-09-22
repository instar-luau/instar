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
    TypeDefinition(u32, u32),
    References(u32, u32, bool),
    Prepare(u32, u32),
    Rename(u32, u32, String),
    Highlights(u32, u32),
}

pub(crate) struct Worker {
    editor: Editor,
    documents: BTreeMap<PathBuf, Arc<Document>>,
    epoch: u64,
    folders: Vec<PathBuf>,
    indexed: bool,
    pub(crate) revision: u64,
    pub(crate) diagnostics: BTreeMap<PathBuf, Vec<Value>>,
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
        }
    }
}

impl Worker {
    pub(crate) fn update(&mut self, snapshot: &crate::Snapshot) -> io::Result<()> {
        if self.revision == snapshot.revision {
            return Ok(());
        }

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

        let mut directories = self.folders.clone();
        let mut paths = Vec::new();

        while let Some(directory) = directories.pop() {
            for entry in std::fs::read_dir(directory)? {
                let entry = entry?;
                let kind = entry.file_type()?;

                if kind.is_dir() {
                    directories.push(entry.path());
                } else if kind.is_file() {
                    let path = entry.path();

                    if matches!(
                        path.extension().and_then(|ext| ext.to_str()),
                        Some("lua" | "luau")
                    ) && !path
                        .file_name()
                        .is_some_and(|name| name.to_string_lossy().ends_with(".d.luau"))
                    {
                        paths.push(path);
                    }
                }
            }
        }

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
        let (definition, target) = self
            .rename_target(path, line, column)?
            .ok_or_else(|| io::Error::other("this symbol cannot be renamed"))?;

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

    fn documentation(&mut self, path: &Path, symbol: &str) -> Option<Value> {
        if symbol.is_empty() {
            return None;
        }

        let docs = self.editor.documentation(path).ok().flatten()?;
        let value = docs.get(symbol)?;

        let text = value
            .as_str()
            .or_else(|| value.get("documentation").and_then(Value::as_str))?;

        Some(json!({"kind": "markdown", "value": text}))
    }

    pub(crate) fn query(&mut self, path: &Path, query: &Query) -> io::Result<Value> {
        if matches!(
            query,
            Query::References(..) | Query::Prepare(..) | Query::Rename(..)
        ) {
            self.index_workspace()?;
        }

        let document = self.document(path)?;

        match *query {
            Query::Hover(line, column) => self.hover(&document, line, column),
            Query::Completion(line, column) => self.completion(&document, line, column),

            Query::Signature(line, column) => {
                let result = self.editor.query(path, |checker, host, name| {
                    checker.signature_help(host, name, line, column)
                })?;

                Ok(result.map_or(Value::Null, |result| json!({"signatures": [{"label": result.label,
                    "parameters": result.parameters.into_iter().map(|label| json!({"label": label})).collect::<Vec<_>>() }],
                    "activeSignature": 0, "activeParameter": result.active_parameter})))
            }

            Query::Definition(line, column) | Query::TypeDefinition(line, column) => {
                let results = self.editor.query(path, |checker, host, name| {
                    if matches!(query, Query::Definition(..)) {
                        checker.definition(host, name, line, column)
                    } else {
                        checker.type_definition(host, name, line, column)
                    }
                })?;

                let mut targets = Vec::new();

                for result in results {
                    if let Ok(target) = self.target(&result.path, result.selection) {
                        targets.push(target);
                    }
                }

                Ok(json!(targets))
            }

            Query::References(line, column, include_declaration) => {
                let results = self.editor.query(path, |checker, host, name| {
                    checker.references(host, name, line, column)
                })?;

                let mut targets = Vec::new();

                for result in results {
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
                    checker.references(host, name, line, column)
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

    fn hover(&mut self, document: &Document, line: u32, column: u32) -> io::Result<Value> {
        let path = &document.path;

        let result = self.editor.query(path, |checker, host, name| {
            checker.hover(host, name, line, column)
        })?;

        let Some(result) = result else {
            return Ok(Value::Null);
        };

        let mut text = format!("```luau\n{}: {}\n```", result.name, result.type_);

        if let Some(docs) = self.documentation(path, &result.documentation_symbol)
            && let Some(docs) = docs["value"].as_str()
        {
            text.push_str("\n\n");
            text.push_str(docs);
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

        for result in results {
            // The bridge completion enum is zero-based; LSP's corresponding enum starts at one.
            let mut item = json!({"label": result.name, "detail": result.detail,
                        "kind": result.kind as u32 + 1});

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

        Ok(json!({"isIncomplete": false, "items": items}))
    }
}
