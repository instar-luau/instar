use std::{
    collections::{BTreeMap, BTreeSet},
    ops::{ControlFlow, Range},
};

use instar_bridge::Frontend;

use instar_core::{
    analysis::{Edit, TokenKind},
    format, lint,
};

use tower_lsp_server::{
    gen_lsp_types::{
        Code, CodeActionParams, CodeActionResponse, DocumentChange, PrepareRenamePlaceholder,
        PrepareRenameResult, RenameParams, TextDocumentPositionParams, TextEdit, WorkspaceEdit,
    },
    jsonrpc::{Error, Result},
};

use super::{
    document::{Document, Encoding},
    error::failure,
    output::actions,
    project::Project,
};

pub(super) fn code_action(
    project: &mut Project,
    parameters: &CodeActionParams,
) -> Result<Option<Vec<CodeActionResponse>>> {
    let module = project.module(&parameters.text_document.uri)?;
    let document = project.snapshot(&module)?;
    let range = document.bytes(parameters.range, project.encoding)?;
    let mut actions = Vec::new();

    let is_requested = |kind: &str| {
        parameters.context.only.as_ref().is_none_or(|kinds| {
            kinds.iter().any(|value| {
                let value = String::from(value.clone());

                value.is_empty() || kind == value || kind.starts_with(&format!("{value}."))
            })
        })
    };

    let findings = if is_requested("quickfix") || is_requested("source.fixAll.instar") {
        project.findings(&module, &document.text)?
    } else {
        Vec::new()
    };

    if is_requested("quickfix") {
        for finding in &findings {
            if finding.edits.is_empty() || !is_intersecting(&range, &finding.range) {
                continue;
            }

            let diagnostics: Vec<_> = parameters
                .context
                .diagnostics
                .iter()
                .filter(|diagnostic| {
                    matches!(&diagnostic.code, Some(Code::String(rule)) if rule == &finding.rule)
                        && diagnostic
                            .source
                            .as_deref()
                            .is_none_or(|source| source == "instar")
                        && document
                            .bytes(diagnostic.range, project.encoding)
                            .is_ok_and(|range| is_intersecting(&range, &finding.range))
                })
                .cloned()
                .collect();

            if !parameters.context.diagnostics.is_empty() && diagnostics.is_empty() {
                continue;
            }

            let selected: Vec<_> = findings
                .iter()
                .filter(|candidate| {
                    candidate.rule == finding.rule && candidate.range == finding.range
                })
                .cloned()
                .collect();

            if let Ok(edit) = fix(project, &module, &document, &selected) {
                actions.push(actions::quickfix(&finding.message, edit, diagnostics));
            }
        }
    }

    if is_requested("source.fixAll.instar") {
        let rules: BTreeSet<_> = parameters
            .context
            .diagnostics
            .iter()
            .filter_map(|diagnostic| {
                if diagnostic
                    .source
                    .as_deref()
                    .is_some_and(|source| source != "instar")
                {
                    return None;
                }

                match &diagnostic.code {
                    Some(Code::String(rule)) => Some(rule.clone()),
                    _ => None,
                }
            })
            .collect();

        let selected: Vec<_> = findings
            .into_iter()
            .filter(|finding| {
                parameters.context.diagnostics.is_empty() || rules.contains(&finding.rule)
            })
            .collect();

        if let Ok(edit) = fix(project, &module, &document, &selected)
            && has_changes(&edit)
        {
            actions.push(actions::fix_all(edit));
        }
    }

    if is_requested("source.organizeImports")
        && let Ok(edit) = organize(project, &module)
        && has_changes(&edit)
    {
        actions.push(actions::organize(edit));
    }

    Ok(Some(actions))
}

pub(super) fn action(project: &mut Project, module: &str, kind: &str) -> Result<WorkspaceEdit> {
    match kind {
        "source.fixAll" | "source.fixAll.instar" => {
            let document = project.snapshot(module)?;
            let findings = project.findings(module, &document.text)?;

            fix(project, module, &document, &findings)
        }

        "source.organizeImports" => organize(project, module),

        _ => Err(Error::invalid_params("Unknown source action.")),
    }
}

pub(super) fn prepare_rename_source(
    project: &mut Project,
    parameters: &TextDocumentPositionParams,
) -> Result<ControlFlow<Option<PrepareRenameResult>>> {
    let module = project.module(&parameters.text_document.uri)?;
    let document = project.snapshot(&module)?;
    let offset = document.offset(parameters.position, project.encoding)?;

    if let Some(target) = document.analysis.prepare_rename(offset)
        && !document.analysis.bindings.iter().any(|binding| {
            binding.is_exported
                && binding
                    .occurrences
                    .iter()
                    .any(|occurrence| occurrence.range == target.range)
        })
    {
        return Ok(ControlFlow::Break(Some(
            PrepareRenameResult::PrepareRenamePlaceholder(PrepareRenamePlaceholder {
                range: document.range(target.range, project.encoding)?,
                placeholder: target.name,
            }),
        )));
    }

    Ok(ControlFlow::Continue(()))
}

