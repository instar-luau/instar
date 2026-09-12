use super::{Response, Result, failure, path, protocol, state::State};
use crate::{project::resolution::Resolver, source::PositionEncoding};
use line_index::LineCol;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

pub(super) fn complete(
    state: &mut State,
    parameters: &protocol::TextDocumentPositionParams,
) -> Result<Vec<protocol::CompletionItem>> {
    let Response::Editor(entries) = state.query(parameters, "completion")? else {
        return Err(tower_lsp_server::jsonrpc::Error::internal_error());
    };

    if let Some(range) = entries
        .iter()
        .find(|entry| entry.native.require == Some(true))
        .and_then(|entry| entry.location.as_ref())
        .map(|location| location.range)
    {
        return paths(state, parameters, range);
    }

    let names = entries
        .iter()
        .filter_map(|entry| entry.native.name.clone())
        .collect::<BTreeSet<_>>();

    let insertion = entries
        .iter()
        .find(|entry| entry.native.imports == Some(true))
        .map(|entry| {
            protocol::Position::new(
                entry
                    .location
                    .as_ref()
                    .map_or(parameters.position.line, |location| {
                        location.range.start.line
                    }),
                0,
            )
        });

    let version = state.version(&path(&parameters.text_document.uri)?);

    let mut items = entries
        .into_iter()
        .filter_map(|entry| {
            Some(protocol::CompletionItem {
                label: entry.native.name?,
                detail: entry.native.description,
                insert_text: entry.native.insert,
                data: Some(serde_json::json!({"position": parameters, "version": version})),
                documentation: entry.documentation.map(|value| {
                    protocol::Documentation::MarkupContent(protocol::MarkupContent {
                        kind: protocol::MarkupKind::Markdown,
                        value,
                    })
                }),
                tags: entry
                    .native
                    .deprecated
                    .filter(|deprecated| *deprecated)
                    .map(|_| vec![protocol::CompletionItemTag::DEPRECATED]),
                ..protocol::CompletionItem::default()
            })
        })
        .collect::<Vec<_>>();

    if let Some(insertion) = insertion {
        items.extend(modules(state, parameters, insertion, &names)?);
    }

    Ok(items)
}

pub(super) fn resolve(
    state: &mut State,
    mut item: protocol::CompletionItem,
) -> Result<protocol::CompletionItem> {
    #[derive(serde::Deserialize)]
    struct Context {
        position: protocol::TextDocumentPositionParams,
        version: Option<i32>,
    }

    let Some(data) = item.data.take() else {
        return Ok(item);
    };

    let context: Context = serde_json::from_value(data).map_err(failure)?;

    if state.version(&path(&context.position.text_document.uri)?) != context.version {
        return Err(tower_lsp_server::jsonrpc::Error::content_modified());
    }

    let Response::Editor(entries) = state.query(&context.position, "completionResolve")? else {
        return Err(tower_lsp_server::jsonrpc::Error::internal_error());
    };

    if let Some(entry) = entries
        .into_iter()
        .find(|entry| entry.native.name.as_ref() == Some(&item.label))
    {
        item.detail = entry.native.description;

        item.documentation = entry.documentation.map(|value| {
            protocol::Documentation::MarkupContent(protocol::MarkupContent {
                kind: protocol::MarkupKind::Markdown,
                value,
            })
        });
    }

    Ok(item)
}

pub(super) fn specifier(from: &Path, target: &Path) -> Option<String> {
    let directory = from.parent()?;

    let directory = if from.file_stem()? == "init" {
        directory.parent().unwrap_or(directory)
    } else {
        directory
    };

    let target = if target.file_stem()? == "init" {
        target.parent()?.to_owned()
    } else {
        target.with_extension("")
    };

    let common = directory
        .ancestors()
        .find(|ancestor| target.starts_with(ancestor))?;

    let mut relative = "../".repeat(directory.strip_prefix(common).ok()?.components().count());

    if relative.is_empty() {
        relative.push_str("./");
    }

    relative.push_str(
        &target
            .strip_prefix(common)
            .ok()?
            .to_str()?
            .replace('\\', "/"),
    );

    Some(relative)
}

