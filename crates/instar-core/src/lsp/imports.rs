use super::{Response, Result, failure, path, protocol, state::State};
use crate::{project::resolution::Resolver, source::PositionEncoding};
use line_index::LineCol;
use std::{collections::BTreeSet, path::Path};

pub(super) fn complete(
    state: &mut State,
    parameters: &protocol::TextDocumentPositionParams,
) -> Result<Vec<protocol::CompletionItem>> {
    let Response::Editor(entries) = state.query(parameters, "completion")? else {
        return Err(tower_lsp_server::jsonrpc::Error::internal_error());
    };

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

    let mut items = entries
        .into_iter()
        .filter_map(|entry| {
            Some(protocol::CompletionItem {
                label: entry.native.name?,
                detail: entry.native.description,
                insert_text: entry.native.insert,
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
        items.extend(modules(state, parameters, insertion)?);
    }

    Ok(items)
}

fn specifier(from: &Path, target: &Path) -> Option<String> {
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

    let Response::Editor(tokens) = state.query(parameters, "tokens")? else {
        return Err(tower_lsp_server::jsonrpc::Error::internal_error());
    };

    let names = tokens
        .into_iter()
        .filter(|entry| {
            entry.native.declaration == Some(true)
                || entry
                    .location
                    .as_ref()
                    .is_none_or(|location| location.range != range)
        })
        .filter_map(|entry| entry.native.name)
        .collect::<BTreeSet<_>>();

    state.indexed()?;
    let mut items = Vec::new();

    for target in state.index.files.keys() {
        if target == source.path() {
            continue;
        }

        let module = if target.file_stem().is_some_and(|stem| stem == "init") {
            target.parent().unwrap_or(target)
        } else {
            target.as_path()
        };

        let Some(name) = module.file_stem().and_then(|name| name.to_str()) else {
            continue;
        };

        if !name.starts_with(prefix) || names.contains(name) || !super::identifier(name) {
            continue;
        }

        let Some(specifier) = specifier(source.path(), target) else {
            continue;
        };

        if Resolver::new(&mut state.sources)
            .resolve(source.path(), &specifier)
            .ok()
            .flatten()
            .as_ref()
            != Some(target)
        {
            continue;
        }

        let quoted = serde_json::to_string(&specifier).map_err(failure)?;

        let newline = if text.contains("\r\n") { "\r\n" } else { "\n" };

        let declaration = format!("local {name} = require({quoted}){newline}");
        let same = insertion == range.start;

        items.push(protocol::CompletionItem {
            label: name.into(),
            kind: Some(protocol::CompletionItemKind::MODULE),
            detail: Some(format!("require({quoted})")),
            text_edit: Some(protocol::CompletionTextEdit::Edit(protocol::TextEdit {
                range,
                new_text: if same {
                    format!("{declaration}{name}")
                } else {
                    name.into()
                },
            })),
            additional_text_edits: (!same).then(|| {
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
