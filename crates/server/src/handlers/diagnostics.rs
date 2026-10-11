use std::{collections::BTreeMap, iter::once, sync::Arc};

use instar_project::{
    DiagnosticQuery, Input, InputIdentity, InputState, InputVersion, ProjectSnapshot, Query,
    QueryOutput, QueryReport, QueryRequest,
};

use instar_shared::{
    Completion, Diagnostic as DomainDiagnostic, DiagnosticCategory,
    DiagnosticTag as DomainDiagnosticTag, FailureKind, Location as DomainLocation, Reason,
    Severity, SourceVersion,
};

use serde_json::Value;
use tokio::task;

use tower_lsp_server::{
    gen_lsp_types::{
        CodeDescription, Diagnostic, DiagnosticRelatedInformation, DiagnosticSeverity,
        DiagnosticTag, DocumentDiagnosticParams, DocumentDiagnosticReport,
        FullDocumentDiagnosticReport, Location as ProtocolLocation, MessageType,
        RelatedFullDocumentDiagnosticReport, RelatedUnchangedDocumentDiagnosticReport,
        UnchangedDocumentDiagnosticReport, Uri, WorkspaceDiagnosticParams,
        WorkspaceDiagnosticReport, WorkspaceDiagnosticReportPartialResult,
        WorkspaceDiagnosticRequest, WorkspaceFullDocumentDiagnosticReport,
        WorkspaceUnchangedDocumentDiagnosticReport,
    },
    jsonrpc::{Error, ErrorCode, Result},
};

use crate::{
    Server,
    session::{LazyRecord, RetainedResult, failure, path},
};

impl Server {
    pub(crate) fn schedule_diagnostics(&self) {
        let mut publication = self.publication.lock().expect("diagnostic publication");

        if let Some((operation, cancellation)) = publication.take() {
            cancellation.cancel();
            operation.abort();
        }

        let pull = self
            .state
            .lock()
            .expect("session state")
            .capabilities
            .pull_diagnostics();

        if pull {
            return;
        }

        let project = Arc::clone(&self.project);
        let state = Arc::clone(&self.state);
        let client = self.client.clone();
        let snapshot = project.snapshot();
        let options = self.options.child();
        let cancellation = options.cancellation.clone();
        let inputs = inputs(self);

        let progress = if inputs.iter().any(|input| matches!(input, Input::Source(_))) {
            self.prepare_progress(None, &options.cancellation)
        } else {
            Ok(None)
        };

        let task = project.query(
            QueryRequest {
                snapshot: snapshot.clone(),
                operation: Query::Diagnostics(DiagnosticQuery {
                    inputs,
                    lint: true,
                    checking: true,
                }),
            },
            options.clone(),
        );

        let operation = task::spawn(async move {
            let mut operation = Box::pin(task);

            let progress = match progress {
                Ok(progress) => progress,

                Err(error) => {
                    client
                        .log_message(MessageType::Error, error.to_string())
                        .await;

                    return;
                }
            };

            let (result, _progress) = tokio::select! {
                result = &mut operation => (result, None),
                progress = async {
                    match progress {
                        Some(progress) => progress.begin("Analyzing Luau workspace", &options).await.map(Some),
                        None => Ok(None),
                    }
                } => match progress {
                    Ok(progress) => (operation.await, progress),
                    Err(error) => {
                        client.log_message(MessageType::Error, error.to_string()).await;
                        return;
                    }
                },
            };

            let (snapshot, publication_inputs, findings, complete) = match result {
                Ok(report) => {
                    if report.completion != Completion::Complete {
                        client
                            .log_message(
                                MessageType::Warning,
                                report.completion.error().to_string(),
                            )
                            .await;
                    }

                    let findings = match findings(&report) {
                        Ok(findings) => findings,

                        Err(error) => {
                            client
                                .log_message(MessageType::Error, error.to_string())
                                .await;

                            return;
                        }
                    };

                    (
                        report.snapshot,
                        report.inputs,
                        findings,
                        report.completion == Completion::Complete,
                    )
                }

                Err(error) => {
                    if matches!(
                        &error.completion,
                        Completion::Incomplete(reasons) if reasons.contains(&Reason::StaleQuery)
                    ) {
                        return;
                    }

                    if error.kind != FailureKind::InvalidInvocation
                        || !error
                            .diagnostics
                            .iter()
                            .any(|finding| finding.code == "resolution.sourcemap")
                    {
                        client
                            .log_message(MessageType::Error, error.to_string())
                            .await;
                    }

                    if error.diagnostics.is_empty()
                        || matches!(
                            error.completion,
                            Completion::Cancelled | Completion::TimedOut
                        )
                    {
                        return;
                    }

                    let current = project.snapshot();

                    let Ok(inputs) = failure_inputs(&snapshot, &current, &error.diagnostics) else {
                        return;
                    };

                    (current, inputs, error.diagnostics, false)
                }
            };

            let mut grouped = BTreeMap::<Uri, Vec<Diagnostic>>::new();

            let conversion = (|| {
                let mut state = state.lock().expect("session state");

                if !state.current(&project, &publication_inputs) {
                    return Err(Error::content_modified());
                }

                if complete {
                    let sources = state
                        .documents
                        .values()
                        .filter(|document| document.synchronized)
                        .map(|document| document.source)
                        .collect::<Vec<_>>();

                    for source in sources {
                        grouped.entry(state.uri(&snapshot, source)?).or_default();
                    }
                }

                for finding in findings {
                    let uri = state.uri(&snapshot, finding.primary.source.identity)?;

                    grouped
                        .entry(uri)
                        .or_default()
                        .push(convert_with(finding, |location| {
                            state.location(&snapshot, location)
                        })?);
                }

                if complete {
                    for uri in &state.published {
                        grouped.entry(uri.clone()).or_default();
                    }
                }

                Ok(())
            })();

            if let Err(error) = conversion {
                if error.code != ErrorCode::ContentModified {
                    client
                        .log_message(MessageType::Error, error.to_string())
                        .await;
                }

                return;
            }

            for (uri, items) in grouped {
                let version = {
                    let mut state = state.lock().expect("session state");

                    if !state.current(&project, &publication_inputs) {
                        return;
                    }

                    if items.is_empty() {
                        state.published.remove(&uri);
                    } else {
                        state.published.insert(uri.clone());
                    }

                    state.documents.get(&uri).map(|document| document.version)
                };

                client.publish_diagnostics(uri, items, version).await;
            }
        });

        *publication = Some((operation, cancellation));
    }
}

