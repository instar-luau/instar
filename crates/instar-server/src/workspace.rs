use std::{
    collections::{BTreeMap, BTreeSet},
    hash::{Hash, Hasher},
    io,
    path::{Path, PathBuf},
    sync::Arc,
};

use instar_core::{filter::Service, project::Project, resolve::Resolver};
use serde_json::{Value, json};
use tower_lsp_server::ls_types::{CompletionItemKind, Range};

use crate::{
    Snapshot,
    document::{Document, uri},
    features::binding_name,
};

#[derive(Default)]
pub(crate) struct Workspace {
    pub(crate) project: Project,
    documents: BTreeMap<PathBuf, Arc<Document>>,
    snapshot: Snapshot,
    files: Option<Vec<PathBuf>>,
    syntax_candidates: BTreeMap<PathBuf, SyntaxCandidates>,
    resolver: Resolver,
    matcher: nucleo_matcher::Matcher,
}

#[derive(Default)]
struct SyntaxCandidates {
    names: BTreeSet<String>,
    strings: Vec<String>,
    decoded_strings: Vec<String>,
    uncertain_string: bool,
    dynamic_bracket: bool,
}

impl Workspace {
    pub(crate) fn update(&mut self, snapshot: &Snapshot) -> io::Result<()> {
        let reset =
            self.snapshot.epoch != snapshot.epoch || self.snapshot.folders != snapshot.folders;

        if reset {
            self.project = Project::default();
            self.documents.clear();
            self.files = None;
            self.syntax_candidates.clear();
        }

        for path in self
            .snapshot
            .documents
            .keys()
            .filter(|path| !snapshot.documents.contains_key(*path))
        {
            self.project.set_source(path, None)?;
            self.syntax_candidates.remove(path);
            self.documents.remove(path);
        }

        for (path, document) in &snapshot.documents {
            if reset
                || self
                    .snapshot
                    .documents
                    .get(path)
                    .is_none_or(|old| !Arc::ptr_eq(old, document))
            {
                self.project.set_source(path, Some(&document.text))?;
                self.syntax_candidates.remove(path);
                self.documents.insert(path.clone(), Arc::clone(document));
            }
        }

        self.resolver = Resolver::new();
        self.snapshot = snapshot.clone();

        Ok(())
    }

    pub(crate) fn files(&mut self, cancelled: &impl Fn() -> bool) -> io::Result<Vec<PathBuf>> {
        if self.files.is_none() {
            let mut directories: Vec<_> = self.snapshot.folders.iter().cloned().collect();
            let mut paths = BTreeSet::new();

            while let Some(directory) = directories.pop() {
                check_cancelled(cancelled)?;

                for entry in std::fs::read_dir(directory)? {
                    let entry = entry?;
                    let kind = entry.file_type()?;

                    if kind.is_dir() && entry.file_name() != ".git" {
                        directories.push(entry.path());
                    } else if kind.is_file() && self.include(&entry.path())? {
                        paths.insert(entry.path());
                    }
                }
            }

            self.files = Some(paths.into_iter().collect());
        }

        let mut files: BTreeSet<_> = self.files.iter().flatten().cloned().collect();

        for path in self.snapshot.documents.keys() {
            files.insert(path.clone());
        }

        Ok(files.into_iter().collect())
    }

