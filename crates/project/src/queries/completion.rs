use std::{
    collections::BTreeSet,
    io,
    path::{Path, PathBuf},
    sync::Arc,
};

use instar_luau::{
    CompletionCandidate, CompletionContext, CompletionKind, ImportIdentity, ImportRequest,
    ImportSite, Notation, Parentheses,
};

use instar_shared::{Completion, Location, OperationFailure, Options, Span, TextEdit};
use instar_syntax::{bindings, identifier, literal, parse, token::TokenKind, tree::Tree};

use super::{
    output::{ProjectCandidate, ProjectCompletions},
    request::CompletionQuery,
};

use crate::{
    capture::{Capture, Listing},
    configuration::{BindingStyle, CheckOverrides, OperationOverrides, RequireStyle},
    execution::Runtime,
    graph::{DependencyGraph, ModuleNode, syntax_settings},
    inputs::{SourceOrigin, normalize},
    request::Invocation,
    resolution::module_path,
    selection::{Matcher, SelectedSource},
    sourcemap::Map,
};

struct AutomaticImports {
    prefix: String,
    occupied: BTreeSet<String>,
    insertion: usize,
    parent: PathBuf,
    ending: &'static str,
}

struct ImportCandidate {
    path: PathBuf,
    name: String,
    request: String,
}

impl Runtime {
    pub(crate) fn complete_imports(
        &mut self,
        request: &CompletionQuery,
        graph: &DependencyGraph,
        capture: &mut Capture,
        completions: &mut ProjectCompletions,
        failures: &mut Vec<OperationFailure>,
        options: &Options,
    ) -> io::Result<()> {
        let node = graph
            .modules
            .get(&completions.native.analysis.module)
            .ok_or_else(|| invalid("completion module is absent"))?;

        if completions.native.require
            && let Some(prefix) = &completions.native.prefix
        {
            for name in Self::import_names(node, prefix, capture, options)? {
                completions.candidates.push(ProjectCandidate {
                    entry: candidate(name, CompletionKind::Path, "Import path".to_owned()),
                    edits: Vec::new(),
                });
            }
        }

        if request.inline
            || !node.effective.settings.editing.imports_enabled
            || !completions.native.loader
            || !matches!(
                completions.native.context,
                CompletionContext::Expression | CompletionContext::Statement
            )
            || capture.snapshot.roots().next().is_none()
        {
            order(completions);

            return Ok(());
        }

        if let Some(mut imports) = automatic_imports(node, request, capture, completions, options)?
        {
            self.complete_candidates(node, &mut imports, capture, completions, failures, options)?;
        }

        order(completions);

        Ok(())
    }

    fn import_names(
        node: &ModuleNode,
        prefix: &str,
        capture: &mut Capture,
        options: &Options,
    ) -> io::Result<Vec<String>> {
        let prefix = prefix.replace('\\', "/");
        let mut names = BTreeSet::new();

        for name in ["./", "../"] {
            if name.starts_with(&prefix) {
                names.insert(name.to_owned());
            }
        }

        if !prefix.contains('/') {
            for alias in node
                .effective
                .settings
                .resolution
                .aliases
                .keys()
                .map(String::as_str)
                .chain(["self"])
            {
                let name = format!("@{alias}/");

                if name.starts_with(&prefix) {
                    names.insert(name);
                }
            }
        }

        if node.effective.settings.environment.roblox
            && (node.map.is_some() || node.effective.settings.resolution.sourcemaps.len() == 1)
            && "@game/".starts_with(&prefix)
        {
            names.insert("@game/".to_owned());
        }

        let base = if prefix.is_empty() {
            Some("./")
        } else {
            prefix.rfind('/').map(|index| &prefix[..=index])
        };

        if let Some(base) = base {
            if let Some(path) = base.strip_prefix("@game/") {
                mapped_import_names(node, path, base, &prefix, capture, &mut names, options)?;
            } else {
                Self::filesystem_import_names(node, base, &prefix, capture, &mut names, options)?;
            }
        }

        Ok(names.into_iter().collect())
    }