fn word(
    source: &crate::source::Source,
    cursor: protocol::Position,
) -> Result<(protocol::Range, &str)> {
    let text = source.text().map_err(failure)?;

    let offset = usize::from(
        source
            .offset(
                LineCol {
                    line: cursor.line,
                    col: cursor.character,
                },
                PositionEncoding::Utf16,
            )
            .map_err(failure)?,
    );

    let identifier = |character: u8| character.is_ascii_alphanumeric() || character == b'_';

    let start = text[..offset]
        .bytes()
        .rposition(|character| !identifier(character))
        .map_or(0, |offset| offset + 1);

    let end = offset
        + text[offset..]
            .bytes()
            .take_while(|character| identifier(*character))
            .count();

    let position = |offset| {
        source
            .position(
                line_index::TextSize::try_from(offset).map_err(failure)?,
                PositionEncoding::Utf16,
            )
            .map(|position| protocol::Position::new(position.line, position.col))
            .map_err(failure)
    };

    Ok((
        protocol::Range::new(position(start)?, position(end)?),
        &text[start..offset],
    ))
}

fn modules(
    state: &mut State,
    parameters: &protocol::TextDocumentPositionParams,
    mut insertion: protocol::Position,
    names: &BTreeSet<String>,
) -> Result<Vec<protocol::CompletionItem>> {
    let source = state
        .sources
        .read(&path(&parameters.text_document.uri)?)
        .map_err(failure)?;

    let text = source.text().map_err(failure)?;

    if text.starts_with("#!") && insertion.line == 0 {
        insertion.line = 1;
    }

    let (range, prefix) = word(&source, parameters.position)?;

    let Response::Editor(existing) = state.query(parameters, "imports")? else {
        return Err(tower_lsp_server::jsonrpc::Error::internal_error());
    };

    for entry in &existing {
        if let Some(location) = &entry.location
            && location.range.end.line < parameters.position.line
        {
            insertion.line = insertion.line.max(location.range.end.line + 1);
        }
    }

    let mut items = Vec::new();

    for import in candidates(state, source.path(), &existing)? {
        let name = &import.name;

        if !name
            .to_ascii_lowercase()
            .starts_with(&prefix.to_ascii_lowercase())
            || names.contains(name)
        {
            continue;
        }

        let newline = if text.contains("\r\n") { "\r\n" } else { "\n" };

        let declaration = format!("local {name} = {}{newline}", import.expression);
        let same = insertion == range.start;

        items.push(protocol::CompletionItem {
            label: name.clone(),
            kind: Some(protocol::CompletionItemKind::MODULE),
            detail: Some(import.expression),
            text_edit: Some(protocol::CompletionTextEdit::Edit(protocol::TextEdit {
                range,
                new_text: import.existing.clone().unwrap_or_else(|| {
                    if same {
                        format!("{declaration}{name}")
                    } else {
                        name.clone()
                    }
                }),
            })),
            additional_text_edits: (!same && import.existing.is_none()).then(|| {
                vec![protocol::TextEdit {
                    range: protocol::Range::new(insertion, insertion),
                    new_text: declaration,
                }]
            }),
            ..protocol::CompletionItem::default()
        });
    }

    Ok(items)
}

struct Import {
    name: String,
    expression: String,
    existing: Option<String>,
}

fn roots(state: &mut State, from: &Path) -> Result<BTreeMap<String, PathBuf>> {
    let mut resolver = Resolver::new(&mut state.sources);
    let mut names = resolver.discovery.alias_names(from).map_err(failure)?;
    names.insert("self".into());
    let mut roots = BTreeMap::new();

    for name in names {
        let prefix = format!("@{name}");

        if let Some((_, target)) = resolver.namespace(from, &prefix).map_err(failure)? {
            roots.insert(prefix, crate::source::absolute(&target).map_err(failure)?);
        }
    }

    Ok(roots)
}

fn identity(target: &Path) -> PathBuf {
    if target.file_stem().is_some_and(|name| name == "init") {
        target.parent().unwrap_or(target).to_owned()
    } else {
        target.with_extension("")
    }
}

fn quoted(value: &str, quote: char) -> String {
    let escaped = value
        .chars()
        .map(|character| {
            if character == quote || character == '\\' || character.is_control() {
                character.escape_default().to_string()
            } else {
                character.to_string()
            }
        })
        .collect::<String>();

    format!("{quote}{escaped}{quote}")
}

fn binding(name: &str) -> String {
    let mut name = name
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || character == '_' {
                character
            } else {
                '_'
            }
        })
        .collect::<String>();

    if !super::identifier(&name) {
        name.insert(0, '_');
    }

    name
}