    pub(crate) fn syntax_candidates(
        &mut self,
        names: &[&str],
        property: bool,
        cancelled: &impl Fn() -> bool,
    ) -> io::Result<Vec<PathBuf>> {
        let mut candidates = Vec::new();

        for path in self.files(cancelled)? {
            check_cancelled(cancelled)?;

            if !self.syntax_candidates.contains_key(&path) {
                let document = self.document(&path)?;
                let source = document.text.as_bytes();
                let mut index = SyntaxCandidates::default();

                for token in vermis::Lexer::new(source) {
                    match token.kind {
                        vermis::TokenKind::Name => {
                            if let Ok(name) = token.utf8(source) {
                                index.names.insert(name.to_owned());
                            }
                        }

                        vermis::TokenKind::QuotedString
                        | vermis::TokenKind::RawString
                        | vermis::TokenKind::Error(vermis::LexError::BrokenString) => {
                            index
                                .strings
                                .push(String::from_utf8_lossy(token.bytes(source)).into_owned());

                            match instar_core::string_value(token.bytes(source)) {
                                Ok(value) => index.decoded_strings.push(value),
                                Err(_) => index.uncertain_string = true,
                            }
                        }

                        vermis::TokenKind::Byte(b'[') => index.dynamic_bracket = true,
                        _ => {}
                    }
                }

                self.syntax_candidates.insert(path.clone(), index);
            }

            let index = &self.syntax_candidates[&path];

            if (property && index.dynamic_bracket)
                || index.uncertain_string
                || names.iter().any(|name| {
                    index.names.contains(*name)
                        || index.strings.iter().any(|value| value.contains(name))
                        || index
                            .decoded_strings
                            .iter()
                            .any(|value| value.contains(name))
                })
            {
                candidates.push(path);
            }
        }

        Ok(candidates)
    }

    fn include(&mut self, path: &Path) -> io::Result<bool> {
        if !matches!(
            path.extension().and_then(|value| value.to_str()),
            Some("lua" | "luau")
        ) || path
            .file_name()
            .is_some_and(|name| name.to_string_lossy().ends_with(".d.luau"))
            || !self.project.includes(path, Service::Lsp)?
        {
            return Ok(false);
        }

        Ok(!self
            .project
            .configuration(path)?
            .settings
            .definitions
            .values()
            .any(|value| Path::new(value) == path))
    }

    pub(crate) fn document(&mut self, path: &Path) -> io::Result<Arc<Document>> {
        if let Some(document) = self.documents.get(path) {
            return Ok(Arc::clone(document));
        }

        let document = Arc::new(Document::new(
            uri(path)?,
            0,
            self.project.source(path)?.to_string(),
        )?);

        self.documents
            .insert(path.to_owned(), Arc::clone(&document));

        Ok(document)
    }

    pub(crate) fn symbols(
        &mut self,
        query: &str,
        cancelled: &impl Fn() -> bool,
    ) -> io::Result<Value> {
        let mut result = Vec::new();
        let query = query.to_lowercase();

        for path in self.files(cancelled)? {
            check_cancelled(cancelled)?;
            let document = self.document(&path)?;

            if let Some(symbols) = document.symbols().as_array() {
                for symbol in symbols {
                    if symbol["name"]
                        .as_str()
                        .is_some_and(|name| name.to_lowercase().contains(&query))
                    {
                        result.push(json!({"name": symbol["name"], "kind": symbol["kind"], "location": {"uri": document.uri, "range": symbol["selectionRange"]}}));
                    }
                }
            }
        }

        Ok(json!(result))
    }

    pub(crate) fn imports(
        &mut self,
        document: &Document,
        site: (usize, &str, Range),
        services: &[String],
        occupied: &impl Fn(&str) -> bool,
    ) -> io::Result<Vec<Value>> {
        let (offset, prefix, range) = site;
        let mut items = Vec::new();
        self.service_imports(document, site, &mut items, services, occupied)?;

        if document.features().names.contains("require") {
            return Ok(items);
        }

        let links = self.project.links(&document.path)?;

        for target in self.files(&|| false)? {
            if target == document.path {
                continue;
            }

            let target_document = self.document(&target)?;

            let stem = target
                .file_stem()
                .and_then(|value| value.to_str())
                .unwrap_or("");

            let stem = if stem == "init" {
                target
                    .parent()
                    .and_then(Path::file_name)
                    .and_then(|name| name.to_str())
                    .unwrap_or(stem)
            } else {
                stem
            };

            let Some(module) = binding_name(stem) else {
                continue;
            };

            let exports = &target_document.features().exports;

            if !module.starts_with(prefix) && !exports.iter().any(|name| name.starts_with(prefix)) {
                continue;
            }

            let existing = document.features().dependencies.iter().find(|dependency| {
                dependency.end < offset
                    && dependency.service.is_none()
                    && links
                        .iter()
                        .any(|(span, path)| span[0] == dependency.argument.start && *path == target)
                    && unique_binding(document, &dependency.name)
            });

            let (binding, statement) = if let Some(existing) = existing {
                (existing.name.clone(), String::new())
            } else {
                let Some(argument) = self
                    .resolver
                    .import_argument(&mut self.project, &document.path, &target)
                    .map_err(io::Error::other)?
                else {
                    continue;
                };

                if argument.starts_with("game") && document.features().names.contains("game") {
                    continue;
                }

                let binding = fresh_name(document, &module, occupied);
                let statement = format!("local {binding} = require({argument})\n");

                (binding, statement)
            };

            if module.starts_with(prefix) {
                items.push(import_item(
                    document,
                    (offset, range),
                    &module,
                    &binding,
                    &statement,
                    &target.to_string_lossy(),
                    CompletionItemKind::MODULE,
                ));
            }

            for export in exports.iter().filter(|name| name.starts_with(prefix)) {
                if binding_name(export).is_some() {
                    items.push(import_item(
                        document,
                        (offset, range),
                        export,
                        &format!("{binding}.{export}"),
                        &statement,
                        &target.to_string_lossy(),
                        CompletionItemKind::FIELD,
                    ));
                }
            }
        }

        Ok(items)
    }

