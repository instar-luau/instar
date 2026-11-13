use std::{collections::BTreeSet, ops::Range};

use instar_bridge::{Completion, CompletionContext, CompletionKind, Frontend, Parentheses};

use instar_core::{format, workspace::SymbolOutcome};

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
    discovery, document::Document, documentation as resolution, error::failure,
    output::documentation, project::Project,
};

use super::imports::{Import, ImportContext, imported, module_bindings, module_imports};

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
        quality: u8,
    },
}

pub(crate) fn completion(
    project: &mut Project,
    frontend: &mut Frontend<'_>,
    parameters: CompletionParams,
    is_ready: bool,
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

    let is_type = candidates
        .iter()
        .any(|candidate| candidate.context == CompletionContext::Type)
        && preceding != Some('.');

    let is_expression = candidates.iter().any(|candidate| {
        matches!(
            candidate.context,
            CompletionContext::Expression | CompletionContext::Statement
        )
    });

    if is_ready
        && project.discovery.is_some()
        && project.configuration.imports.is_enabled
        && (is_type || is_expression)
        && let SymbolOutcome::Ready(symbols) =
            project.workspace.symbols(&module).map_err(failure)?
    {
        let prefix = prefix.to_lowercase();

        let imports = module_imports(
            &module,
            &document,
            &frontend.imports(&module).map_err(failure)?,
        )?;

        let modules: BTreeSet<_> = project
            .discovery
            .as_ref()
            .into_iter()
            .flat_map(|modules| modules.keys())
            .filter(|target| discovery::contains(project, &module, target))
            .cloned()
            .collect();

        let occupied: BTreeSet<_> = candidates
            .iter()
            .filter(|candidate| !is_type || candidate.kind == CompletionKind::Type)
            .map(|candidate| candidate.label.as_str())
            .collect();

        let mut members = Vec::new();

        for module in modules
            .iter()
            .chain(imports.keys().filter(|module| !modules.contains(*module)))
        {
            if !discovery::members(project, module, imports.contains_key(module))? {
                continue;
            }

            if let Some(exports) = project.exports.get(module) {
                let targets =
                    exports
                        .members
                        .iter()
                        .filter(|_| !is_type)
                        .map(|member| Import::Member {
                            module: module.clone(),
                            name: member.name.clone(),
                        })
                        .chain(exports.types.iter().filter(|_| is_type).map(|alias| {
                            Import::Type {
                                module: module.clone(),
                                name: alias.name.clone(),
                            }
                        }));

                for target in targets {
                    if let Some(quality) = target
                        .name()
                        .and_then(|name| matching(name, &prefix, false))
                    {
                        members.push((target, quality));
                    }
                }
            }
        }

        let targets: Vec<_> = project
            .workspace
            .services(&module)
            .map_err(failure)?
            .into_iter()
            .map(Import::Service)
            .chain(modules.into_iter().map(Import::Module))
            .filter(|_| !is_type)
            .filter_map(|target| {
                let quality = matching(
                    target.name()?,
                    &prefix,
                    matches!(target, Import::Service(_)),
                )?;

                Some((target, quality))
            })
            .chain(members)
            .filter(|(target, _)| target.name().is_some_and(|name| !occupied.contains(name)))
            .collect();

        if !targets.is_empty()
            && let Some(insertion) =
                format::insertion(&document.text, replacement.start).map_err(failure)?
        {
            let imports = module_bindings(imports, &insertion);

            let context = ImportContext {
                document: &document,
                position: position.position,
                symbols: &symbols,
                insertion,
                is_resolved: false,
                imports,
            };

            for (target, quality) in targets {
                if let Some(mut item) = imported(project, &module, &target, quality, &context)? {
                    if can_resolve(project, "detail") {
                        item.detail = None;
                    }

                    item.data = Some(project.data(
                        &module,
                        Resolve::Import {
                            position: position.position,
                            target,
                            quality,
                        },
                    )?);

                    items.push(item);
                }
            }
        }
    }

    items.sort_by(|left, right| {
        left.sort_text
            .as_deref()
            .unwrap_or(&left.label)
            .cmp(right.sort_text.as_deref().unwrap_or(&right.label))
    });

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
    is_ready: bool,
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

        Resolve::Import {
            position,
            target,
            quality,
        } => {
            if !is_ready {
                return Err(Error::content_modified());
            }

            let imports = module_imports(
                &module,
                &document,
                &frontend.imports(&module).map_err(failure)?,
            )?;

            let offset = document.offset(position, project.encoding)?;
            let replacement = word(&document.text, offset);

            let insertion = format::insertion(&document.text, replacement.start)
                .map_err(failure)?
                .ok_or_else(Error::content_modified)?;

            let imports = module_bindings(imports, &insertion);
            let is_type = matches!(target, Import::Type { .. });

            let is_available = match &target {
                Import::Module(target) => discovery::contains(project, &module, target),

                Import::Member {
                    module: target,
                    name,
                }
                | Import::Type {
                    module: target,
                    name,
                } => {
                    (discovery::contains(project, &module, target) || imports.contains_key(target))
                        && discovery::members(project, target, imports.contains_key(target))?
                        && project.exports.get(target).is_some_and(|exports| {
                            if is_type {
                                exports.types.iter().any(|alias| alias.name == *name)
                            } else {
                                exports.members.iter().any(|member| member.name == *name)
                            }
                        })
                }

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

            let context = ImportContext {
                document: &document,
                position,
                symbols: &symbols,
                insertion,
                is_resolved: true,
                imports,
            };

            imported(project, &module, &target, quality, &context)?
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

    let sort_text = if matches!(
        candidate.kind,
        CompletionKind::Binding | CompletionKind::Module
    ) {
        let offset = document.offset(position, project.encoding)?;
        let prefix = &document.text[word(&document.text, offset).start..offset];
        let quality = matching(&candidate.label, prefix, false).unwrap_or(3);

        Some(format!("0{quality}:{}", candidate.label))
    } else {
        None
    };

    Ok(Some(CompletionItem {
        label: candidate.label.clone(),
        sort_text,
        kind: Some(match candidate.kind {
            CompletionKind::Property if candidate.is_function => {
                if candidate.has_receiver {
                    CompletionItemKind::Method
                } else {
                    CompletionItemKind::Function
                }
            }

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

fn matching(name: &str, prefix: &str, is_service: bool) -> Option<u8> {
    let prefix = prefix.to_ascii_lowercase();
    let lowercase = name.to_ascii_lowercase();

    if lowercase == prefix {
        return Some(0);
    }

    if is_service
        && name
            .bytes()
            .enumerate()
            .filter(|(index, character)| {
                *index == 0
                    || character.is_ascii_uppercase()
                        && (!name.as_bytes()[index - 1].is_ascii_uppercase()
                            || name
                                .as_bytes()
                                .get(index + 1)
                                .is_some_and(u8::is_ascii_lowercase))
            })
            .map(|(_, character)| character.to_ascii_lowercase())
            .eq(prefix.bytes())
    {
        return Some(1);
    }

    if lowercase.starts_with(&prefix) {
        return Some(1 + u8::from(is_service));
    }

    let mut remaining = prefix.chars();
    let mut wanted = remaining.next()?;
    let mut previous: Option<char> = None;
    let mut matched = None;
    let mut follows_boundaries = true;

    for (index, character) in name.chars().enumerate() {
        if character.to_ascii_lowercase() == wanted {
            let boundary = previous.is_none_or(|previous| {
                !previous.is_ascii_alphanumeric()
                    || previous.is_ascii_lowercase() && character.is_ascii_uppercase()
            });

            if matched != index.checked_sub(1) && !boundary {
                follows_boundaries = false;
            }

            matched = Some(index);

            if let Some(next) = remaining.next() {
                wanted = next;
            } else {
                return Some((if follows_boundaries { 2 } else { 3 }) + u8::from(is_service));
            }
        }

        previous = Some(character);
    }

    None
}

pub(super) fn text_edit(
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

pub(super) fn can_resolve(project: &Project, property: &str) -> bool {
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
        && left.context == right.context
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

pub(super) fn word(source: &str, offset: usize) -> Range<usize> {
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
