use instar_project::{
    DiagnosticQuery, EditReport, EditRequest, ExtractionRequest, FixRequest, ImportRequest, Input,
    LinkedRequest, PositionQueryKind, Query, QueryOutput, RenameRequest,
};

use instar_shared::{Completion, SourceVersion, Span};
use serde_json::json;

use tower_lsp_server::{
    gen_lsp_types::{
        CodeAction, CodeActionParams, CodeActionResponse, Command, Diagnostic,
        LinkedEditingRangeParams, LinkedEditingRanges, PrepareRenameResult, ProgressToken,
        RenameParams, TextDocumentPositionParams, WorkspaceEdit,
    },
    jsonrpc::{Error, Result},
};

use super::{assistance::lazy, diagnostics};

use crate::{
    Server, position,
    session::{LazyRecord, failure},
};

pub(crate) async fn prepare(
    server: &Server,
    parameters: TextDocumentPositionParams,
) -> Result<Option<PrepareRenameResult>> {
    server
        .state
        .lock()
        .expect("session state")
        .editable(&parameters.text_document.uri)?;

    let (snapshot, position) = server.position(
        &parameters.text_document.uri,
        parameters.position,
        PositionQueryKind::PrepareRename,
    )?;

    let report = server
        .query(snapshot, Query::Inspect(position), None)
        .await?;

    let QueryOutput::Locations(locations) = report.output else {
        return Err(Error::internal_error());
    };

    if locations.len() > 1 {
        return Err(Error::invalid_params("rename target is ambiguous"));
    }

    locations
        .into_iter()
        .next()
        .map(|location| {
            server
                .range(&report.snapshot, location)
                .map(PrepareRenameResult::Range)
        })
        .transpose()
}

pub(crate) async fn rename(
    server: &Server,
    parameters: RenameParams,
) -> Result<Option<WorkspaceEdit>> {
    let parameters_position = parameters.text_document_position_params;

    server
        .state
        .lock()
        .expect("session state")
        .editable(&parameters_position.text_document.uri)?;

    let (snapshot, position) = server.position(
        &parameters_position.text_document.uri,
        parameters_position.position,
        PositionQueryKind::PrepareRename,
    )?;

    let report = edit(
        server,
        EditRequest::Rename(RenameRequest {
            snapshot,
            position,
            name: parameters.new_name,
            workspace: true,
        }),
        parameters.work_done_progress_params.work_done_token,
    )
    .await?;

    let validated = report
        .validated
        .as_ref()
        .ok_or_else(|| Error::invalid_params("rename did not establish the required guarantees"))?;

    server
        .workspace_edit(&report.snapshot, validated, false)
        .map(Some)
}

pub(crate) async fn actions(
    server: &Server,
    parameters: CodeActionParams,
) -> Result<Option<Vec<CodeActionResponse>>> {
    server
        .state
        .lock()
        .expect("session state")
        .editable(&parameters.text_document.uri)?;

    let (snapshot, source) = server.capture(&parameters.text_document.uri)?;
    let encoding = server.state.lock().expect("session state").encoding.clone();

    let range = Span::new(
        position::offset(&source.text, parameters.range.start, &encoding)?,
        position::offset(&source.text, parameters.range.end, &encoding)?,
        &source,
    )
    .map_err(|error| failure(&error))?;

    let selected = |kind: &str| {
        parameters.context.only.as_ref().is_none_or(|only| {
            only.iter().any(|requested| {
                kind == requested.as_str() || kind.starts_with(&format!("{}.", requested.as_str()))
            })
        })
    };

    let deferred = server
        .state
        .lock()
        .expect("session state")
        .capabilities
        .resolves("codeAction", "edit");

    let mut result = Vec::new();

    if selected("quickfix") || selected("source.fixAll.instar") {
        let queried = server
            .query(
                snapshot.clone(),
                Query::Diagnostics(DiagnosticQuery {
                    inputs: vec![Input::Source(source.version.identity)],
                    lint: true,
                    checking: false,
                }),
                parameters.work_done_progress_params.work_done_token,
            )
            .await?;

        let QueryOutput::Diagnostics {
            lint: Some(lint), ..
        } = queried.output
        else {
            return Err(Error::internal_error());
        };

        let fixes = lint
            .fixes
            .iter()
            .filter(|fix| {
                fix.proposal.edits.iter().any(|edit| {
                    edit.location.source == source.version
                        && edit.location.span.start <= range.end
                        && range.start <= edit.location.span.end
                })
            })
            .cloned()
            .collect::<Vec<_>>();

        if selected("quickfix") {
            for fix in &fixes {
                let title = format!("Fix {}", fix.rule);

                let findings = lint
                    .findings
                    .iter()
                    .filter(|finding| finding.code == fix.rule.to_string())
                    .cloned()
                    .map(|finding| {
                        diagnostics::convert(server, &lint.snapshot, &lint.snapshots, finding)
                    })
                    .collect::<Result<Vec<_>>>()?;

                let report = edit(
                    server,
                    EditRequest::Fixes(FixRequest {
                        snapshot: snapshot.clone(),
                        fixes: vec![fix.clone()],
                    }),
                    None,
                )
                .await?;

                result.push(action(
                    server,
                    source.version,
                    title,
                    "quickfix",
                    report,
                    findings,
                    deferred,
                )?);
            }
        }

        if selected("source.fixAll.instar") && !lint.fixes.is_empty() {
            let report = edit(
                server,
                EditRequest::Fixes(FixRequest {
                    snapshot: snapshot.clone(),
                    fixes: lint.fixes,
                }),
                None,
            )
            .await?;

            result.push(action(
                server,
                source.version,
                "Fix all Instar findings".to_owned(),
                "source.fixAll.instar",
                report,
                Vec::new(),
                deferred,
            )?);
        }
    }

    if selected("source.organizeImports") {
        let report = edit(
            server,
            EditRequest::Imports(ImportRequest {
                snapshot: snapshot.clone(),
                source: source.version,
            }),
            None,
        )
        .await?;

        result.push(action(
            server,
            source.version,
            "Organize imports".to_owned(),
            "source.organizeImports",
            report,
            Vec::new(),
            deferred,
        )?);
    }

    if selected("refactor.extract") && range.start != range.end {
        let report = edit(
            server,
            EditRequest::Extract(ExtractionRequest {
                snapshot,
                source: source.version,
                range,
            }),
            None,
        )
        .await?;

        result.push(action(
            server,
            source.version,
            "Extract literal".to_owned(),
            "refactor.extract",
            report,
            Vec::new(),
            deferred,
        )?);
    }

    Ok(Some(result))
}