    fn service_imports(
        &mut self,
        document: &Document,
        site: (usize, &str, Range),
        items: &mut Vec<Value>,
        services: &[String],
        occupied: &impl Fn(&str) -> bool,
    ) -> io::Result<()> {
        let (offset, prefix, range) = site;

        let start = document.offset(range.start)?;
        let end = document.offset(range.end)?;

        let local_binding = document.bindings().tokens().any(|(span, _, modifiers)| {
            span.start == start && span.end == end && modifiers & 2 != 0
        });

        let declaration = document.features().locals.iter().any(|local| {
            local.name.start == start
                && local.name.end == end
                && document.text[local.name.end..local.statement.end]
                    .trim()
                    .is_empty()
        });

        if local_binding && !declaration {
            return Ok(());
        }

        let pattern = nucleo_matcher::pattern::Atom::new(
            prefix,
            nucleo_matcher::pattern::CaseMatching::Ignore,
            nucleo_matcher::pattern::Normalization::Smart,
            nucleo_matcher::pattern::AtomKind::Fuzzy,
            false,
        );

        for (service, score) in pattern.match_list(services, &mut self.matcher) {
            let existing = document.features().dependencies.iter().find(|dependency| {
                dependency.service.as_ref() == Some(service)
                    && dependency.end < offset
                    && unique_binding(document, &dependency.name)
            });

            if declaration && document.features().names.contains("game") {
                continue;
            }

            let (insertion, statement) = if declaration {
                let argument = serde_json::to_string(service)?;

                (
                    format!("{service} = game:GetService({argument})"),
                    String::new(),
                )
            } else if let Some(existing) = existing {
                (existing.name.clone(), String::new())
            } else {
                if document.features().names.contains("game") {
                    continue;
                }

                let argument = serde_json::to_string(service)?;
                let name = fresh_name(document, service, occupied);
                let statement = format!("local {name} = game:GetService({argument})\n");

                (name, statement)
            };

            let mut item = import_item(
                document,
                (offset, range),
                service,
                &insertion,
                "",
                "Roblox service",
                CompletionItemKind::CLASS,
            );

            if !statement.is_empty() {
                let edit = document
                    .features()
                    .service_insertion(document, offset, service, &statement);

                insert_import(document, &mut item, range, &insertion, edit);
            }

            item["filterText"] = json!(format!("{prefix} {service}"));
            item["sortText"] = json!(format!("z{:05}{service}", u16::MAX - score));
            items.push(item);
        }

        Ok(())
    }