fn candidates(
    state: &mut State,
    from: &Path,
    existing: &[super::EditorEntry],
) -> Result<Vec<Import>> {
    let environment = state.environment(from)?;
    let roots = roots(state, from)?;

    let services = environment
        .classes
        .iter()
        .filter_map(|class| class.split_once('\0'))
        .filter(|(_, flags)| flags.starts_with('1'))
        .map(|(name, _)| name)
        .collect::<BTreeSet<_>>();

    let imported = |category: &str, target: &str| {
        existing
            .iter()
            .find(|entry| {
                entry.native.label.as_deref() == Some(category)
                    && entry.native.description.as_deref() == Some(target)
            })
            .and_then(|entry| entry.native.name.clone())
    };

    let mut imports = services
        .iter()
        .map(|name| Import {
            name: (*name).into(),
            expression: format!("game:GetService({})", quoted(name, '"')),
            existing: imported("service", name),
        })
        .collect::<Vec<_>>();

    let mut targets = state.workspace()?.into_iter().collect::<BTreeSet<_>>();

    for root in roots.values() {
        if root.is_dir() && !targets.iter().any(|path| path.starts_with(root)) {
            targets
                .extend(super::workspace::files(&BTreeSet::from([root.clone()])).map_err(failure)?);
        }
    }

    let mut resolver = Resolver::new(&mut state.sources);

    for prefix in roots.keys() {
        if let Some(target) = resolver.resolve(from, prefix).map_err(failure)? {
            targets.insert(target);
        }
    }

    for (index, node) in environment.nodes.iter().enumerate() {
        if node.class_name == "ModuleScript" && environment.readable(&environment.identity(index)) {
            targets.insert(environment.source(&environment.identity(index)));
        }
    }

    for target in targets {
        if target == from || target.to_string_lossy().ends_with(".d.luau") {
            continue;
        }

        let module = identity(&target);

        let Some(name) = module.file_name().and_then(|name| name.to_str()) else {
            continue;
        };

        let mapped = environment.node(&target);

        if mapped.is_some_and(|index| environment.nodes[index].class_name != "ModuleScript") {
            continue;
        }

        let name = binding(mapped.map_or(name, |index| environment.nodes[index].name.as_str()));
        let mut expressions = BTreeSet::new();

        for specifier in specifiers(from, &target, &roots) {
            if environment
                .require(&mut resolver, from, &specifier)
                .ok()
                .flatten()
                .map(|resolved| environment.source(&resolved))
                .as_ref()
                == Some(&target)
            {
                expressions.insert(format!("require({})", quoted(&specifier, '"')));
            }
        }

        if let Some(index) = mapped {
            expressions.extend(instances(
                &environment,
                from,
                index,
                existing,
                &services,
                &roots,
            ));
        }

        for expression in expressions {
            imports.push(Import {
                name: name.clone(),
                expression,
                existing: imported("module", &target.to_string_lossy().replace('\\', "/")),
            });
        }
    }

    Ok(imports)
}

fn specifiers(from: &Path, target: &Path, roots: &BTreeMap<String, PathBuf>) -> Vec<String> {
    let mut specifiers = Vec::new();

    if let Some(relative) = specifier(from, target) {
        specifiers.push(relative);
    }

    let module = identity(target);

    for (prefix, root) in roots {
        if let Ok(tail) = module.strip_prefix(root) {
            let tail = tail.to_string_lossy().replace('\\', "/");

            specifiers.push(if tail.is_empty() {
                prefix.clone()
            } else {
                format!("{prefix}/{tail}")
            });
        }
    }

    specifiers
}

fn member(expression: &mut String, name: &str) {
    if super::identifier(name) && name != "Parent" {
        expression.push('.');
        expression.push_str(name);
    } else if name == "Parent" {
        expression.push_str(":WaitForChild(\"Parent\")");
    } else {
        expression.push('[');
        expression.push_str(&quoted(name, '"'));
        expression.push(']');
    }
}

