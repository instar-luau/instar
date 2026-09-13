use super::{Result, internal_error, path, protocol, state::State};
use crate::{
    lint::{Edit, Finding, configuration::Level},
    source::{PositionEncoding, Source},
};

fn range(source: &Source, start: usize, end: usize) -> Result<protocol::Range> {
    let position = |offset| {
        let position = source
            .position(
                u32::try_from(offset).map_err(internal_error)?.into(),
                PositionEncoding::Utf16,
            )
            .map_err(internal_error)?;

        Ok(protocol::Position::new(position.line, position.col))
    };

    Ok(protocol::Range::new(position(start)?, position(end)?))
}

pub(super) fn append(
    state: &mut State,
    paths: &[std::path::PathBuf],
    output: &mut std::collections::BTreeMap<std::path::PathBuf, Vec<protocol::Diagnostic>>,
) {
    for path in paths {
        let messages = output.entry(path.clone()).or_default();

        match diagnostics(state, path) {
            Ok(findings) => messages.extend(findings),

            Err(error) => messages.push(protocol::Diagnostic {
                severity: Some(protocol::DiagnosticSeverity::ERROR),
                source: Some("instar".into()),
                message: error.to_string(),
                ..protocol::Diagnostic::default()
            }),
        }
    }
}

fn diagnostics(state: &mut State, path: &std::path::Path) -> Result<Vec<protocol::Diagnostic>> {
    let source = state.sources.read(path).map_err(internal_error)?;

    state
        .lint(path)?
        .findings
        .into_iter()
        .map(|finding| diagnostic(&source, finding))
        .collect()
}

fn diagnostic(source: &Source, finding: Finding) -> Result<protocol::Diagnostic> {
    Ok(protocol::Diagnostic {
        range: range(source, finding.start, finding.end)?,
        severity: Some(match finding.level {
            Level::Deny => protocol::DiagnosticSeverity::ERROR,
            Level::Warn => protocol::DiagnosticSeverity::WARNING,
            Level::Info | Level::Allow => protocol::DiagnosticSeverity::HINT,
        }),
        code: Some(protocol::NumberOrString::String(finding.rule)),
        source: Some("instar".into()),
        message: finding.message,
        ..protocol::Diagnostic::default()
    })
}

fn action(
    state: &State,
    source: &Source,
    parameters: &protocol::CodeActionParams,
    title: String,
    kind: protocol::CodeActionKind,
    edits: &[Edit],
) -> Result<protocol::CodeActionOrCommand> {
    crate::lint::apply(source, edits).map_err(internal_error)?;

    let edits = edits
        .iter()
        .map(|edit| {
            Ok(protocol::OneOf::Left(protocol::TextEdit {
                range: range(source, edit.start, edit.end)?,
                new_text: edit.text.clone(),
            }))
        })
        .collect::<Result<_>>()?;

    Ok(protocol::CodeActionOrCommand::CodeAction(
        protocol::CodeAction {
            title,
            kind: Some(kind),
            edit: Some(protocol::WorkspaceEdit {
                document_changes: Some(protocol::DocumentChanges::Edits(vec![
                    protocol::TextDocumentEdit {
                        text_document: protocol::OptionalVersionedTextDocumentIdentifier {
                            uri: parameters.text_document.uri.clone(),
                            version: state.version(source.path()),
                        },
                        edits,
                    },
                ])),
                ..protocol::WorkspaceEdit::default()
            }),
            ..protocol::CodeAction::default()
        },
    ))
}

pub(super) fn actions(
    state: &mut State,
    parameters: &protocol::CodeActionParams,
    individual: bool,
    all: bool,
) -> Result<protocol::CodeActionResponse> {
    let path = path(&parameters.text_document.uri)?;
    let source = state.sources.read(&path).map_err(internal_error)?;
    let report = state.lint(&path)?;
    let mut actions = Vec::new();

    if individual {
        for finding in &report.findings {
            let location = range(&source, finding.start, finding.end)?;

            if finding.edits.is_empty()
                || location.end < parameters.range.start
                || location.start > parameters.range.end
            {
                continue;
            }

            let title = if matches!(
                finding.rule.as_str(),
                "unused_variable" | "unused_function" | "unused_import"
            ) {
                format!(
                    "Prefix '{}' with '_'",
                    &source.text().map_err(internal_error)?[finding.start..finding.end]
                )
            } else {
                format!("Fix {}", finding.rule)
            };

            let edits =
                crate::lint::edits(std::slice::from_ref(finding)).map_err(internal_error)?;

            if let Ok(action) = action(
                state,
                &source,
                parameters,
                title,
                protocol::CodeActionKind::QUICKFIX,
                &edits,
            ) {
                actions.push(action);
            }
        }
    }

    if all
        && let Ok(edits) = crate::lint::edits(&report.findings)
        && !edits.is_empty()
        && let Ok(action) = action(
            state,
            &source,
            parameters,
            "Fix all Instar lint findings".into(),
            protocol::CodeActionKind::SOURCE_FIX_ALL,
            &edits,
        )
    {
        actions.push(action);
    }

    Ok(actions)
}
