use std::{
    collections::BTreeMap,
    hash::{DefaultHasher, Hash, Hasher},
    io,
    path::PathBuf,
    sync::{Arc, Condvar, Mutex},
    thread::{self, JoinHandle},
    time::Instant,
};

use instar_analysis::{Cancellation, Completion, Options};
use instar_core::{analysis::Origin, project::Project, snapshot::Snapshot};
use instar_lint::{self as lint, Level};
use tokio::sync::{oneshot, watch};

use tower_lsp_server::{
    gen_lsp_types::{
        CodeDescription, Diagnostic, DiagnosticRelatedInformation, DiagnosticSeverity,
        DiagnosticTag, DocumentDiagnosticParams, DocumentDiagnosticReport,
        FullDocumentDiagnosticReport, PositionEncodingKind, RelatedDocument,
        RelatedFullDocumentDiagnosticReport, RelatedUnchangedDocumentDiagnosticReport,
        UnchangedDocumentDiagnosticReport, Uri, WorkspaceDiagnosticParams,
        WorkspaceDiagnosticReport, WorkspaceFullDocumentDiagnosticReport,
        WorkspaceUnchangedDocumentDiagnosticReport,
    },
    jsonrpc::{self, Error},
};

use crate::{location, session::Session};

pub(crate) type Report = BTreeMap<PathBuf, Vec<Diagnostic>>;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Activity {
    Idle,
    Running,
    Stopped,
}

struct Job {
    snapshot: Snapshot,
    paths: Vec<PathBuf>,
    roots: Vec<PathBuf>,
    encoding: PositionEncodingKind,
    options: Options,
    started: Instant,
    response: oneshot::Sender<io::Result<Report>>,
}

#[derive(Default)]
struct State {
    pending: Option<Job>,
    cancellation: Cancellation,
    stopped: bool,
}

pub(crate) struct Diagnostics {
    state: Arc<(Mutex<State>, Condvar)>,
    handle: Mutex<Option<JoinHandle<()>>>,
    activity: watch::Sender<Activity>,
}

impl Diagnostics {
    pub(crate) fn new(options: &Options) -> io::Result<Self> {
        let state = Arc::new((Mutex::new(State::default()), Condvar::new()));
        let shared = Arc::clone(&state);
        let timeout = options.timeout;
        let (activity, _) = watch::channel(Activity::Idle);
        let progress = activity.clone();

        let handle = thread::Builder::new()
            .name("instar-diagnostics".to_owned())
            .spawn(move || {
                let mut project = Project::new(timeout);

                loop {
                    let job = {
                        let (state, ready) = &*shared;
                        let mut state = state.lock().expect("diagnostic state");

                        while state.pending.is_none() && !state.stopped {
                            state = ready.wait(state).expect("diagnostic state");
                        }

                        if state.stopped {
                            break;
                        }

                        progress.send_replace(Activity::Running);

                        state.pending.take().expect("pending analysis")
                    };

                    project.apply(&job.snapshot);

                    let result = analyze(
                        &job.paths,
                        &job.roots,
                        &job.encoding,
                        &mut project,
                        &job.options,
                        job.started,
                    );

                    drop(job.response.send(result));
                    let state = shared.0.lock().expect("diagnostic state");

                    if !state.stopped {
                        progress.send_replace(Activity::Idle);
                    }
                }
            })?;

        Ok(Self {
            state,
            handle: Mutex::new(Some(handle)),
            activity,
        })
    }

    pub(crate) fn submit(
        &self,
        snapshot: Snapshot,
        paths: Vec<PathBuf>,
        roots: Vec<PathBuf>,
        encoding: PositionEncodingKind,
        options: Options,
    ) -> oneshot::Receiver<io::Result<Report>> {
        let (response, receiver) = oneshot::channel();
        let (state, ready) = &*self.state;
        let mut state = state.lock().expect("diagnostic state");
        state.cancellation.cancel();

        if !state.stopped {
            state.cancellation = options.cancellation.clone();

            state.pending = Some(Job {
                snapshot,
                paths,
                roots,
                encoding,
                options,
                started: Instant::now(),
                response,
            });

            ready.notify_one();
        }

        receiver
    }

    pub(crate) fn activity(&self) -> watch::Receiver<Activity> {
        self.activity.subscribe()
    }

    pub(crate) fn stop(&self) {
        let (state, ready) = &*self.state;
        let mut state = state.lock().expect("diagnostic state");
        state.stopped = true;
        state.cancellation.cancel();
        state.pending.take();
        self.activity.send_replace(Activity::Stopped);
        ready.notify_one();
    }

    pub(crate) fn finish(&self) -> io::Result<()> {
        self.stop();

        if let Some(handle) = self.handle.lock().expect("diagnostic handle").take() {
            handle
                .join()
                .map_err(|_| io::Error::other("diagnostic worker failed"))?;
        }

        Ok(())
    }
}