fn instances(
    environment: &crate::roblox::Environment,
    from: &Path,
    target: usize,
    existing: &[super::EditorEntry],
    services: &BTreeSet<&str>,
    roots: &BTreeMap<String, PathBuf>,
) -> Vec<String> {
    let mut chain = Vec::new();
    let mut current = Some(target);

    while let Some(index) = current {
        chain.push(index);
        current = environment.nodes[index].parent;
    }

    chain.reverse();
    let mut expressions = Vec::new();

    if environment.nodes[chain[0]].class_name == "DataModel" && chain.len() > 1 {
        if !roots.keys().any(|name| name.eq_ignore_ascii_case("@game")) {
            let specifier = format!(
                "@game/{}",
                chain[1..]
                    .iter()
                    .map(|index| environment.nodes[*index].name.as_str())
                    .collect::<Vec<_>>()
                    .join("/")
            );

            if environment.namespace(from, &specifier) == Some(target) {
                expressions.push(format!("require({})", quoted(&specifier, '"')));
            }
        }

        let service = &environment.nodes[chain[1]];

        let mut expression = if services.contains(service.class_name.as_str()) {
            existing
                .iter()
                .find(|entry| {
                    entry.native.label.as_deref() == Some("service")
                        && entry.native.description.as_deref() == Some(service.class_name.as_str())
                })
                .and_then(|entry| entry.native.name.clone())
                .unwrap_or_else(|| format!("game:GetService({})", quoted(&service.class_name, '"')))
        } else {
            let mut expression = "game".to_owned();
            member(&mut expression, &service.name);

            expression
        };

        for index in &chain[2..] {
            member(&mut expression, &environment.nodes[*index].name);
        }

        expressions.push(format!("require({expression})"));
    }

    if let Some(mut index) = environment.node(from) {
        let mut expression = "script".to_owned();
        let mut specifier = "@self".to_owned();

        loop {
            if let Some(common) = chain.iter().position(|ancestor| *ancestor == index) {
                for index in &chain[common + 1..] {
                    member(&mut expression, &environment.nodes[*index].name);
                    specifier.push('/');
                    specifier.push_str(&environment.nodes[*index].name);
                }

                if environment.namespace(from, &specifier) == Some(target) {
                    expressions.push(format!("require({})", quoted(&specifier, '"')));
                }

                expressions.push(format!("require({expression})"));
                break;
            }

            let Some(parent) = environment.nodes[index].parent else {
                break;
            };

            index = parent;
            expression.push_str(".Parent");
            specifier.push_str("/..");
        }
    }

    expressions
}

fn paths(
    state: &mut State,
    parameters: &protocol::TextDocumentPositionParams,
    range: protocol::Range,
) -> Result<Vec<protocol::CompletionItem>> {
    let source = state
        .sources
        .read(&path(&parameters.text_document.uri)?)
        .map_err(failure)?;

    let offsets = super::formatting::offsets(&source, range)?;
    let text = source.text().map_err(failure)?;
    let literal = &text[offsets.clone()];

    let Some(quote) = literal
        .chars()
        .next()
        .filter(|quote| matches!(quote, '\'' | '"'))
    else {
        return Ok(Vec::new());
    };

    let cursor = super::formatting::offsets(
        &source,
        protocol::Range::new(range.start, parameters.position),
    )?
    .end;

    let Some(partial) = text.get(offsets.start + 1..cursor) else {
        return Ok(Vec::new());
    };

    let roots = roots(state, source.path())?;
    let environment = state.environment(source.path())?;
    let mut choices = BTreeMap::new();

    if !partial.contains('/') {
        for prefix in ["./".to_owned(), "../".to_owned()]
            .into_iter()
            .chain(roots.keys().map(|prefix| format!("{prefix}/")))
        {
            choices.insert(prefix, protocol::CompletionItemKind::FOLDER);
        }

        if environment.namespace(source.path(), "@game").is_some() {
            choices.insert("@game/".into(), protocol::CompletionItemKind::FOLDER);
        }
    } else if let Some((head, _)) = partial.rsplit_once('/') {
        let prefix = format!("{head}/");
        let configured = roots.keys().any(|name| name.eq_ignore_ascii_case("@game"));

        let mapped = (!configured || !head.starts_with("@game"))
            .then(|| environment.namespace(source.path(), head))
            .flatten();

        if let Some(index) = mapped {
            for child in &environment.nodes[index].descendants {
                let node = &environment.nodes[*child];

                if node.class_name == "ModuleScript"
                    && environment.readable(&environment.identity(*child))
                {
                    choices.insert(
                        format!("{prefix}{}", node.name),
                        protocol::CompletionItemKind::MODULE,
                    );
                }

                if !node.descendants.is_empty() {
                    choices.insert(
                        format!("{prefix}{}/", node.name),
                        protocol::CompletionItemKind::FOLDER,
                    );
                }
            }
        } else {
            filesystem(state, source.path(), &prefix, &mut choices)?;
        }
    }

    let mut range = range;
    range.start.character += 1;

    if literal.len() > 1 && literal.ends_with(quote) {
        range.end.character -= 1;
    }

    let mut items = Vec::new();

    for (value, kind) in choices {
        if !value.starts_with(partial) {
            continue;
        }

        let escaped = quoted(&value, quote);

        items.push(protocol::CompletionItem {
            label: value.clone(),
            kind: Some(kind),
            text_edit: Some(protocol::CompletionTextEdit::Edit(protocol::TextEdit {
                range,
                new_text: escaped[1..escaped.len() - 1].into(),
            })),
            ..protocol::CompletionItem::default()
        });
    }

    Ok(items)
}