pub(super) fn prepare_rename(
    project: &mut Project,
    frontend: &mut Frontend<'_>,
    parameters: &TextDocumentPositionParams,
) -> Result<Option<PrepareRenameResult>> {
    let module = project.module(&parameters.text_document.uri)?;
    let document = project.snapshot(&module)?;
    let offset = document.offset(parameters.position, project.encoding)?;

    let Some(token) = document.analysis.tokens.iter().find(|token| {
        token.range.contains(&offset)
            && matches!(
                token.kind,
                TokenKind::Variable
                    | TokenKind::Property
                    | TokenKind::Function
                    | TokenKind::Method
                    | TokenKind::Type
            )
    }) else {
        return Ok(None);
    };

    let name = &document.text[token.range.clone()];
    let (line, column) = document.native(parameters.position, project.encoding)?;
    let candidates = project.index()?;

    if frontend
        .rename(&module, line, column, name, &candidates)
        .map_err(failure)?
        .is_empty()
    {
        return Ok(None);
    }

    Ok(Some(PrepareRenameResult::PrepareRenamePlaceholder(
        PrepareRenamePlaceholder {
            range: document.range(token.range.clone(), project.encoding)?,
            placeholder: name.to_owned(),
        },
    )))
}

pub(super) fn rename_source(
    project: &mut Project,
    parameters: &RenameParams,
) -> Result<ControlFlow<Option<WorkspaceEdit>>> {
    let position = &parameters.text_document_position_params;
    let module = project.module(&position.text_document.uri)?;
    let document = project.snapshot(&module)?;
    let offset = document.offset(position.position, project.encoding)?;

    if document.analysis.prepare_rename(offset).is_some() {
        let edits = document
            .analysis
            .rename(offset, &parameters.new_name)
            .map_err(failure)?;

        if !document.analysis.bindings.iter().any(|binding| {
            binding.is_exported
                && binding
                    .occurrences
                    .iter()
                    .any(|occurrence| occurrence.range.contains(&offset))
        }) {
            return project
                .edits(BTreeMap::from([(module, edits)]))
                .map(|edit| ControlFlow::Break(Some(edit)));
        }
    }

    Ok(ControlFlow::Continue(()))
}

pub(super) fn rename(
    project: &mut Project,
    frontend: &mut Frontend<'_>,
    parameters: RenameParams,
) -> Result<Option<WorkspaceEdit>> {
    let position = parameters.text_document_position_params;
    let module = project.module(&position.text_document.uri)?;
    let document = project.snapshot(&module)?;
    let (line, column) = document.native(position.position, project.encoding)?;
    let candidates = project.index()?;

    let occurrences = frontend
        .rename(&module, line, column, &parameters.new_name, &candidates)
        .map_err(failure)?;

    let mut changes: BTreeMap<String, Vec<Edit>> = BTreeMap::new();

    for occurrence in occurrences {
        let target = occurrence.location.module;
        let document = project.snapshot(&target)?;
        let range = document.native_bytes(occurrence.location.range)?;

        changes.entry(target).or_default().push(Edit {
            range,
            replacement: parameters.new_name.clone(),
        });
    }

    for edits in changes.values_mut() {
        edits.sort_by_key(|edit| (edit.range.start, edit.range.end));
        edits.dedup();
    }

    project.edits(changes).map(Some)
}

pub(super) fn format(
    document: &Document,
    encoding: Encoding,
    settings: &format::Prepared,
    range: Option<Range<usize>>,
) -> Result<Option<Vec<TextEdit>>> {
    if let Some(range) = range {
        let edit = format::range(&document.text, settings, range).map_err(failure)?;

        return edit
            .into_iter()
            .map(|edit| {
                if document.text.get(edit.range.clone()) != Some(edit.expected.as_str()) {
                    return Err(Error::content_modified());
                }

                Ok(TextEdit {
                    range: document.range(edit.range, encoding)?,
                    new_text: edit.replacement,
                })
            })
            .collect::<Result<Vec<_>>>()
            .map(Some);
    }

    let replacement = format::format(&document.text, settings).map_err(failure)?;

    Ok(Some(if replacement == document.text.as_ref() {
        Vec::new()
    } else {
        vec![TextEdit {
            range: document.range(0..document.text.len(), encoding)?,
            new_text: replacement,
        }]
    }))
}

fn organize(project: &mut Project, module: &str) -> Result<WorkspaceEdit> {
    let document = project.snapshot(module)?;
    let settings = project.workspace.formatting(module).map_err(failure)?;

    let edits = format::organize(&document.text, &settings)
        .map_err(failure)?
        .into_iter()
        .map(|edit| {
            if document.text.get(edit.range.clone()) != Some(edit.expected.as_str()) {
                return Err(Error::content_modified());
            }

            Ok(Edit {
                range: edit.range,
                replacement: edit.replacement,
            })
        })
        .collect::<Result<Vec<_>>>()?;

    project.edits(BTreeMap::from([(module.to_owned(), edits)]))
}

fn fix(
    project: &mut Project,
    module: &str,
    document: &Document,
    findings: &[lint::Finding],
) -> Result<WorkspaceEdit> {
    let replacement = lint::apply(&document.text, findings).map_err(failure)?;

    let edits = if replacement == document.text.as_ref() {
        Vec::new()
    } else {
        vec![Edit {
            range: 0..document.text.len(),
            replacement,
        }]
    };

    project.edits(BTreeMap::from([(module.to_owned(), edits)]))
}

fn is_intersecting(left: &Range<usize>, right: &Range<usize>) -> bool {
    if left.is_empty() {
        right.start <= left.start && left.start <= right.end
    } else {
        left.start < right.end && right.start < left.end
    }
}

fn has_changes(edit: &WorkspaceEdit) -> bool {
    edit.changes.as_ref().is_some_and(|changes| changes.values().any(|edits| !edits.is_empty()))
        || edit.document_changes.as_ref().is_some_and(|changes| changes.iter().any(|change| {
            matches!(change, DocumentChange::TextDocumentEdit(document) if !document.edits.is_empty())
        }))
}
