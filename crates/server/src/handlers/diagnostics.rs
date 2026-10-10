use std::{collections::BTreeMap, iter::once, sync::Arc};

use instar_project::{
    DiagnosticQuery, Input, ProjectSnapshot, Query, QueryOutput, QueryReport, QueryRequest,
};

use instar_shared::{
    Completion, Diagnostic as DomainDiagnostic, DiagnosticTag as DomainDiagnosticTag,
    Location as DomainLocation, Severity, SourceVersion,
};

use serde_json::Value;
use tokio::task;

use tower_lsp_server::{
    gen_lsp_types::{
        Diagnostic, DiagnosticRelatedInformation, DiagnosticSeverity, DiagnosticTag,
        DocumentDiagnosticParams, DocumentDiagnosticReport, FullDocumentDiagnosticReport,
        Location as ProtocolLocation, MessageType, RelatedFullDocumentDiagnosticReport,
        RelatedUnchangedDocumentDiagnosticReport, UnchangedDocumentDiagnosticReport, Uri,
        WorkspaceDiagnosticParams, WorkspaceDiagnosticReport,
        WorkspaceDiagnosticReportPartialResult, WorkspaceDiagnosticRequest,
        WorkspaceFullDocumentDiagnosticReport, WorkspaceUnchangedDocumentDiagnosticReport,
    },
    jsonrpc::{Error, ErrorCode, Result},
};

use crate::{
    Server,
    session::{LazyRecord, failure},
};

pub(crate) async fn document(
    server: &Server,
    parameters: DocumentDiagnosticParams,
) -> Result<DocumentDiagnosticReport> {
    if parameters.identifier.is_some() {
        return Err(Error::invalid_params("unknown diagnostic provider"));
    }

    let (snapshot, source) = server.capture(&parameters.text_document.uri)?;

    let report = server
        .query(
            snapshot,
            Query::Diagnostics(DiagnosticQuery {
                inputs: vec![Input::Source(source.version.identity)],
                lint: true,
                checking: true,
            }),
            parameters.work_done_progress_params.work_done_token,
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

    let report = server
        .query(
            snapshot,
            Query::Diagnostics(DiagnosticQuery {
                inputs,
                lint: true,
                checking: true,
            }),
            parameters.work_done_progress_params.work_done_token,
        )
        .await?;

    let mut grouped = BTreeMap::<Uri, Vec<Diagnostic>>::new();

    if report.completion == Completion::Complete {
        for source in report.snapshot.sources() {
            let uri = server
                .state
                .lock()
                .expect("session state")
                .uri(&report.snapshot, source.version.identity)?;

            grouped.entry(uri).or_default();
        }
    }

    for finding in findings(&report)? {
        let uri = server.location(&report.snapshot, finding.primary)?.uri;

        grouped
            .entry(uri)
            .or_default()
            .push(convert(server, &report.snapshot, &[], finding)?);
    }

    if report.completion == Completion::Complete {
        for previous in &parameters.previous_result_ids {
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

        let source = report
            .snapshot
            .source_for_address(uri.as_str())
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
    location: impl Fn(DomainLocation) -> Result<ProtocolLocation>,
) -> Result<Diagnostic> {
    let range = location(finding.primary)?.range;

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
        code: Some(finding.code.into()),
        source: Some("Instar".to_owned()),
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
    let payload = serde_json::to_value(items).map_err(|_| Error::internal_error())?;
    let mut state = server.state.lock().expect("session state");

    if !state.current(&server.project, &report.inputs) {
        return Err(Error::content_modified());
    }

    if let Some(identity) = previous
        && let Some(record) = serde_json::from_str::<Value>(identity)
            .ok()
            .and_then(|data| state.identities.get(Some(&data), "diagnostics").ok())
        && record.source == source
        && record.inputs == report.inputs
        && record.payload == payload
    {
        return Ok((identity.to_owned(), true));
    }

    let identity = state.identities.insert(LazyRecord {
        operation: "diagnostics",
        source,
        inputs: report.inputs.clone(),
        snapshot: report.snapshot.clone(),
        payload,
        edit: None,
    })?;

    Ok((identity.to_string(), false))
}

impl Server {
    pub(crate) fn schedule_diagnostics(&self) {
        if let Some(publication) = self
            .publication
            .lock()
            .expect("diagnostic publication")
            .take()
        {
            publication.abort();
        }

        let pull = self
            .state
            .lock()
            .expect("session state")
            .capabilities
            .has("/textDocument/diagnostic");

        if pull {
            return;
        }

        let project = Arc::clone(&self.project);
        let state = Arc::clone(&self.state);
        let client = self.client.clone();
        let snapshot = project.snapshot();

        let task = project.query(
            QueryRequest {
                snapshot,
                operation: Query::Diagnostics(DiagnosticQuery {
                    inputs: inputs(self),
                    lint: true,
                    checking: true,
                }),
            },
            self.options.child(),
        );

        *self.publication.lock().expect("diagnostic publication") = Some(task::spawn(async move {
            let report = match task.await {
                Ok(report) => report,

                Err(error) => {
                    client
                        .log_message(MessageType::Error, error.to_string())
                        .await;

                    return;
                }
            };

            if report.completion != Completion::Complete {
                client
                    .log_message(MessageType::Warning, report.completion.error().to_string())
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

            let mut grouped = BTreeMap::<Uri, Vec<Diagnostic>>::new();

            let conversion = (|| {
                let state = state.lock().expect("session state");

                if !state.current(&project, &report.inputs) {
                    return Err(Error::content_modified());
                }

                if report.completion == Completion::Complete {
                    for document in state
                        .documents
                        .values()
                        .filter(|document| document.synchronized)
                    {
                        grouped
                            .entry(state.uri(&report.snapshot, document.source)?)
                            .or_default();
                    }
                }

                for finding in findings {
                    let uri = state.uri(&report.snapshot, finding.primary.source.identity)?;

                    grouped
                        .entry(uri)
                        .or_default()
                        .push(convert_with(finding, |location| {
                            state.location(&report.snapshot, location)
                        })?);
                }

                if report.completion == Completion::Complete {
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

                    if !state.current(&project, &report.inputs) {
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
        }));
    }
}