pub(crate) async fn document(
    server: &Server,
    parameters: DocumentDiagnosticParams,
) -> Result<DocumentDiagnosticReport> {
    if parameters.identifier.is_some() {
        return Err(Error::invalid_params("unknown diagnostic provider"));
    }

    let options = server.options.child();
    let (snapshot, source) = server.capture(&parameters.text_document.uri, &options)?;

    let report = server
        .query(
            snapshot,
            Query::Diagnostics(DiagnosticQuery {
                inputs: vec![Input::Source(source.version.identity)],
                lint: true,
                checking: true,
            }),
            parameters.work_done_progress_params.work_done_token,
            &options,
        )
        .await?;

    let findings = findings(&report)?;
    let mut items = Vec::new();

    for finding in findings
        .into_iter()
        .filter(|finding| finding.primary.source.identity == source.version.identity)
    {
        items.push(convert(server, &report.snapshot, &[], finding)?);
    }

    if report.completion != Completion::Complete && items.is_empty() {
        return Err(failure(&report.completion.error()));
    }

    let (result_id, unchanged) = identifier(
        server,
        &report,
        Some(source.version),
        &items,
        parameters.previous_result_id.as_deref(),
    )?;

    if unchanged {
        Ok(RelatedUnchangedDocumentDiagnosticReport {
            unchanged_document_diagnostic_report: UnchangedDocumentDiagnosticReport { result_id },
            ..RelatedUnchangedDocumentDiagnosticReport::default()
        }
        .into())
    } else {
        Ok(RelatedFullDocumentDiagnosticReport {
            full_document_diagnostic_report: FullDocumentDiagnosticReport {
                result_id: Some(result_id),
                items,
            },
            ..RelatedFullDocumentDiagnosticReport::default()
        }
        .into())
    }
}

