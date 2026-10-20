use std::{ops::Range, path::Path};

use instar_bridge::{Completion, CompletionKind, Frontend};

use instar_core::{
    analysis::{self, TokenKind, is_identifier},
    lint::Symbols,
    presentation::{self, Surface},
    workspace::{Purpose, SymbolOutcome},
};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use tower_lsp_server::{
    gen_lsp_types::{
        CompletionItem, CompletionItemKind, CompletionItemTag, CompletionItemTextEdit,
        CompletionList, CompletionParams, CompletionResponse, Documentation, InsertReplaceEdit,
        InsertTextFormat, Position, Range as ProtocolRange, TextEdit,
    },
    jsonrpc::{Error, Result},
};

use crate::{
    configuration::Require, document::Document, error::failure, output::documentation,
    project::Project,
};

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
enum Resolve {
    Semantic {
        position: Position,
        index: usize,
        label: String,
    },

    Import {
        position: Position,
        target: String,
    },
}

pub(crate) fn completion(
    project: &mut Project,
    frontend: &mut Frontend<'_>,
    parameters: CompletionParams,
) -> Result<Option<CompletionResponse>> {
    let position = parameters.text_document_position_params;
    let module = project.module(&position.text_document.uri)?;
    let document = project.snapshot(&module)?;
    let (line, column) = document.native(position.position, project.encoding)?;

    let candidates = frontend
        .completion(&module, line, column)
        .map_err(failure)?;

    let mut items = Vec::new();

    for (index, candidate) in candidates.iter().enumerate() {
        if candidates[..index]
            .iter()
            .any(|previous| is_same_completion(previous, candidate))
        {
            continue;
        }

        let Some(mut item) = semantic(project, &document, position.position, candidate)? else {
            continue;
        };

        if !can_resolve(project, "documentation") {
            item.documentation = completion_documentation(project, &module, candidate)?;
        }

        item.data = Some(project.data(
            &module,
            Resolve::Semantic {
                position: position.position,
                index,
                label: candidate.label.clone(),
            },
        )?);

        items.push(item);
    }

    let offset = document.offset(position.position, project.encoding)?;
    let replacement = word(&document.text, offset);
    let prefix = &document.text[replacement.start..offset];

    let preceding = document.text[..replacement.start]
        .trim_end()
        .chars()
        .next_back();

    let is_expression = candidates.iter().any(|candidate| {
        matches!(
            candidate.kind,
            CompletionKind::Binding | CompletionKind::Module
        )
    }) && !matches!(preceding, Some('.' | ':'))
        && !document.analysis.tokens.iter().any(|token| {
            token.range.start <= offset
                && offset < token.range.end
                && matches!(token.kind, TokenKind::String | TokenKind::Comment)
        });

    if project.configuration.imports.is_enabled
        && is_expression
        && let SymbolOutcome::Ready(symbols) =
            project.workspace.symbols(&module).map_err(failure)?
    {
        let prefix = prefix.to_lowercase();

        for target in project.index()? {
            let Some(name) = import_name(&target) else {
                continue;
            };

            if !name.to_lowercase().starts_with(&prefix)
                || items.iter().any(|existing| existing.label == name)
            {
                continue;
            }

            if let Some(mut item) = imported(
                project,
                &module,
                &document,
                position.position,
                &target,
                name,
                &symbols,
            )? {
                if can_resolve(project, "detail") {
                    item.detail = None;
                }

                if can_resolve(project, "additionalTextEdits") {
                    item.additional_text_edits = None;
                }

                item.data = Some(project.data(
                    &module,
                    Resolve::Import {
                        position: position.position,
                        target,
                    },
                )?);

                items.push(item);
            }
        }
    }

    Ok(Some(CompletionResponse::CompletionList(CompletionList {
        is_incomplete: true,
        items,
        ..CompletionList::default()
    })))
}

pub(crate) fn completion_resolve(
    project: &mut Project,
    frontend: &mut Frontend<'_>,
    item: CompletionItem,
) -> Result<CompletionItem> {
    let (module, identity) = project.resolve::<Resolve>(item.data)?;
    let document = project.snapshot(&module)?;

    match identity {
        Resolve::Semantic {
            position,
            index,
            label,
        } => {
            let (line, column) = document.native(position, project.encoding)?;

            let candidate = frontend
                .completion_detail(&module, line, column, index)
                .map_err(failure)?
                .filter(|candidate| candidate.label == label)
                .ok_or_else(Error::content_modified)?;

            let mut item = semantic(project, &document, position, &candidate)?
                .ok_or_else(Error::content_modified)?;

            item.detail = candidate
                .snapshot
                .as_ref()
                .map(|snapshot| {
                    presentation::compact(snapshot, Surface::Completion).map_err(failure)
                })
                .transpose()?;

            item.documentation = completion_documentation(project, &module, &candidate)?;

            Ok(item)
        }

        Resolve::Import { position, target } => {
            if !project.index()?.contains(&target) {
                return Err(Error::content_modified());
            }

            let SymbolOutcome::Ready(symbols) =
                project.workspace.symbols(&module).map_err(failure)?
            else {
                return Err(Error::content_modified());
            };

            let name = import_name(&target).ok_or_else(Error::content_modified)?;

            imported(
                project, &module, &document, position, &target, name, &symbols,
            )?
            .ok_or_else(Error::content_modified)
        }
    }
}