    pub(crate) fn rename_files(
        &mut self,
        renames: &[(PathBuf, PathBuf)],
        cancelled: &impl Fn() -> bool,
    ) -> io::Result<Value> {
        let mut changes = Vec::new();

        for source in self.files(cancelled)? {
            check_cancelled(cancelled)?;
            let document = self.document(&source)?;
            let new_source = remap(&source, renames);

            let entries = self
                .resolver
                .entries(&mut self.project, &source)
                .map_err(io::Error::other)?;

            if entries.iter().any(|entry| entry.instance.is_some()) {
                continue;
            }

            let config = self.project.configuration(&source)?;
            let mut edits = Vec::new();

            for (span, target) in self.project.links(&source)? {
                let raw = &document.text[span[0]..span[1]];

                let Ok(request) = instar_core::string_value(raw.as_bytes()) else {
                    continue;
                };

                let new_target = remap(&target, renames);

                if new_source == source && new_target == target {
                    continue;
                }

                let from = module_path(&new_source);
                let to = module_path(&new_target);

                let base = from
                    .parent()
                    .ok_or_else(|| io::Error::other("module has no parent"))?;

                let mut replacement = relative(base, &to).map(|value| {
                    if value.starts_with("../") {
                        value
                    } else {
                        format!("./{value}")
                    }
                });

                if let Some(alias) = request
                    .strip_prefix('@')
                    .and_then(|value| value.split('/').next())
                    && let Some(alias_config) = config.aliases.get(&alias.to_ascii_lowercase())
                    && !alias_config.target.starts_with('@')
                {
                    let manifest = remap(&alias_config.defined_in, renames);

                    let root = instar_core::absolute(
                        &manifest.parent().unwrap_or(base).join(&alias_config.target),
                    )?;

                    if let Ok(suffix) = to.strip_prefix(root) {
                        replacement = Some(
                            format!("@{alias}/{}", suffix.to_string_lossy().replace('\\', "/"))
                                .trim_end_matches('/')
                                .to_owned(),
                        );
                    }
                }

                if let Some(replacement) = replacement {
                    edits.push(json!({"range": document.range(span[0], span[1]), "newText": quote(&replacement, raw)?}));
                }
            }

            if !edits.is_empty() {
                changes.push(json!({"textDocument": {"uri": document.uri, "version": self.snapshot.documents.get(&source).map(|document| document.version)}, "edits": edits}));
            }
        }

        Ok(json!({"documentChanges": changes}))
    }
}

fn unique_binding(document: &Document, name: &str) -> bool {
    document
        .bindings()
        .tokens()
        .filter(|(span, _, modifiers)| {
            modifiers & 2 != 0 && &document.text[span.start..span.end] == name
        })
        .count()
        == 1
}

fn import_item(
    document: &Document,
    site: (usize, Range),
    label: &str,
    insertion: &str,
    statement: &str,
    detail: &str,
    kind: CompletionItemKind,
) -> Value {
    let (offset, range) = site;
    let mut item = json!({"label": label, "kind": kind, "detail": detail, "sortText": format!("z{label}"), "textEdit": {"range": range, "newText": insertion}});

    if !statement.is_empty() {
        let (position, prefix) = document.features().insertion(document, offset);

        insert_import(
            document,
            &mut item,
            range,
            insertion,
            (position, format!("{prefix}{statement}")),
        );
    }

    item
}

fn insert_import(
    document: &Document,
    item: &mut Value,
    range: Range,
    insertion: &str,
    edit: (usize, String),
) {
    let (position, text) = edit;

    if document.position(position) == range.start {
        item["textEdit"]["newText"] = json!(format!("{text}{insertion}"));
    } else {
        item["additionalTextEdits"] =
            json!([{"range": document.range(position, position), "newText": text}]);
    }
}

pub(crate) fn remap(path: &Path, renames: &[(PathBuf, PathBuf)]) -> PathBuf {
    renames
        .iter()
        .filter_map(|(old, new)| {
            path.strip_prefix(old)
                .ok()
                .map(|suffix| (old.components().count(), new.join(suffix)))
        })
        .max_by_key(|(depth, _)| *depth)
        .map_or_else(|| path.to_owned(), |(_, path)| path)
}

fn module_path(path: &Path) -> PathBuf {
    if matches!(
        path.file_name().and_then(|name| name.to_str()),
        Some("init.lua" | "init.luau")
    ) {
        path.parent().unwrap_or(path).to_owned()
    } else {
        path.with_extension("")
    }
}

