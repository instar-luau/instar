use std::{collections::BTreeSet, ops::Range, path::Path};

use instar_bridge::{Completion, CompletionKind, Frontend, Parameter};

use instar_core::{
    analysis::{self, TokenKind},
    lint::Symbols,
    workspace::{Purpose, SymbolOutcome},
};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use tower_lsp_server::{
    gen_lsp_types::{
        ActiveParameter, CompletionItem, CompletionItemKind, CompletionItemTag,
        CompletionItemTextEdit, CompletionList, CompletionParams, CompletionResponse, Contents,
        Documentation, Hover, HoverParams, InsertReplaceEdit, InsertTextFormat, MarkupContent,
        MarkupKind, ParameterInformation, ParameterInformationLabel, Position,
        Range as ProtocolRange, SignatureHelp, SignatureHelpParams, SignatureInformation, TextEdit,
        Uri,
    },
    jsonrpc::{Error, Result},
};

use super::{
    configuration::Require,
    document::Document,
    project::{Project, failure},
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

pub(super) fn hover(
    project: &mut Project,
    frontend: &mut Frontend<'_>,
    parameters: HoverParams,
) -> Result<Option<Hover>> {
    let position = parameters.text_document_position_params;
    let module = project.module(&position.text_document.uri)?;
    let document = project.snapshot(&module)?;
    let (line, column) = document.native(position.position, project.encoding)?;
    let markdown = format(project, "/textDocument/hover/contentFormat");
    let mut sections = Vec::new();
    let mut range = None;

    for value in frontend.hover(&module, line, column).map_err(failure)? {
        let declaration = if value.symbol.is_empty() {
            value.type_name
        } else {
            format!("{}: {}", value.symbol, value.type_name)
        };

        let mut section = if markdown {
            format!("```luau\n{declaration}\n```")
        } else {
            declaration
        };

        let documentation = documentation(
            project,
            &module,
            &value.documentation_symbol,
            &value.documentation,
            markdown,
        )?;

        if !documentation.is_empty() {
            section.push_str("\n\n");
            section.push_str(&documentation);
        }

        if !sections.contains(&section) {
            sections.push(section);
        }

        if range.is_none() {
            range = Some(document.native_range(value.location.range, project.encoding)?);
        }
    }

    Ok((!sections.is_empty()).then(|| Hover {
        contents: Contents::MarkupContent(markup(sections.join("\n\n---\n\n"), markdown)),
        range,
    }))
}

pub(super) fn completion(
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
    let mut seen = BTreeSet::new();

    for (index, candidate) in candidates.iter().enumerate() {
        let identity = (
            candidate.label.clone(),
            candidate.type_name.clone(),
            candidate.insert_text.clone(),
            candidate.replacement.range.start.line,
            candidate.replacement.range.start.column,
            candidate.replacement.range.end.line,
            candidate.replacement.range.end.column,
            candidate.replace_dot,
        );

        if !seen.insert(identity) {
            continue;
        }

        let Some(mut item) = semantic(project, &document, position.position, candidate)? else {
            continue;
        };

        if !resolves(project, "documentation") {
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

    let expression = candidates.iter().any(|candidate| {
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

    if project.configuration.imports.enabled
        && expression
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
                if resolves(project, "detail") {
                    item.detail = None;
                }

                if resolves(project, "additionalTextEdits") {
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

pub(super) fn completion_resolve(
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
                .completion(&module, line, column)
                .map_err(failure)?
                .into_iter()
                .nth(index)
                .filter(|candidate| candidate.label == label)
                .ok_or_else(Error::content_modified)?;

            let mut item = semantic(project, &document, position, &candidate)?
                .ok_or_else(Error::content_modified)?;

            item.detail = (!candidate.type_name.is_empty()).then_some(candidate.type_name.clone());
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

pub(super) fn signature_help(
    project: &mut Project,
    frontend: &mut Frontend<'_>,
    parameters: SignatureHelpParams,
) -> Result<Option<SignatureHelp>> {
    let position = parameters.text_document_position_params;
    let module = project.module(&position.text_document.uri)?;
    let document = project.snapshot(&module)?;
    let (line, column) = document.native(position.position, project.encoding)?;

    let markdown = format(
        project,
        "/textDocument/signatureHelp/signatureInformation/documentationFormat",
    );

    let mut signatures = Vec::new();
    let mut active_signature = None;
    let mut active_parameter = None;

    for signature in frontend
        .signatures(&module, line, column)
        .map_err(failure)?
    {
        let active = signature
            .active_parameter
            .min(u32::try_from(signature.parameters.len().saturating_sub(1)).map_err(failure)?);

        let metadata = metadata(project, &module, &signature.documentation_symbol)?;

        let text = render(
            project,
            &module,
            metadata.as_ref(),
            &signature.documentation,
            markdown,
            &mut BTreeSet::new(),
        )?;

        let mut search = 0;

        let parameters = signature
            .parameters
            .iter()
            .enumerate()
            .map(|(index, parameter)| {
                let label = parameter_label(project, &signature.label, parameter, &mut search)?;
                let mut text = parameter.documentation.clone();

                if let Some(parameters) = metadata
                    .as_ref()
                    .and_then(|metadata| metadata.get("params"))
                    .and_then(Value::as_array)
                {
                    let entry = parameters
                        .iter()
                        .find(|entry| {
                            entry.get("name").and_then(Value::as_str)
                                == Some(parameter.name.as_str())
                        })
                        .or_else(|| {
                            parameters.get(
                                index
                                    + usize::from(
                                        parameters
                                            .first()
                                            .and_then(|entry| entry.get("name"))
                                            .and_then(Value::as_str)
                                            == Some("self"),
                                    ),
                            )
                        });

                    if let Some(value) = entry.and_then(|entry| entry.get("documentation")) {
                        text = render(
                            project,
                            &module,
                            Some(value),
                            &text,
                            markdown,
                            &mut BTreeSet::new(),
                        )?;
                    }
                }

                Ok(ParameterInformation {
                    label,
                    documentation: (!text.is_empty())
                        .then(|| Documentation::MarkupContent(markup(text, markdown))),
                })
            })
            .collect::<Result<Vec<_>>>()?;

        let value = SignatureInformation {
            label: signature.label,
            documentation: (!text.is_empty())
                .then(|| Documentation::MarkupContent(markup(text, markdown))),
            parameters: Some(parameters),
            active_parameter: project
                .supports("/textDocument/signatureHelp/signatureInformation/activeParameterSupport")
                .then_some(ActiveParameter::Int(active)),
        };

        if let Some(index) = signatures.iter().position(|existing| existing == &value) {
            if signature.selected {
                active_signature = Some(u32::try_from(index).map_err(failure)?);
                active_parameter = Some(ActiveParameter::Int(active));
            }

            continue;
        }

        if signature.selected || active_signature.is_none() {
            active_signature = Some(u32::try_from(signatures.len()).map_err(failure)?);
            active_parameter = Some(ActiveParameter::Int(active));
        }

        signatures.push(value);
    }

    Ok((!signatures.is_empty()).then_some(SignatureHelp {
        signatures,
        active_signature,
        active_parameter,
    }))
}

fn semantic(
    project: &Project,
    document: &Document,
    position: Position,
    candidate: &Completion,
) -> Result<Option<CompletionItem>> {
    let snippets = project.configuration.completion_snippets
        && project.supports("/textDocument/completion/completionItem/snippetSupport");

    let insertion = if candidate.snippet && !snippets {
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

    if candidate.replace_dot {
        additional.push(TextEdit {
            range: document.native_range(candidate.operation, project.encoding)?,
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
        detail: (!resolves(project, "detail") && !candidate.type_name.is_empty())
            .then_some(candidate.type_name.clone()),
        insert_text_format: Some(if candidate.snippet && snippets {
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
        .filter(|name| identifier(name))
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
            .accepts(project.workspace.directory(), Path::new(target))
        || project
            .workspace
            .excluded(target, Purpose::Index)
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
        || written(document, "require")
        || written(document, name)
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
            || written(document, root)
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

    let combined = insertion == replacement.start;

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
            if combined {
                format!("{declaration}{name}")
            } else {
                name.to_owned()
            },
        )),
        insert_text_format: Some(InsertTextFormat::PlainText),
        additional_text_edits: if combined {
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

fn written(document: &Document, name: &str) -> bool {
    document
        .analysis
        .tokens
        .iter()
        .any(|token| token.access.write() && document.text.get(token.range.clone()) == Some(name))
}

fn text_edit(
    project: &Project,
    replacement: ProtocolRange,
    position: Position,
    new_text: String,
) -> CompletionItemTextEdit {
    if project.supports("/textDocument/completion/completionItem/insertReplaceSupport")
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

fn resolves(project: &Project, property: &str) -> bool {
    project
        .capabilities
        .pointer("/textDocument/completion/completionItem/resolveSupport/properties")
        .and_then(Value::as_array)
        .is_some_and(|properties| properties.iter().any(|value| value == property))
}

fn format(project: &Project, pointer: &str) -> bool {
    project
        .capabilities
        .pointer(pointer)
        .and_then(Value::as_array)
        .is_some_and(|formats| formats.iter().any(|format| format == "markdown"))
}

fn markup(value: String, markdown: bool) -> MarkupContent {
    MarkupContent {
        kind: if markdown {
            MarkupKind::Markdown
        } else {
            MarkupKind::PlainText
        },
        value,
    }
}

fn completion_documentation(
    project: &Project,
    module: &str,
    candidate: &Completion,
) -> Result<Option<Documentation>> {
    let markdown = format(
        project,
        "/textDocument/completion/completionItem/documentationFormat",
    );

    let text = documentation(
        project,
        module,
        &candidate.documentation_symbol,
        &candidate.documentation,
        markdown,
    )?;

    Ok((!text.is_empty()).then(|| Documentation::MarkupContent(markup(text, markdown))))
}

fn documentation(
    project: &Project,
    module: &str,
    symbol: &str,
    fallback: &str,
    markdown: bool,
) -> Result<String> {
    let value = metadata(project, module, symbol)?;

    render(
        project,
        module,
        value.as_ref(),
        fallback,
        markdown,
        &mut BTreeSet::new(),
    )
}

fn metadata(project: &Project, module: &str, symbol: &str) -> Result<Option<Value>> {
    if symbol.is_empty() {
        return Ok(None);
    }

    project
        .workspace
        .documentation(module, symbol)
        .map_err(failure)
}

fn render(
    project: &Project,
    module: &str,
    value: Option<&Value>,
    fallback: &str,
    markdown: bool,
    visited: &mut BTreeSet<String>,
) -> Result<String> {
    let Some(value) = value else {
        return Ok(fallback.to_owned());
    };

    let text = value
        .as_str()
        .or_else(|| value.get("documentation").and_then(Value::as_str))
        .unwrap_or(fallback);

    if text.starts_with('@') && text.contains('/') && !text.contains(char::is_whitespace) {
        if !visited.insert(text.to_owned()) {
            return Ok(fallback.to_owned());
        }

        let referenced = metadata(project, module, text)?;

        return render(
            project,
            module,
            referenced.as_ref(),
            fallback,
            markdown,
            visited,
        );
    }

    let mut text = if markdown {
        text.replace("<code>", "`")
            .replace("</code>", "`")
            .replace("<strong>", "**")
            .replace("</strong>", "**")
    } else {
        text.replace("<code>", "")
            .replace("</code>", "")
            .replace("<strong>", "")
            .replace("</strong>", "")
    };

    if let Some(sample) = value
        .get("code_sample")
        .and_then(Value::as_str)
        .filter(|sample| !sample.is_empty())
    {
        text.push_str(if markdown { "\n\n```luau\n" } else { "\n\n" });

        text.push_str(sample);

        if markdown {
            text.push_str("\n```");
        }
    }

    if let Some(link) = value
        .get("learn_more_link")
        .and_then(Value::as_str)
        .filter(|link| {
            link.parse::<Uri>()
                .is_ok_and(|uri| matches!(uri.scheme(), "http" | "https"))
        })
    {
        text.push_str(if markdown {
            "\n\n[Learn more]("
        } else {
            "\n\n"
        });

        text.push_str(link);

        if markdown {
            text.push(')');
        }
    }

    Ok(text)
}

fn parameter_label(
    project: &Project,
    signature: &str,
    parameter: &Parameter,
    search: &mut usize,
) -> Result<ParameterInformationLabel> {
    let named = if parameter.name.is_empty() || parameter.variadic {
        parameter.type_name.clone()
    } else {
        format!("{}: {}", parameter.name, parameter.type_name)
    };

    let label = if signature[*search..].contains(&named) {
        named
    } else {
        parameter.type_name.clone()
    };

    if project.supports(
        "/textDocument/signatureHelp/signatureInformation/parameterInformation/labelOffsetSupport",
    ) && let Some(start) = signature[*search..]
        .find(&label)
        .map(|index| *search + index)
    {
        let end = start + label.len();
        *search = end;

        return Ok(ParameterInformationLabel::Tuple((
            u32::try_from(signature[..start].encode_utf16().count()).map_err(failure)?,
            u32::try_from(signature[..end].encode_utf16().count()).map_err(failure)?,
        )));
    }

    Ok(ParameterInformationLabel::String(label))
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

fn identifier(name: &str) -> bool {
    let mut characters = name.chars();

    characters
        .next()
        .is_some_and(|character| character.is_ascii_alphabetic() || character == '_')
        && characters.all(|character| character.is_ascii_alphanumeric() || character == '_')
        && !matches!(
            name,
            "and"
                | "break"
                | "do"
                | "else"
                | "elseif"
                | "end"
                | "false"
                | "for"
                | "function"
                | "if"
                | "in"
                | "local"
                | "nil"
                | "not"
                | "or"
                | "repeat"
                | "return"
                | "then"
                | "true"
                | "until"
                | "while"
                | "const"
        )
}
