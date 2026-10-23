use std::{ops::Range, path::Path};

use instar_bridge::{Completion, CompletionKind, Frontend, Parentheses};

use instar_core::{
    analysis::{self, TokenKind, is_identifier},
    lint::Symbols,
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
    configuration::Require, document::Document, documentation as resolution, error::failure,
    output::documentation, project::Project,
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
        target: Import,
    },
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
enum Import {
    Module(String),
    Service(String),
}

impl Import {
    fn name(&self) -> Option<&str> {
        let name = match self {
            Self::Module(target) => {
                let path = Path::new(target);

                path.file_stem()
                    .and_then(|name| name.to_str())
                    .filter(|name| *name != "init")
                    .or_else(|| {
                        path.parent()
                            .and_then(Path::file_name)
                            .and_then(|name| name.to_str())
                    })
            }

            Self::Service(name) => Some(name.as_str()),
        };

        name.filter(|name| is_identifier(name))
    }
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
        && !document.analysis().tokens.iter().any(|token| {
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

        let modules = project.index()?;

        let targets = project
            .workspace
            .services(&module)
            .map_err(failure)?
            .into_iter()
            .map(Import::Service)
            .chain(modules.into_iter().map(Import::Module));

        for target in targets {
            let Some(name) = target.name() else {
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
                &symbols,
                false,
            )? {
                if can_resolve(project, "detail") {
                    item.detail = None;
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
    let Some((module, identity)) = project.resolve::<Resolve>(item.data.clone())? else {
        return Ok(item);
    };

    let document = project.snapshot(&module)?;

    match identity {
        Resolve::Semantic {
            position,
            index,
            label,
        } => {
            let (line, column) = document.native(position, project.encoding)?;

            let Some(candidate) = frontend
                .completion_detail(&module, line, column, index)
                .map_err(failure)?
                .filter(|candidate| candidate.label == label)
            else {
                return Ok(item);
            };

            let Some(mut item) = semantic(project, &document, position, &candidate)? else {
                return Ok(item);
            };

            item.detail.clone_from(&candidate.presentation);

            item.documentation = completion_documentation(project, &module, &candidate)?;

            Ok(item)
        }

        Resolve::Import { position, target } => {
            let is_available = match &target {
                Import::Module(target) => project.index()?.contains(target),

                Import::Service(name) => project
                    .workspace
                    .services(&module)
                    .map_err(failure)?
                    .contains(name),
            };

            if !is_available {
                return Err(Error::content_modified());
            }

            let SymbolOutcome::Ready(symbols) =
                project.workspace.symbols(&module).map_err(failure)?
            else {
                return Err(Error::content_modified());
            };

            imported(
                project, &module, &document, position, &target, &symbols, true,
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

    let mut insertion = candidate.insert_text.clone();

    insertion.push_str(match candidate.parentheses {
        Parentheses::CursorAfter if should_use_snippets => "()$0",
        Parentheses::CursorInside if should_use_snippets => "($0)",
        Parentheses::CursorAfter | Parentheses::CursorInside => "()",
        _ => "",
    });

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
        tags: (candidate.is_deprecated
            && project
                .capabilities
                .pointer("/textDocument/completion/completionItem/tagSupport")
                .is_some())
        .then_some(vec![CompletionItemTag::Deprecated]),
        detail: if can_resolve(project, "detail") {
            None
        } else {
            candidate.presentation.clone()
        },
        insert_text_format: Some(
            if candidate.parentheses != Parentheses::None && should_use_snippets {
                InsertTextFormat::Snippet
            } else {
                InsertTextFormat::PlainText
            },
        ),
        text_edit: Some(text_edit(project, replacement, position, insertion)),
        additional_text_edits: (!additional.is_empty()).then_some(additional),
        ..CompletionItem::default()
    }))
}

fn imported(
    project: &mut Project,
    module: &str,
    document: &Document,
    position: Position,
    target: &Import,
    symbols: &Symbols,
    is_resolved: bool,
) -> Result<Option<CompletionItem>> {
    let Some(name) = target.name() else {
        return Ok(None);
    };

    if !project.configuration.imports.is_enabled
        || symbols.globals.contains(name)
        || document
            .analysis()
            .bindings
            .iter()
            .any(|binding| binding.name == name)
        || is_written(document, name)
    {
        return Ok(None);
    }

    let offset = document.offset(position, project.encoding)?;
    let replacement = word(&document.text, offset);
    let insertion = insertion(&document.text);
    let is_combined = insertion == replacement.start;
    let should_defer = !is_resolved && !is_combined && can_resolve(project, "additionalTextEdits");

    let expression = match target {
        Import::Module(target) => {
            if target == module
                || !project
                    .configuration
                    .imports
                    .can_import(project.workspace.directory(), Path::new(target))
                || project
                    .workspace
                    .is_excluded(target, Purpose::Index)
                    .map_err(failure)?
                || document
                    .analysis()
                    .bindings
                    .iter()
                    .any(|binding| binding.name == "require")
                || is_written(document, "require")
            {
                return Ok(None);
            }

            let is_instance = project.configuration.imports.require == Require::Instance;
            let key = (module.to_owned(), target.clone(), is_instance);

            let expression = if should_defer && let Some(expression) = project.imports.get(&key) {
                expression.clone()
            } else {
                let expression = project
                    .workspace
                    .import(module, target, is_instance)
                    .map_err(failure)?;

                if should_defer
                    && project
                        .has_capability("/workspace/didChangeWatchedFiles/dynamicRegistration")
                {
                    project.imports.insert(key, expression.clone());
                }

                expression
            };

            let Some(expression) = expression else {
                return Ok(None);
            };

            expression
        }

        Import::Service(_) => {
            if !symbols.globals.contains("game") {
                return Ok(None);
            }

            format!("game:GetService(\"{name}\")")
        }
    };

    let root = expression.split(['.', ':', '[']).next().unwrap_or_default();

    if !expression.starts_with(['\'', '"'])
        && (root == name
            || is_written(document, root)
            || document
                .analysis()
                .bindings
                .iter()
                .any(|binding| binding.name == root))
    {
        return Ok(None);
    }

    let expression = match target {
        Import::Module(_) => format!("require({expression})"),
        Import::Service(_) => expression,
    };

    let newline = if document.text.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    };

    let declaration = format!(
        "{} {name} = {expression}{newline}",
        project.configuration.imports.binding.keyword()
    );

    if insertion > replacement.start || analysis::validate(&declaration).is_err() {
        return Ok(None);
    }

    if !should_defer {
        let mut proposed = document.text.to_string();
        proposed.replace_range(replacement.clone(), name);
        proposed.insert_str(insertion, &declaration);

        if analysis::validate(&proposed).is_err() && analysis::validate(&document.text).is_ok() {
            return Ok(None);
        }
    }

    let replacement = document.range(replacement, project.encoding)?;

    Ok(Some(CompletionItem {
        label: name.to_owned(),
        kind: Some(match target {
            Import::Module(_) => CompletionItemKind::Module,
            Import::Service(_) => CompletionItemKind::Variable,
        }),
        detail: Some(expression),
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
        additional_text_edits: if is_combined || should_defer {
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
    document.analysis().tokens.iter().any(|token| {
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
    project: &mut Project,
    module: &str,
    candidate: &Completion,
) -> Result<Option<Documentation>> {
    let should_use_markdown = documentation::should_use_markdown(
        project,
        "/textDocument/completion/completionItem/documentationFormat",
    );

    let metadata = resolution::resolve(project, module, &candidate.documentation)?;

    let text = documentation::completion(
        project,
        module,
        metadata.as_deref(),
        candidate.has_receiver,
        should_use_markdown,
    )?;

    Ok((!text.is_empty())
        .then(|| Documentation::MarkupContent(documentation::markup(text, should_use_markdown))))
}

fn is_same_completion(left: &Completion, right: &Completion) -> bool {
    left.label == right.label
        && left.insert_text == right.insert_text
        && left.parentheses == right.parentheses
        && left.has_receiver == right.has_receiver
        && left.replacement.range == right.replacement.range
        && left.operation == right.operation
        && match (&left.presentation, &right.presentation) {
            (Some(_), Some(_)) => left.equivalence != 0 && left.equivalence == right.equivalence,
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