fn relative(base: &Path, target: &Path) -> Option<String> {
    let base: Vec<_> = base.components().collect();
    let target: Vec<_> = target.components().collect();

    if base.first() != target.first() {
        return None;
    }

    let common = base.iter().zip(&target).take_while(|(a, b)| a == b).count();

    Some(
        std::iter::repeat_n("..".to_owned(), base.len() - common)
            .chain(
                target[common..]
                    .iter()
                    .map(|part| part.as_os_str().to_string_lossy().into_owned()),
            )
            .collect::<Vec<_>>()
            .join("/"),
    )
}

fn quote(value: &str, original: &str) -> io::Result<String> {
    let encoded = serde_json::to_string(value)?;

    if original.starts_with('\'') {
        Ok(format!(
            "'{}'",
            encoded[1..encoded.len() - 1].replace('\'', "\\'")
        ))
    } else {
        Ok(encoded)
    }
}

pub(crate) fn check_cancelled(cancelled: &impl Fn() -> bool) -> io::Result<()> {
    if cancelled() {
        Err(io::Error::new(
            io::ErrorKind::Interrupted,
            "request cancelled",
        ))
    } else {
        Ok(())
    }
}

pub(crate) enum Request {
    Symbols(String),
    Rename(Vec<(PathBuf, PathBuf)>),
    Diagnostics(Vec<(String, String)>),
    Document(PathBuf, Option<String>),
}

pub(crate) struct Command {
    pub(crate) snapshot: Snapshot,
    pub(crate) request: Request,
    pub(crate) cancelled: Arc<std::sync::atomic::AtomicBool>,
    pub(crate) progress: tokio::sync::mpsc::UnboundedSender<(usize, usize)>,
    pub(crate) reply: tokio::sync::oneshot::Sender<io::Result<Value>>,
}

pub(crate) fn start() -> (
    std::sync::mpsc::Sender<Option<Command>>,
    std::thread::JoinHandle<()>,
) {
    let (sender, receiver) = std::sync::mpsc::channel::<Option<Command>>();

    let thread = std::thread::spawn(move || {
        let mut workspace = Workspace::default();
        let mut editor = instar_core::analysis::Editor::default();

        while let Ok(Some(command)) = receiver.recv() {
            let cancelled = || {
                command.reply.is_closed()
                    || command.cancelled.load(std::sync::atomic::Ordering::Acquire)
            };

            if command.reply.is_closed() {
                continue;
            }

            let result = (|| {
                check_cancelled(&cancelled)?;

                if workspace.snapshot.epoch != command.snapshot.epoch {
                    editor.refresh();
                }

                for path in workspace
                    .snapshot
                    .documents
                    .keys()
                    .filter(|path| !command.snapshot.documents.contains_key(*path))
                {
                    editor.set_source(path, None)?;
                }

                for (path, document) in &command.snapshot.documents {
                    if workspace
                        .snapshot
                        .documents
                        .get(path)
                        .is_none_or(|old| !Arc::ptr_eq(old, document))
                    {
                        editor.set_source(path, Some(&document.text))?;
                    }
                }

                workspace.update(&command.snapshot)?;

                match command.request {
                    Request::Symbols(query) => workspace.symbols(&query, &cancelled),
                    Request::Rename(renames) => workspace.rename_files(&renames, &cancelled),

                    Request::Diagnostics(previous) => workspace.diagnostics(
                        &mut editor,
                        &previous,
                        None,
                        &cancelled,
                        &command.progress,
                    ),

                    Request::Document(path, previous) => {
                        let document_uri = uri(&path)?.to_string();

                        let previous: Vec<_> = previous
                            .map(|id| (document_uri.clone(), id))
                            .into_iter()
                            .collect();

                        let result = workspace.diagnostics(
                            &mut editor,
                            &previous,
                            Some(path),
                            &cancelled,
                            &command.progress,
                        )?;

                        Ok(result["items"]
                            .as_array()
                            .and_then(|items| {
                                items
                                    .iter()
                                    .find(|item| item["uri"].as_str() == Some(&document_uri))
                            })
                            .cloned()
                            .unwrap_or_else(|| json!({"kind": "full", "items": []})))
                    }
                }
            })();

            drop(command.reply.send(result));
        }
    });

    (sender, thread)
}