fn semantic(
    project: &Project,
    document: &Document,
    position: Position,
    candidate: &Completion,
) -> Result<Option<CompletionItem>> {
    let should_use_snippets = project.configuration.should_use_completion_snippets
        && project.has_capability("/textDocument/completion/completionItem/snippetSupport");

    let insertion = if candidate.snippet && !should_use_snippets {
        candidate.insert_text.replace("$0", "")
    } else {
        candidate.insert_text.clone()
    };

    let replacement = document.native_range(candidate.replacement.range, project.encoding)?;

    if replacement.start.line != replacement.end.line
        || position < replacement.start
        || replacement.end < position
    {
        return Ok(None);
    }

    let mut additional = Vec::new();

    if let Some(operation) = candidate.operation {
        additional.push(TextEdit {
            range: document.native_range(operation, project.encoding)?,
            new_text: ":".to_owned(),
        });
    }

    Ok(Some(CompletionItem {
        label: candidate.label.clone(),
        kind: Some(match candidate.kind {
            CompletionKind::Property => CompletionItemKind::Property,

            CompletionKind::Binding => {
                if candidate.is_function {
                    CompletionItemKind::Function
                } else {
                    CompletionItemKind::Variable
                }
            }

            CompletionKind::Keyword | CompletionKind::HotComment => CompletionItemKind::Keyword,
            CompletionKind::String => CompletionItemKind::Value,
            CompletionKind::Type => CompletionItemKind::Interface,
            CompletionKind::Module | CompletionKind::RequirePath => CompletionItemKind::Module,
            CompletionKind::GeneratedFunction => CompletionItemKind::Function,
            _ => CompletionItemKind::Text,
        }),
        tags: (candidate.deprecated
            && project
                .capabilities
                .pointer("/textDocument/completion/completionItem/tagSupport")
                .is_some())
        .then_some(vec![CompletionItemTag::Deprecated]),
        detail: if can_resolve(project, "detail") {
            None
        } else {
            candidate
                .snapshot
                .as_ref()
                .map(|snapshot| {
                    presentation::compact(snapshot, Surface::Completion).map_err(failure)
                })
                .transpose()?
        },
        insert_text_format: Some(if candidate.snippet && should_use_snippets {
            InsertTextFormat::Snippet
        } else {
            InsertTextFormat::PlainText
        }),
        text_edit: Some(text_edit(project, replacement, position, insertion)),
        additional_text_edits: (!additional.is_empty()).then_some(additional),
        ..CompletionItem::default()
    }))
}

fn import_name(target: &str) -> Option<&str> {
    let path = Path::new(target);

    path.file_stem()
        .and_then(|name| name.to_str())
        .filter(|name| *name != "init")
        .or_else(|| {
            path.parent()
                .and_then(Path::file_name)
                .and_then(|name| name.to_str())
        })
        .filter(|name| is_identifier(name))
}