pub(crate) async fn workspace(
    server: &Server,
    parameters: WorkspaceDiagnosticParams,
) -> Result<WorkspaceDiagnosticReport> {
    if parameters.identifier.is_some() {
        return Err(Error::invalid_params("unknown diagnostic provider"));
    }

    let snapshot = server.project.snapshot();
    let inputs = inputs(server);

    let options = server.options.child();

    let report = server
        .query(
            snapshot,
            Query::Diagnostics(DiagnosticQuery {
                inputs,
                lint: true,
                checking: true,
            }),
            parameters.work_done_progress_params.work_done_token,
            &options,
        )
        .await?;

    let mut grouped = BTreeMap::<Uri, Vec<Diagnostic>>::new();

    let QueryOutput::Diagnostics { check, lint } = &report.output else {
        return Err(Error::internal_error());
    };

    if report.completion == Completion::Complete {
        for source in check
            .iter()
            .flat_map(|check| &check.sources)
            .chain(lint.iter().flat_map(|lint| &lint.sources))
        {
            let uri = server
                .state
                .lock()
                .expect("session state")
                .uri(&report.snapshot, source.identity)?;

            grouped.entry(uri).or_default();
        }

        for (uri, document) in &server.state.lock().expect("session state").documents {
            if document.synchronized
                && report
                    .snapshot
                    .source(document.source)
                    .is_some_and(|source| source.version == document.revision)
            {
                grouped.entry(uri.clone()).or_default();
            }
        }
    }

    for finding in findings(&report)? {
        let uri = server.location(&report.snapshot, finding.primary)?.uri;

        grouped
            .entry(uri)
            .or_default()
            .push(convert(server, &report.snapshot, &[], finding)?);
    }

    for previous in &parameters.previous_result_ids {
        let removed = if previous.uri.scheme() == "file" {
            let filename = path(&previous.uri)?;

            report.snapshot.source_at(&filename).is_none()
                && report.inputs.iter().any(|input| {
                    input.identity == InputIdentity::Path(filename.clone())
                        && input.state == InputState::Missing
                })
        } else {
            false
        };

        if report.completion == Completion::Complete || removed {
            grouped.entry(previous.uri.clone()).or_default();
        }
    }

    let mut items = Vec::new();

    for (uri, diagnostics) in grouped {
        let version = server
            .state
            .lock()
            .expect("session state")
            .documents
            .get(&uri)
            .map(|document| document.version);

        let source = if uri.scheme() == "file" {
            report.snapshot.source_at(&path(&uri)?)
        } else {
            report.snapshot.source_for_address(uri.as_str())
        }
        .map(|source| source.version);

        let previous = parameters
            .previous_result_ids
            .iter()
            .find(|previous| previous.uri == uri)
            .map(|previous| previous.value.as_str());

        let (result_id, unchanged) = identifier(server, &report, source, &diagnostics, previous)?;

        if unchanged {
            items.push(
                WorkspaceUnchangedDocumentDiagnosticReport {
                    uri,
                    version,
                    unchanged_document_diagnostic_report: UnchangedDocumentDiagnosticReport {
                        result_id,
                    },
                }
                .into(),
            );
        } else {
            items.push(
                WorkspaceFullDocumentDiagnosticReport {
                    uri,
                    version,
                    full_document_diagnostic_report: FullDocumentDiagnosticReport {
                        result_id: Some(result_id),
                        items: diagnostics,
                    },
                }
                .into(),
            );
        }
    }

    if let Some(token) = parameters.partial_result_params.partial_result_token {
        server
            .client
            .send_partial_result::<WorkspaceDiagnosticRequest>(
                token,
                WorkspaceDiagnosticReportPartialResult { items },
            )
            .await;

        return Ok(WorkspaceDiagnosticReport { items: Vec::new() });
    }

    Ok(WorkspaceDiagnosticReport { items })
}

pub(crate) fn convert(
    server: &Server,
    snapshot: &ProjectSnapshot,
    snapshots: &[ProjectSnapshot],
    finding: DomainDiagnostic,
) -> Result<Diagnostic> {
    convert_with(finding, |location| {
        let snapshot = once(snapshot)
            .chain(snapshots.iter())
            .find(|snapshot| {
                snapshot
                    .source(location.source.identity)
                    .is_some_and(|source| source.version == location.source)
            })
            .ok_or_else(Error::content_modified)?;

        server.location(snapshot, location)
    })
}

fn convert_with(
    finding: DomainDiagnostic,
    mut location: impl FnMut(DomainLocation) -> Result<ProtocolLocation>,
) -> Result<Diagnostic> {
    let range = location(finding.primary)?.range;
    let native = finding.identity.producer() == "luau";

    let related = finding
        .related
        .into_iter()
        .map(|related| {
            Ok(DiagnosticRelatedInformation {
                location: location(related.location)?,
                message: related.message,
            })
        })
        .collect::<Result<Vec<_>>>()?;

    Ok(Diagnostic {
        range,
        severity: Some(match finding.severity {
            Severity::Error => DiagnosticSeverity::Error,
            Severity::Warning => DiagnosticSeverity::Warning,
            Severity::Information => DiagnosticSeverity::Information,
            Severity::Hint => DiagnosticSeverity::Hint,
        }),
        code: Some(
            if native && let Ok(code) = finding.identity.code().parse::<i32>() {
                code.into()
            } else {
                finding.code.into()
            },
        ),
        code_description: if native && finding.category == DiagnosticCategory::Lint {
            Some(CodeDescription {
                href: "https://luau.org/lint/"
                    .parse::<Uri>()
                    .map_err(|error| Error {
                        message: error.to_string().into(),
                        ..Error::internal_error()
                    })?,
            })
        } else {
            None
        },
        source: Some(if native { "Luau" } else { "Instar" }.to_owned()),
        message: finding.message.into(),
        related_information: (!related.is_empty()).then_some(related),
        tags: (!finding.tags.is_empty()).then(|| {
            finding
                .tags
                .into_iter()
                .map(|tag| match tag {
                    DomainDiagnosticTag::Unnecessary => DiagnosticTag::Unnecessary,
                    DomainDiagnosticTag::Deprecated => DiagnosticTag::Deprecated,
                })
                .collect()
        }),
        ..Diagnostic::default()
    })
}