impl Drop for Diagnostics {
    fn drop(&mut self) {
        if let Err(error) = self.finish() {
            tracing::error!(%error, "Diagnostic cleanup failed");
        }
    }
}

impl Session {
    pub(crate) async fn document_diagnostics(
        &self,
        parameters: DocumentDiagnosticParams,
    ) -> jsonrpc::Result<DocumentDiagnosticReport> {
        if parameters.identifier.is_some() {
            return Err(Error::invalid_params("unknown diagnostic provider"));
        }

        let locations = self.workspace.lock().expect("workspace lock").locations();
        let uri = parameters.text_document.uri.clone();

        let result = self
            .query(
                &parameters.text_document.uri,
                &self.queries,
                move |path, encoding, project, options, started| {
                    let mut diagnostics = analyze(
                        &[path.to_path_buf()],
                        &[],
                        encoding,
                        project,
                        options,
                        started,
                    )?;

                    let mut items = diagnostics.remove(path).unwrap_or_default();
                    remap(&locations, &mut items);
                    let result = report(&uri, items, parameters.previous_result_id.as_deref());
                    options.check(started)?;

                    Ok(result)
                },
            )
            .await?;

        Ok(match result {
            RelatedDocument::FullDocumentDiagnosticReport(report) => {
                RelatedFullDocumentDiagnosticReport {
                    full_document_diagnostic_report: report,
                    ..RelatedFullDocumentDiagnosticReport::default()
                }
                .into()
            }

            RelatedDocument::UnchangedDocumentDiagnosticReport(report) => {
                RelatedUnchangedDocumentDiagnosticReport {
                    unchanged_document_diagnostic_report: report,
                    ..RelatedUnchangedDocumentDiagnosticReport::default()
                }
                .into()
            }
        })
    }

    pub(crate) async fn workspace_diagnostics(
        &self,
        parameters: WorkspaceDiagnosticParams,
    ) -> jsonrpc::Result<WorkspaceDiagnosticReport> {
        if parameters.identifier.is_some() {
            return Err(Error::invalid_params("unknown diagnostic provider"));
        }

        let (snapshot, documents, locations, roots, encoding) = {
            let workspace = self.workspace.lock().expect("workspace lock");

            (
                workspace.snapshot.clone(),
                workspace.documents(),
                workspace.locations(),
                workspace.roots.iter().cloned().collect::<Vec<_>>(),
                workspace.encoding.clone(),
            )
        };

        let revision = snapshot.revision();

        let result = self
            .execute(snapshot, &self.queries, move |project, options, started| {
                let paths = documents.keys().cloned().collect::<Vec<_>>();
                let diagnostics = analyze(&paths, &roots, &encoding, project, options, started)?;
                let mut current = BTreeMap::new();

                for (path, mut diagnostics) in diagnostics {
                    options.check(started)?;
                    remap(&locations, &mut diagnostics);

                    let uri = locations
                        .get(&path)
                        .cloned()
                        .map_or_else(|| location::uri(&path), Ok)?;

                    current.insert(uri, (documents.get(&path).copied(), diagnostics));
                }

                let previous = parameters
                    .previous_result_ids
                    .into_iter()
                    .map(|previous| (previous.uri, previous.value))
                    .collect::<BTreeMap<_, _>>();

                for uri in previous.keys() {
                    current
                        .entry(uri.clone())
                        .or_insert_with(|| (None, Vec::new()));
                }

                let mut items = Vec::new();

                for (uri, (version, diagnostics)) in current {
                    options.check(started)?;

                    let diagnostic =
                        report(&uri, diagnostics, previous.get(&uri).map(String::as_str));

                    items.push(match diagnostic {
                        RelatedDocument::FullDocumentDiagnosticReport(report) => {
                            WorkspaceFullDocumentDiagnosticReport {
                                uri,
                                version,
                                full_document_diagnostic_report: report,
                            }
                            .into()
                        }

                        RelatedDocument::UnchangedDocumentDiagnosticReport(report) => {
                            WorkspaceUnchangedDocumentDiagnosticReport {
                                uri,
                                version,
                                unchanged_document_diagnostic_report: report,
                            }
                            .into()
                        }
                    });
                }

                options.check(started)?;

                Ok(WorkspaceDiagnosticReport { items })
            })
            .await?;

        if self
            .workspace
            .lock()
            .expect("workspace lock")
            .snapshot
            .revision()
            != revision
        {
            return Err(Error::content_modified());
        }

        Ok(result)
    }
}

pub(crate) fn failure(message: &str) -> Diagnostic {
    Diagnostic {
        severity: Some(DiagnosticSeverity::Error),
        source: Some("Instar".to_owned()),
        message: message.to_owned().into(),
        ..Diagnostic::default()
    }
}