pub(crate) async fn resolve(server: &Server, action: CodeAction) -> Result<CodeAction> {
    let record = lazy(server, action.data.as_ref(), "action")?;

    let mut result: CodeAction = serde_json::from_value(record.payload["item"].clone())
        .map_err(|_| Error::internal_error())?;

    result.data = action.data;

    Ok(result)
}

pub(crate) async fn linked(
    server: &Server,
    parameters: LinkedEditingRangeParams,
) -> Result<Option<LinkedEditingRanges>> {
    let parameters_position = parameters.text_document_position_params;

    let (snapshot, position) = server.position(
        &parameters_position.text_document.uri,
        parameters_position.position,
        PositionQueryKind::PrepareRename,
    )?;

    let source = position.source;

    let report = edit(
        server,
        EditRequest::Linked(LinkedRequest { snapshot, position }),
        parameters.work_done_progress_params.work_done_token,
    )
    .await?;

    let ranges = report
        .linked_ranges
        .into_iter()
        .map(|location| {
            if location.source != source {
                return Err(Error::invalid_params(
                    "linked editing must remain in one source",
                ));
            }

            server.range(&report.snapshot, location)
        })
        .collect::<Result<Vec<_>>>()?;

    if ranges.is_empty() {
        Ok(None)
    } else {
        Ok(Some(LinkedEditingRanges {
            ranges,
            word_pattern: Some("[A-Za-z_][A-Za-z_0-9]*".to_owned()),
        }))
    }
}

pub(crate) async fn edit(
    server: &Server,
    request: EditRequest,
    token: Option<ProgressToken>,
) -> Result<EditReport> {
    let options = server.options.child();
    let task = server.project.edit(request, options.clone());

    let report = server
        .task(task, &options, token, "Preparing Luau edits")
        .await?;

    if !server
        .state
        .lock()
        .expect("session state")
        .current(&server.project, &report.inputs)
    {
        return Err(Error::content_modified());
    }

    if report.completion != Completion::Complete || !report.unavailable_guarantees.is_empty() {
        let rendered = report
            .diagnostics
            .iter()
            .cloned()
            .map(|finding| {
                diagnostics::convert(server, &report.snapshot, &report.snapshots, finding)
            })
            .collect::<Result<Vec<_>>>()?;

        let mut error = if report.completion != Completion::Complete {
            failure(&report.completion.error())
        } else {
            Error::invalid_params(report.unavailable_guarantees.join("\n"))
        };

        error.data = Some(
            json!({"diagnostics":report.diagnostics,"rendered":rendered,"completion":report.completion}),
        );

        return Err(error);
    }

    Ok(report)
}

fn action(
    server: &Server,
    source: SourceVersion,
    title: String,
    kind: &str,
    report: EditReport,
    findings: Vec<Diagnostic>,
    deferred: bool,
) -> Result<CodeActionResponse> {
    let validated = report
        .validated
        .as_ref()
        .ok_or_else(|| Error::invalid_params("action did not establish the required guarantees"))?;

    let literal = server
        .state
        .lock()
        .expect("session state")
        .capabilities
        .has("/textDocument/codeAction/codeActionLiteralSupport");

    if !literal {
        let command = match kind {
            "quickfix" => "instar.applyFix",
            "source.fixAll.instar" => "instar.fixAll",
            "source.organizeImports" => "instar.organizeImports",
            "refactor.extract" => "instar.extractLiteral",
            _ => return Err(Error::internal_error()),
        };

        let mut state = server.state.lock().expect("session state");

        if !state.current(&server.project, &report.inputs) {
            return Err(Error::content_modified());
        }

        let argument = state.identities.insert(LazyRecord {
            operation: "command",
            source: Some(source),
            inputs: report.inputs,
            snapshot: report.snapshot,
            payload: json!({"command":command}),
            edit: Some(validated.clone()),
        })?;

        return Ok(Command {
            title,
            tooltip: None,
            command: command.to_owned(),
            arguments: Some(vec![argument]),
        }
        .into());
    }

    let mut item = CodeAction {
        title,
        kind: Some(kind.to_owned().into()),
        diagnostics: (!findings.is_empty()).then_some(findings),
        ..CodeAction::default()
    };

    item.edit = Some(server.workspace_edit(&report.snapshot, validated, false)?);
    let mut state = server.state.lock().expect("session state");

    if !state.current(&server.project, &report.inputs) {
        return Err(Error::content_modified());
    }

    let identity = state.identities.insert(LazyRecord {
        operation: "action",
        source: Some(source),
        inputs: report.inputs,
        snapshot: report.snapshot,
        payload: json!({"item":item}),
        edit: Some(validated.clone()),
    })?;

    item.data = Some(identity);

    if deferred {
        item.edit = None;
    }

    Ok(item.into())
}