    fn filesystem_import_names(
        node: &ModuleNode,
        base: &str,
        prefix: &str,
        capture: &mut Capture,
        names: &mut BTreeSet<String>,
        options: &Options,
    ) -> io::Result<()> {
        let Some(path) = capture
            .snapshot
            .origin(node.source.version.identity)
            .and_then(SourceOrigin::path)
        else {
            return Ok(());
        };

        let directory = if let Some(alias) = base.strip_prefix('@') {
            let (name, suffix) = alias.split_once('/').unwrap_or((alias, ""));

            normalize(
                &node
                    .effective
                    .settings
                    .resolution
                    .alias(name, &module_path(path))?
                    .join(suffix),
            )
        } else {
            normalize(
                &module_path(path)
                    .parent()
                    .ok_or_else(|| invalid("module has no parent"))?
                    .join(base),
            )
        };

        if let Listing::Directory(entries) = capture.listing(&directory, options)? {
            for path in entries {
                options.check()?;

                let name = if matches!(
                    path.extension().and_then(|extension| extension.to_str()),
                    Some("lua" | "luau")
                ) {
                    path.file_stem()
                } else {
                    path.file_name()
                };

                let Some(name) = name.and_then(|name| name.to_str()) else {
                    continue;
                };

                if matches!(name, "instar" | "init") {
                    continue;
                }

                let name = format!("{base}{name}");

                if !name.starts_with(prefix) {
                    continue;
                }

                let site = ImportSite {
                    identity: ImportIdentity(0),
                    call: Location::new(&node.source, Span { start: 0, end: 0 })?,
                    argument: Location::new(&node.source, Span { start: 0, end: 0 })?,
                    request: ImportRequest::Static(name.clone()),
                };

                match Self::resolve(node, &site, capture, options) {
                    Ok(Some(_)) => {
                        names.insert(name);
                    }

                    Err(error) if unavailable(&error) => {
                        if matches!(capture.listing(&path, options)?, Listing::Directory(_)) {
                            names.insert(format!("{name}/"));
                        }
                    }

                    Err(error) => return Err(error),
                    Ok(None) => {}
                }
            }
        }

        Ok(())
    }

    fn complete_candidates(
        &mut self,
        node: &ModuleNode,
        imports: &mut AutomaticImports,
        capture: &mut Capture,
        completions: &mut ProjectCompletions,
        failures: &mut Vec<OperationFailure>,
        options: &Options,
    ) -> io::Result<()> {
        let selected = self.editor_targets(None, true, capture, None, options)?;

        for target in &selected {
            for group in &target.effective.settings.environment.groups {
                for path in &group.definitions {
                    if let Some(source) = capture.read(path, options)? {
                        capture.declaration(&source)?;
                    }
                }

                for path in &group.documentation {
                    capture.read(path, options)?;
                }
            }
        }

        capture.admit_inventory(options)?;
        capture.consult_inventory();

        for target in selected {
            options.check()?;

            if target.identity == node.source.version.identity {
                continue;
            }

            let Some(proposed) = automatic_import_candidate(&target, imports, capture)? else {
                continue;
            };

            let site = ImportSite {
                identity: ImportIdentity(0),
                call: completions.native.replacement,
                argument: completions.native.replacement,
                request: ImportRequest::Static(proposed.request.clone()),
            };

            match Self::resolve(node, &site, capture, options) {
                Ok(Some(resolved)) if resolved.path == proposed.path => {}
                Ok(_) => continue,
                Err(error) if unavailable(&error) => continue,
                Err(error) => return Err(error),
            }

            let reachable = self.graph(
                &[SelectedSource {
                    identity: target.identity,
                    effective: target.effective.clone(),
                }],
                &Invocation::default(),
                &OperationOverrides::Check(CheckOverrides::default()),
                capture,
                true,
                options,
            )?;

            failures.extend(
                reachable
                    .modules
                    .values()
                    .flat_map(|node| node.environment.failures.iter().cloned()),
            );

            completions.native.diagnostics.extend(reachable.diagnostics);
            let available = reachable.completion == Completion::Complete;

            completions.native.completion = completions
                .native
                .completion
                .clone()
                .combine(reachable.completion);

            if !available
                || reachable
                    .modules
                    .keys()
                    .any(|module| module.source == node.source.version.identity)
            {
                continue;
            }

            let Some(request_text) = import_expression(node, &proposed) else {
                continue;
            };

            let keyword = if matches!(
                node.effective.settings.editing.import_binding,
                BindingStyle::Const
            ) {
                "const"
            } else {
                "local"
            };

            let edit = TextEdit {
                location: Location::new(
                    &node.source,
                    Span {
                        start: imports.insertion,
                        end: imports.insertion,
                    },
                )?,
                replacement: format!(
                    "{keyword} {} = require({request_text}){}",
                    proposed.name, imports.ending
                ),
            };

            completions.candidates.push(ProjectCandidate {
                entry: candidate(
                    proposed.name.clone(),
                    CompletionKind::Module,
                    format!("Auto import from {}", proposed.request),
                ),
                edits: vec![edit],
            });

            imports.occupied.insert(proposed.name);
        }

        Ok(())
    }
}