pub(crate) fn remap(locations: &BTreeMap<PathBuf, Uri>, diagnostics: &mut [Diagnostic]) {
    for diagnostic in diagnostics {
        for related in diagnostic.related_information.iter_mut().flatten() {
            if let Ok(path) = related.location.uri.to_file_path()
                && let Some(uri) = locations.get(&path)
            {
                related.location.uri = uri.clone();
            }
        }
    }
}

pub(crate) fn convert(
    finding: lint::Diagnostic<Origin>,
    project: &mut Project,
    encoding: &PositionEncodingKind,
) -> io::Result<Diagnostic> {
    let range = location::convert(&finding.location, project, encoding)?.range;
    let mut related = Vec::new();

    for information in finding.related {
        related.push(DiagnosticRelatedInformation {
            location: location::convert(&information.location, project, encoding)?,
            message: information.message,
        });
    }

    let tags = match &finding.kind {
        lint::Kind::Rule(lint::Rule::UnusedVariable) => Some(vec![DiagnosticTag::Unnecessary]),
        lint::Kind::Rule(lint::Rule::Deprecated) => Some(vec![DiagnosticTag::Deprecated]),

        lint::Kind::Native { name, .. } => match name.as_str() {
            "LocalUnused" | "FunctionUnused" | "ImportUnused" | "UnreachableCode" => {
                Some(vec![DiagnosticTag::Unnecessary])
            }

            "DeprecatedGlobal" | "DeprecatedApi" => Some(vec![DiagnosticTag::Deprecated]),
            _ => None,
        },

        _ => None,
    };

    let code_description = if matches!(finding.kind, lint::Kind::Native { .. }) {
        Some(CodeDescription {
            href: "https://luau.org/lint/".parse().map_err(io::Error::other)?,
        })
    } else {
        None
    };

    let (source, code) = match finding.kind {
        lint::Kind::Analysis(kind) => ("Luau", kind.native_code().map(Into::into)),
        lint::Kind::Native { code, .. } => ("Luau", Some(code.into())),
        lint::Kind::Rule(rule) => ("Instar", Some(rule.to_string().into())),
    };

    Ok(Diagnostic {
        range,
        severity: Some(match finding.level {
            Level::Info => DiagnosticSeverity::Information,
            Level::Deny => DiagnosticSeverity::Error,
            _ => DiagnosticSeverity::Warning,
        }),
        code,
        code_description,
        tags,
        source: Some(source.to_owned()),
        message: finding.message.into(),
        related_information: (!related.is_empty()).then_some(related),
        ..Diagnostic::default()
    })
}

fn analyze(
    requested: &[PathBuf],
    roots: &[PathBuf],
    encoding: &PositionEncodingKind,
    project: &mut Project,
    options: &Options,
    started: Instant,
) -> io::Result<Report> {
    let mut report = Report::new();
    let mut paths = project.diagnostic_sources(roots, &options.remaining(started))?;
    paths.extend_from_slice(requested);
    paths.sort();
    paths.dedup();
    let entries = project.entries(&paths, &options.remaining(started))?;

    for path in &paths {
        report.insert(path.clone(), Vec::new());
    }

    if entries.is_empty() {
        return Ok(report);
    }

    let checked = project.check(&entries, &options.remaining(started))?;
    let linted = project.lint(&entries, &options.remaining(started))?;
    let completion = checked.completion.combine(linted.completion);

    let mut findings: Vec<lint::Diagnostic<Origin>> =
        checked.diagnostics.into_iter().map(Into::into).collect();

    for finding in linted.diagnostics {
        if !findings.iter().any(|existing| {
            existing.location == finding.location
                && existing.message == finding.message
                && matches!(
                    (&existing.kind, &finding.kind),
                    (lint::Kind::Analysis(_), lint::Kind::Analysis(_))
                )
        }) {
            findings.push(finding);
        }
    }

    for finding in findings {
        if finding.level == Level::Allow {
            continue;
        }

        let path = finding.location.module.source().to_path_buf();
        let diagnostic = convert(finding, project, encoding)?;

        let diagnostics = report.entry(path).or_default();

        if !diagnostics.contains(&diagnostic) {
            diagnostics.push(diagnostic);
        }
    }

    if let Completion::Incomplete(reason) = completion {
        for path in &paths {
            report
                .entry(path.clone())
                .or_default()
                .push(failure(&format!("Analysis incomplete: {}", reason.error())));
        }
    }

    options.check(started)?;

    Ok(report)
}

fn report(uri: &Uri, diagnostics: Vec<Diagnostic>, previous: Option<&str>) -> RelatedDocument {
    let mut identifier = DefaultHasher::new();
    uri.hash(&mut identifier);
    diagnostics.hash(&mut identifier);
    let result_identifier = format!("{:016x}", identifier.finish());

    if previous == Some(result_identifier.as_str()) {
        UnchangedDocumentDiagnosticReport {
            result_id: result_identifier,
        }
        .into()
    } else {
        FullDocumentDiagnosticReport {
            result_id: Some(result_identifier),
            items: diagnostics,
        }
        .into()
    }
}