fn filesystem(
    state: &mut State,
    from: &Path,
    prefix: &str,
    choices: &mut BTreeMap<String, protocol::CompletionItemKind>,
) -> Result<()> {
    let Some((_, directory)) = Resolver::new(&mut state.sources)
        .namespace(from, prefix)
        .map_err(failure)?
    else {
        return Ok(());
    };

    let directory = crate::source::absolute(&directory).map_err(failure)?;
    let mut entries = BTreeSet::new();

    match std::fs::read_dir(&directory) {
        Ok(children) => {
            for child in children {
                let child = child.map_err(failure)?;
                entries.insert(child.path());
            }
        }

        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
            ) => {}

        Err(error) => return Err(failure(error)),
    }

    for target in state.workspace()? {
        if let Ok(tail) = target.strip_prefix(&directory)
            && let Some(component) = tail.components().next()
        {
            entries.insert(directory.join(component));
        }
    }

    for target in entries {
        let Some(name) = target.file_name().and_then(|name| name.to_str()) else {
            continue;
        };

        let folder = target.is_dir()
            || state
                .sources
                .has_open_descendants(&target)
                .map_err(failure)?;

        if folder {
            choices.insert(
                format!("{prefix}{name}/"),
                protocol::CompletionItemKind::FOLDER,
            );
        }

        let name = if folder {
            name
        } else {
            if name.ends_with(".d.luau")
                || !matches!(
                    target.extension().and_then(|extension| extension.to_str()),
                    Some("lua" | "luau")
                )
            {
                continue;
            }

            let Some(name) = target
                .file_stem()
                .and_then(|name| name.to_str())
                .filter(|name| *name != "init")
            else {
                continue;
            };

            name
        };

        let specifier = format!("{prefix}{name}");

        if Resolver::new(&mut state.sources)
            .resolve(from, &specifier)
            .ok()
            .flatten()
            .is_some()
        {
            choices.insert(specifier, protocol::CompletionItemKind::MODULE);
        }
    }

    Ok(())
}

pub(super) fn fixes(
    state: &mut State,
    parameters: &protocol::CodeActionParams,
) -> Result<protocol::CodeActionResponse> {
    let source = state
        .sources
        .read(&path(&parameters.text_document.uri)?)
        .map_err(failure)?;

    let text = source.text().map_err(failure)?;
    let mut actions = Vec::new();
    let mut seen = BTreeSet::new();

    for diagnostic in &parameters.context.diagnostics {
        if diagnostic.source.as_deref() != Some("instar")
            || !diagnostic.message.contains("Unknown global")
        {
            continue;
        }

        let offsets = super::formatting::offsets(&source, diagnostic.range)?;
        let name = &text[offsets];

        if !super::identifier(name) {
            continue;
        }

        let position = protocol::TextDocumentPositionParams {
            text_document: parameters.text_document.clone(),
            position: diagnostic.range.end,
        };

        for item in complete(state, &position)? {
            if item.label != name || item.detail.is_none() {
                continue;
            }

            let Some(protocol::CompletionTextEdit::Edit(replacement)) = item.text_edit else {
                continue;
            };

            let mut edits = item.additional_text_edits.unwrap_or_default();

            if replacement.new_text != name {
                edits.push(replacement);
            }

            if edits.is_empty() {
                continue;
            }

            let title = format!(
                "Import '{name}' from {}",
                item.detail.as_deref().expect("import detail")
            );

            if !seen.insert(title.clone()) {
                continue;
            }

            actions.push(protocol::CodeActionOrCommand::CodeAction(
                protocol::CodeAction {
                    title,
                    kind: Some(protocol::CodeActionKind::QUICKFIX),
                    diagnostics: Some(vec![diagnostic.clone()]),
                    edit: Some(protocol::WorkspaceEdit {
                        document_changes: Some(protocol::DocumentChanges::Edits(vec![
                            protocol::TextDocumentEdit {
                                text_document: protocol::OptionalVersionedTextDocumentIdentifier {
                                    uri: parameters.text_document.uri.clone(),
                                    version: state.version(source.path()),
                                },
                                edits: edits.into_iter().map(protocol::OneOf::Left).collect(),
                            },
                        ])),
                        ..protocol::WorkspaceEdit::default()
                    }),
                    ..protocol::CodeAction::default()
                },
            ));
        }
    }

    Ok(actions)
}