fn automatic_imports(
    node: &ModuleNode,
    request: &CompletionQuery,
    capture: &Capture,
    completions: &ProjectCompletions,
    options: &Options,
) -> io::Result<Option<AutomaticImports>> {
    let source = &node.source;

    let prefix = source
        .text
        .get(completions.native.replacement.span.start..request.position.offset)
        .ok_or_else(|| invalid("completion prefix is outside source"))?
        .to_ascii_lowercase();

    if prefix.is_empty() {
        return Ok(None);
    }

    let parsed = parse(source, &syntax_settings(&node.effective), options);
    let bindings = bindings(&parsed.tree, options);
    let tree = parsed.tree.syntax();

    if bindings.global_assignments.contains("require")
        || bindings
            .declarations
            .values()
            .any(|binding| tree.text(binding.name) == b"require")
    {
        return Ok(None);
    }

    let mut occupied = completions
        .candidates
        .iter()
        .map(|candidate| candidate.entry.name.clone())
        .collect::<BTreeSet<_>>();

    for binding in bindings.declarations.values() {
        occupied.insert(String::from_utf8_lossy(tree.text(binding.name)).into_owned());
    }

    occupied.extend(bindings.global_assignments);
    let insertion = prologue(&source.text, tree, options)?;

    if insertion >= completions.native.replacement.span.start {
        return Ok(None);
    }

    let path = capture
        .snapshot
        .origin(source.version.identity)
        .and_then(SourceOrigin::path)
        .ok_or_else(|| invalid("automatic import has no source context"))?;

    let logical = module_path(path);

    let parent = logical
        .parent()
        .ok_or_else(|| invalid("module has no parent"))?
        .to_path_buf();

    let ending = if source
        .text
        .find('\n')
        .is_some_and(|index| index > 0 && source.text.as_bytes()[index - 1] == b'\r')
    {
        "\r\n"
    } else {
        "\n"
    };

    Ok(Some(AutomaticImports {
        prefix,
        occupied,
        insertion,
        parent,
        ending,
    }))
}

fn automatic_import_candidate(
    target: &SelectedSource,
    imports: &AutomaticImports,
    capture: &Capture,
) -> io::Result<Option<ImportCandidate>> {
    let Some(path) = capture
        .snapshot
        .origin(target.identity)
        .and_then(SourceOrigin::path)
        .map(Path::to_path_buf)
    else {
        return Ok(None);
    };

    let policy = &target.effective.settings.selection.imports;

    if !Matcher::new(
        policy
            .include
            .as_deref()
            .or(target.effective.settings.selection.include.as_deref()),
    )?
    .includes(&path)
        || Matcher::new(
            policy
                .exclude
                .as_deref()
                .or(target.effective.settings.selection.exclude.as_deref()),
        )?
        .matches(&path)
            && !policy.override_exclusions
    {
        return Ok(None);
    }

    let logical = module_path(&path);

    let Some(name) = logical.file_name().and_then(|name| name.to_str()) else {
        return Ok(None);
    };

    if !name.to_ascii_lowercase().starts_with(&imports.prefix)
        || imports.occupied.contains(name)
        || !identifier(name)
    {
        return Ok(None);
    }

    let Some(request) = relative(&imports.parent, &logical) else {
        return Ok(None);
    };

    Ok(Some(ImportCandidate {
        path,
        name: name.to_owned(),
        request,
    }))
}

fn import_expression(node: &ModuleNode, candidate: &ImportCandidate) -> Option<String> {
    match node.effective.settings.editing.import_require {
        RequireStyle::String => Some(literal::quote(&candidate.request)),

        RequireStyle::Instance => {
            let (map, script) = node.map.as_ref()?;
            let placements = map.placements(&candidate.path);

            if placements.len() != 1 {
                return None;
            }

            let target = placements[0];
            let mut current = *script;
            let mut expression = "script".to_owned();
            let mut ancestors = Vec::new();

            while let Ok(parent) = map.parent(current) {
                ancestors.push((current, expression.clone()));
                current = parent;
                expression.push_str(".Parent");
            }

            ancestors.push((current, expression));

            for (ancestor, expression) in ancestors {
                if let Some(steps) = mapped_path(map, ancestor, target, &mut BTreeSet::new()) {
                    return Some(steps.into_iter().fold(expression, |expression, step| {
                        format!("{expression}:WaitForChild({})", literal::quote(&step))
                    }));
                }
            }

            None
        }
    }
}