impl Workspace {
    fn diagnostics(
        &mut self,
        editor: &mut instar_core::analysis::Editor,
        previous: &[(String, String)],
        only: Option<PathBuf>,
        cancelled: &impl Fn() -> bool,
        progress: &tokio::sync::mpsc::UnboundedSender<(usize, usize)>,
    ) -> io::Result<Value> {
        let files = match only {
            Some(path) => vec![path],
            None => self.files(cancelled)?,
        };

        let diagnostics = editor.check_workspace(&files, &mut |done, total| {
            if cancelled() {
                return false;
            }

            progress.send((done, total)).is_ok()
        })?;

        let mut reports: BTreeMap<PathBuf, Vec<Value>> = files
            .iter()
            .cloned()
            .map(|path| (path, Vec::new()))
            .collect();

        for diagnostic in diagnostics {
            check_cancelled(cancelled)?;
            let path = diagnostic.location.module.source;
            let document = self.document(&path)?;
            let mut item = json!({"range": document.native_range(diagnostic.location.range), "severity": if diagnostic.error { 1 } else { 2 }, "source": "instar", "message": diagnostic.message});

            if let Some((location, message)) = diagnostic.related {
                let target = self.document(&location.module.source)?;
                item["relatedInformation"] = json!([{"location": {"uri": target.uri, "range": target.native_range(location.range)}, "message": message}]);
            }

            reports.entry(path).or_default().push(item);
        }

        let mut items = Vec::new();

        for (path, mut diagnostics) in reports {
            check_cancelled(cancelled)?;
            diagnostics.sort_by_key(Value::to_string);
            diagnostics.dedup();
            let uri = uri(&path)?;
            let encoded = serde_json::to_string(&diagnostics)?;
            let mut hash = std::collections::hash_map::DefaultHasher::new();
            encoded.hash(&mut hash);
            let result_id = format!("{:x}", hash.finish());
            let mut report = json!({"uri": uri, "version": self.snapshot.documents.get(&path).map(|document| document.version), "resultId": result_id});

            if previous
                .iter()
                .any(|(old_uri, old_id)| old_uri == uri.as_str() && *old_id == result_id)
            {
                report["kind"] = json!("unchanged");
            } else {
                report["kind"] = json!("full");
                report["items"] = json!(diagnostics);
            }

            items.push(report);
        }

        for (old_uri, _) in previous {
            if !items
                .iter()
                .any(|item| item["uri"].as_str() == Some(old_uri))
            {
                items.push(json!({"uri": old_uri, "version": null, "kind": "full", "items": []}));
            }
        }

        Ok(json!({"items": items}))
    }
}

pub(crate) fn import_site(document: &Document, offset: usize) -> Option<(usize, &str, Range)> {
    let bytes = document.text.as_bytes();
    let mut start = offset;

    while start > 0 && (bytes[start - 1].is_ascii_alphanumeric() || bytes[start - 1] == b'_') {
        start -= 1;
    }

    let prefix = &document.text[start..offset];

    if prefix.is_empty()
        || document.text[..start].trim_end().ends_with(['.', ':'])
        || document.bindings().tokens().any(|(span, kind, _)| {
            span.start <= offset
                && offset < span.end
                && matches!(kind, crate::bindings::SemanticKind::Comment)
        })
    {
        return None;
    }

    let mut end = offset;

    while end < bytes.len() && (bytes[end].is_ascii_alphanumeric() || bytes[end] == b'_') {
        end += 1;
    }

    Some((offset, prefix, document.range(start, end)))
}

fn fresh_name(document: &Document, name: &str, occupied: &impl Fn(&str) -> bool) -> String {
    let mut candidate = name.to_owned();
    let mut suffix = 2;

    while document.features().names.contains(&candidate) || occupied(&candidate) {
        candidate = format!("{name}{suffix}");
        suffix += 1;
    }

    candidate
}