pub(crate) fn findings(report: &QueryReport) -> Result<Vec<DomainDiagnostic>> {
    let QueryOutput::Diagnostics { check, lint } = &report.output else {
        return Err(Error::internal_error());
    };

    let mut findings = report.diagnostics.clone();

    if let Some(check) = check {
        findings.extend(check.findings.clone());
        findings.extend(check.environment_diagnostics.clone());
    }

    if let Some(lint) = lint {
        findings.extend(lint.findings.clone());
    }

    let mut unique = Vec::new();

    for finding in findings {
        if !unique
            .iter()
            .any(|existing: &DomainDiagnostic| existing.identity == finding.identity)
        {
            unique.push(finding);
        }
    }

    if report.completion != Completion::Complete && unique.is_empty() {
        return Err(failure(&report.completion.error()));
    }

    Ok(unique)
}

fn inputs(server: &Server) -> Vec<Input> {
    let state = server.state.lock().expect("session state");

    state
        .workspace_folders
        .iter()
        .cloned()
        .map(Input::Path)
        .chain(
            state
                .documents
                .values()
                .filter(|document| document.synchronized)
                .map(|document| Input::Source(document.source)),
        )
        .collect()
}

fn identifier(
    server: &Server,
    report: &QueryReport,
    source: Option<SourceVersion>,
    items: &[Diagnostic],
    previous: Option<&str>,
) -> Result<(String, bool)> {
    let mut state = server.state.lock().expect("session state");

    if !state.current(&server.project, &report.inputs) {
        return Err(Error::content_modified());
    }

    let mut unchanged = false;

    if let Some(identity) = previous
        && let Some(record) = serde_json::from_str::<Value>(identity)
            .ok()
            .and_then(|data| state.identities.get(Some(&data), "diagnostics").ok())
        && record.source == source
        && matches!(&record.result, RetainedResult::Diagnostics(existing) if existing == items)
    {
        if record.inputs == report.inputs {
            return Ok((identity.to_owned(), true));
        }

        unchanged = true;
    }

    let record = LazyRecord {
        source,
        inputs: report.inputs.clone(),
        snapshot: report.snapshot.clone(),
        result: RetainedResult::Diagnostics(items.to_vec()),
    };

    let identity = match state.identities.find(&record) {
        Some(identity) => identity,
        None => state.identities.insert(record)?,
    };

    Ok((identity.to_string(), unchanged))
}

fn failure_inputs(
    original: &ProjectSnapshot,
    snapshot: &ProjectSnapshot,
    findings: &[DomainDiagnostic],
) -> Result<Vec<InputVersion>> {
    let mut inputs = original
        .input_versions
        .values()
        .filter(|input| {
            !matches!(input.state, InputState::Stale)
                && matches!(
                    input.identity,
                    InputIdentity::Source(_) | InputIdentity::Context(_)
                )
        })
        .cloned()
        .collect::<Vec<_>>();

    for finding in findings {
        for source in finding.identity.inputs().iter().copied().chain(
            finding
                .related
                .iter()
                .map(|related| related.location.source),
        ) {
            let captured = [snapshot, original]
                .into_iter()
                .find(|snapshot| {
                    snapshot
                        .source(source.identity)
                        .is_some_and(|captured| captured.version == source)
                })
                .ok_or_else(Error::content_modified)?;

            let input = captured
                .input_versions
                .get(&InputIdentity::Source(source.identity))
                .filter(|input| {
                    matches!(&input.state, InputState::Source(captured) if captured.version == source)
                })
                .ok_or_else(Error::content_modified)?;

            if !inputs.contains(input) {
                inputs.push(input.clone());
            }
        }
    }

    Ok(inputs)
}