fn mapped_import_names(
    node: &ModuleNode,
    path: &str,
    base: &str,
    prefix: &str,
    capture: &mut Capture,
    names: &mut BTreeSet<String>,
    options: &Options,
) -> io::Result<()> {
    let map = if let Some((map, _)) = &node.map {
        Arc::clone(map)
    } else if let [path] = node.effective.settings.resolution.sourcemaps.as_slice() {
        let source = capture.source(path, options)?;

        Map::parse(path, &source)?
    } else {
        return Ok(());
    };

    let mut current = map.root()?;

    for step in path
        .trim_end_matches('/')
        .split('/')
        .filter(|step| !step.is_empty())
    {
        current = if step == ".." {
            map.parent(current)?
        } else {
            map.child(current, step, false)?
        };
    }

    for name in map.children(current) {
        let Ok(child) = map.child(current, name, false) else {
            continue;
        };

        let suffix = if map.class(child) == "ModuleScript" {
            ""
        } else if map.children(child).next().is_some() {
            "/"
        } else {
            continue;
        };

        let candidate = format!("{base}{name}{suffix}");

        if candidate.starts_with(prefix) {
            names.insert(candidate);
        }
    }

    Ok(())
}

fn prologue(source: &str, tree: &Tree<'_>, options: &Options) -> io::Result<usize> {
    let mut offset = usize::from(source.starts_with('\u{feff}')) * '\u{feff}'.len_utf8();

    if source[offset..].starts_with("#!") {
        offset = source[offset..]
            .find('\n')
            .map_or(source.len(), |end| offset + end + 1);
    }

    for token in &tree.tokens {
        options.check()?;

        if token.span.end <= offset {
            continue;
        }

        if token.kind == TokenKind::Whitespace
            || token.kind == TokenKind::Comment && token.bytes(tree.source).starts_with(b"--!")
        {
            offset = token.span.end;
        } else {
            break;
        }
    }

    Ok(offset)
}

pub(crate) fn relative(source: &Path, target: &Path) -> Option<String> {
    let source = source.components().collect::<Vec<_>>();
    let target = target.components().collect::<Vec<_>>();

    let common = source
        .iter()
        .zip(&target)
        .take_while(|(left, right)| left == right)
        .count();

    if common == 0 {
        return None;
    }

    let mut components = vec![".."; source.len() - common];

    for component in &target[common..] {
        components.push(component.as_os_str().to_str()?);
    }

    let path = components.join("/");

    Some(if source.len() == common {
        format!("./{path}")
    } else {
        path
    })
}

fn mapped_path(
    map: &Map,
    current: usize,
    target: usize,
    visited: &mut BTreeSet<usize>,
) -> Option<Vec<String>> {
    if current == target {
        return Some(Vec::new());
    }

    if !visited.insert(current) {
        return None;
    }

    for name in map.children(current) {
        let child = map.child(current, name, false).ok()?;

        if let Some(mut steps) = mapped_path(map, child, target, visited) {
            steps.insert(0, name.to_owned());

            return Some(steps);
        }
    }

    None
}

fn candidate(name: String, kind: CompletionKind, detail: String) -> CompletionCandidate {
    CompletionCandidate {
        name,
        kind,
        detail,
        documentation: None,
        insert_text: None,
        parentheses: Parentheses::None,
        deprecated: false,
        callable: false,
        preferred: false,
        notation: Notation::Identifier,
    }
}

fn order(completions: &mut ProjectCompletions) {
    completions.candidates.sort_by(|left, right| {
        left.entry
            .name
            .cmp(&right.entry.name)
            .then_with(|| left.entry.detail.cmp(&right.entry.detail))
    });

    completions.candidates.dedup_by(|left, right| {
        left.entry.name == right.entry.name && left.entry.detail == right.entry.detail
    });
}

fn unavailable(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::InvalidInput
            | io::ErrorKind::NotFound
            | io::ErrorKind::Unsupported
            | io::ErrorKind::NotADirectory
    )
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}