fn imported(
    project: &Project,
    module: &str,
    document: &Document,
    position: Position,
    target: &str,
    name: &str,
    symbols: &Symbols,
) -> Result<Option<CompletionItem>> {
    if target == module
        || !project
            .configuration
            .imports
            .can_import(project.workspace.directory(), Path::new(target))
        || project
            .workspace
            .is_excluded(target, Purpose::Index)
            .map_err(failure)?
    {
        return Ok(None);
    }

    if symbols.globals.contains(name)
        || document
            .analysis
            .bindings
            .iter()
            .any(|binding| binding.name == name || binding.name == "require")
        || is_written(document, "require")
        || is_written(document, name)
    {
        return Ok(None);
    }

    let Some(expression) = project
        .workspace
        .import(
            module,
            target,
            project.configuration.imports.require == Require::Instance,
        )
        .map_err(failure)?
    else {
        return Ok(None);
    };

    let root = expression.split(['.', ':', '[']).next().unwrap_or_default();

    if !expression.starts_with(['\'', '"'])
        && (root == name
            || is_written(document, root)
            || document
                .analysis
                .bindings
                .iter()
                .any(|binding| binding.name == root))
    {
        return Ok(None);
    }

    let offset = document.offset(position, project.encoding)?;
    let replacement = word(&document.text, offset);
    let insertion = insertion(&document.text);

    let newline = if document.text.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    };

    let declaration = format!(
        "{} {name} = require({expression}){newline}",
        project.configuration.imports.binding.keyword()
    );

    let is_combined = insertion == replacement.start;

    if insertion > replacement.start {
        return Ok(None);
    }

    let mut proposed = document.text.to_string();
    proposed.replace_range(replacement.clone(), name);
    proposed.insert_str(insertion, &declaration);

    if analysis::validate(&proposed).is_err() {
        return Ok(None);
    }

    let replacement = document.range(replacement, project.encoding)?;

    Ok(Some(CompletionItem {
        label: name.to_owned(),
        kind: Some(CompletionItemKind::Module),
        detail: Some(format!("require({expression})")),
        text_edit: Some(text_edit(
            project,
            replacement,
            position,
            if is_combined {
                format!("{declaration}{name}")
            } else {
                name.to_owned()
            },
        )),
        insert_text_format: Some(InsertTextFormat::PlainText),
        additional_text_edits: if is_combined {
            None
        } else {
            Some(vec![TextEdit {
                range: document.range(insertion..insertion, project.encoding)?,
                new_text: declaration,
            }])
        },
        sort_text: Some(format!("~{name}")),
        ..CompletionItem::default()
    }))
}

fn is_written(document: &Document, name: &str) -> bool {
    document.analysis.tokens.iter().any(|token| {
        token.access.is_write() && document.text.get(token.range.clone()) == Some(name)
    })
}

fn text_edit(
    project: &Project,
    replacement: ProtocolRange,
    position: Position,
    new_text: String,
) -> CompletionItemTextEdit {
    if project.has_capability("/textDocument/completion/completionItem/insertReplaceSupport")
        && replacement.start.line == replacement.end.line
        && replacement.start <= position
        && position <= replacement.end
    {
        CompletionItemTextEdit::InsertReplaceEdit(InsertReplaceEdit {
            new_text,
            insert: ProtocolRange {
                start: replacement.start,
                end: position,
            },
            replace: replacement,
        })
    } else {
        CompletionItemTextEdit::TextEdit(TextEdit {
            range: replacement,
            new_text,
        })
    }
}

fn can_resolve(project: &Project, property: &str) -> bool {
    project
        .capabilities
        .pointer("/textDocument/completion/completionItem/resolveSupport/properties")
        .and_then(Value::as_array)
        .is_some_and(|properties| properties.iter().any(|value| value == property))
}

fn completion_documentation(
    project: &Project,
    module: &str,
    candidate: &Completion,
) -> Result<Option<Documentation>> {
    let should_use_markdown = documentation::should_use_markdown(
        project,
        "/textDocument/completion/completionItem/documentationFormat",
    );

    let text = documentation::documentation(
        project,
        module,
        &candidate.documentation_symbol,
        &candidate.documentation,
        should_use_markdown,
    )?;

    Ok((!text.is_empty())
        .then(|| Documentation::MarkupContent(documentation::markup(text, should_use_markdown))))
}

fn is_same_completion(left: &Completion, right: &Completion) -> bool {
    left.label == right.label
        && left.insert_text == right.insert_text
        && left.replacement.range == right.replacement.range
        && left.operation == right.operation
        && match (&left.snapshot, &right.snapshot) {
            (Some(left), Some(right)) => left.equivalent(right),
            (None, None) => true,
            _ => false,
        }
}

fn word(source: &str, offset: usize) -> Range<usize> {
    let start = source[..offset]
        .char_indices()
        .rev()
        .take_while(|(_, character)| character.is_ascii_alphanumeric() || *character == '_')
        .last()
        .map_or(offset, |(index, _)| index);

    let end = source[offset..]
        .char_indices()
        .take_while(|(_, character)| character.is_ascii_alphanumeric() || *character == '_')
        .last()
        .map_or(offset, |(index, character)| {
            offset + index + character.len_utf8()
        });

    start..end
}

fn insertion(source: &str) -> usize {
    let mut offset = usize::from(source.starts_with('\u{feff}')) * 3;

    for line in source[offset..].split_inclusive('\n') {
        if line.starts_with("--!") || (offset <= 3 && line.starts_with("#!")) {
            offset += line.len();
        } else {
            break;
        }
    }

    offset
}
