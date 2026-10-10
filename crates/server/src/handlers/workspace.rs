use std::{collections::HashMap, path::PathBuf};

use instar_format::FormatScope;

use instar_project::{
    ChangeBatch, CheckOverrides, CheckRequest, EditReport, EditRequest, ExtractionRequest,
    FileChange, FileRequest, FixMode, FixRequest, FormatMode, FormatOverrides, FormatRequest,
    ImportRequest, Input, Invocation, LintOverrides, LintRequest, OperationOverrides,
    ProjectChange, ProjectSettingsPatch, ProjectSnapshot, Query, QueryOutput, SettingsRequest,
};

use instar_shared::{Completion, ContextIdentity, SourceIdentity, SourceVersion, Span};
use serde_json::{Value, json};

use tower_lsp_server::{
    gen_lsp_types::{
        CreateFilesParams, DeleteFilesParams, DidChangeConfigurationParams,
        DidChangeWatchedFilesParams, DidChangeWorkspaceFoldersParams, ExecuteCommandParams, LspAny,
        MessageActionItem, MessageType, RenameFilesParams, ShowDocumentParams,
        TextDocumentContentParams, TextDocumentContentResult, WorkspaceEdit,
    },
    jsonrpc::{Error, Result},
};

use super::{assistance::lazy, editing::edit};

use crate::{
    Server,
    session::{RetainedResult, failure, path},
};

pub(crate) async fn folders(server: &Server, parameters: DidChangeWorkspaceFoldersParams) {
    let result = (|| {
        let added = parameters
            .event
            .added
            .into_iter()
            .map(|folder| path(&folder.uri))
            .collect::<Result<Vec<_>>>()?;

        let removed = parameters
            .event
            .removed
            .into_iter()
            .map(|folder| path(&folder.uri))
            .collect::<Result<Vec<_>>>()?;

        let mut state = server.state.lock().expect("session state");

        server
            .project
            .change(ChangeBatch {
                changes: vec![ProjectChange::WorkspaceRoots {
                    added: added.clone(),
                    removed: removed.clone(),
                }],
            })
            .map_err(|error| failure(&error))?;

        for root in removed {
            state.workspace_folders.remove(&root);
        }

        state.workspace_folders.extend(added);

        Ok(())
    })();

    server.schedule_diagnostics();
    server.report(result).await;
    server.refresh().await;
}

pub(crate) async fn watched(server: &Server, parameters: DidChangeWatchedFilesParams) {
    let result = invalidate(
        server,
        parameters
            .changes
            .into_iter()
            .map(|change| path(&change.uri))
            .collect::<Result<Vec<_>>>(),
    );

    server.schedule_diagnostics();
    server.report(result).await;
    server.refresh().await;
}

pub(crate) async fn configuration(server: &Server, parameters: DidChangeConfigurationParams) {
    let roots = server
        .state
        .lock()
        .expect("session state")
        .workspace_folders
        .iter()
        .cloned()
        .collect::<Vec<_>>();

    let settings = parameters
        .settings
        .get("instar")
        .cloned()
        .unwrap_or(parameters.settings);

    let values = vec![settings; roots.len().max(1)];
    let result = configuration_overlays(server, &roots, values);
    server.schedule_diagnostics();
    server.report(result).await;
    server.refresh().await;
}

pub(crate) fn configuration_overlays(
    server: &Server,
    roots: &[PathBuf],
    values: Vec<Value>,
) -> Result<()> {
    if values.len() != roots.len().max(1) {
        return Err(Error::invalid_params(
            "configuration response count does not match requested scopes",
        ));
    }

    let snapshot = server.project.snapshot();

    let contexts = if roots.is_empty() {
        vec![ContextIdentity::new(0)]
    } else {
        roots.iter().map(|root| snapshot.context_at(root)).collect()
    };

    let changes = contexts
        .into_iter()
        .zip(values)
        .map(|(context, value)| {
            let patch: Option<ProjectSettingsPatch> = if value.is_null() {
                None
            } else {
                Some(
                    serde_json::from_value(value)
                        .map_err(|error| Error::invalid_params(error.to_string()))?,
                )
            };

            if let Some(patch) = &patch {
                patch.validate().map_err(|error| failure(&error))?;
            }

            Ok(ProjectChange::ClientConfiguration {
                context,
                patch: patch.map(Box::new),
            })
        })
        .collect::<Result<Vec<_>>>()?;

    server
        .project
        .change(ChangeBatch { changes })
        .map_err(|error| failure(&error))?;

    Ok(())
}

pub(crate) async fn created(server: &Server, parameters: CreateFilesParams) {
    let result = invalidate(
        server,
        parameters
            .files
            .into_iter()
            .map(|file| path(&file.uri))
            .collect(),
    );

    server.schedule_diagnostics();
    server.report(result).await;
    server.refresh().await;
}

pub(crate) async fn deleted(server: &Server, parameters: DeleteFilesParams) {
    let result = invalidate(
        server,
        parameters
            .files
            .into_iter()
            .map(|file| path(&file.uri))
            .collect(),
    );

    server.schedule_diagnostics();
    server.report(result).await;
    server.refresh().await;
}

pub(crate) async fn renamed(server: &Server, parameters: RenameFilesParams) {
    let result = (|| {
        let movements = parameters
            .files
            .into_iter()
            .map(|file| Ok((path(&file.old_uri)?, path(&file.new_uri)?)))
            .collect::<Result<Vec<_>>>()?;

        let paths = movements
            .iter()
            .flat_map(|(old, new)| [old.clone(), new.clone()])
            .collect();

        let changes = vec![ProjectChange::Invalidate { paths }];

        server
            .state
            .lock()
            .expect("session state")
            .renamed(&server.project, &movements, changes)
    })();

    server.schedule_diagnostics();
    server.report(result).await;
    server.refresh().await;
}

pub(crate) async fn will_create(
    server: &Server,
    parameters: CreateFilesParams,
) -> Result<Option<WorkspaceEdit>> {
    files(
        server,
        parameters
            .files
            .into_iter()
            .map(|file| path(&file.uri).map(FileChange::Create))
            .collect(),
    )
    .await
}

pub(crate) async fn will_delete(
    server: &Server,
    parameters: DeleteFilesParams,
) -> Result<Option<WorkspaceEdit>> {
    files(
        server,
        parameters
            .files
            .into_iter()
            .map(|file| path(&file.uri).map(FileChange::Delete))
            .collect(),
    )
    .await
}

pub(crate) async fn will_rename(
    server: &Server,
    parameters: RenameFilesParams,
) -> Result<Option<WorkspaceEdit>> {
    files(
        server,
        parameters
            .files
            .into_iter()
            .map(|file| {
                Ok(FileChange::Rename {
                    old: path(&file.old_uri)?,
                    new: path(&file.new_uri)?,
                })
            })
            .collect(),
    )
    .await
}

pub(crate) async fn content(
    server: &Server,
    parameters: TextDocumentContentParams,
) -> Result<TextDocumentContentResult> {
    let options = server.options.child();
    let (snapshot, source) = server.capture(&parameters.uri, &options)?;

    if snapshot.is_declaration(source.version) {
        return Ok(TextDocumentContentResult {
            text: source.text.to_string(),
        });
    }

    let report = server
        .query(
            snapshot,
            Query::Content(source.version.identity),
            None,
            &options,
        )
        .await?;

    let QueryOutput::Content(Some(source)) = report.output else {
        return Err(Error::invalid_params("document content is unavailable"));
    };

    Ok(TextDocumentContentResult {
        text: source.text.to_string(),
    })
}

pub(crate) async fn execute(
    server: &Server,
    parameters: ExecuteCommandParams,
) -> Result<Option<LspAny>> {
    if !crate::session::COMMANDS.contains(&parameters.command.as_str()) {
        return Err(Error::invalid_params("unknown command"));
    }

    let arguments = parameters.arguments.unwrap_or_default();

    let [arguments] = arguments.as_slice() else {
        return Err(Error::invalid_params(
            "command requires one revision identity and typed controls",
        ));
    };

    if arguments.get("identity").is_some() {
        let record = lazy(server, Some(arguments), "command")?;

        let RetainedResult::Command { command, edit } = record.result else {
            return Err(Error::internal_error());
        };

        if command != parameters.command {
            return Err(Error::invalid_params("identity belongs to another command"));
        }

        if let Some(proposal) = edit {
            let edit = server.workspace_edit(&record.snapshot, &proposal, true)?;

            return serde_json::to_value(server.apply_edit(parameters.command, edit).await?)
                .map(Some)
                .map_err(|_| Error::internal_error());
        }
    }

    let (snapshot, source, controls) = command_source(server, arguments, &parameters.command)?;

    let invocation = Invocation {
        inputs: vec![Input::Source(source.identity)],
        ..Invocation::default()
    };

    let options = server.options.child();
    let token = parameters.work_done_progress_params.work_done_token;

    match parameters.command.as_str() {
        "instar.check" => {
            let task = server.project.check(
                CheckRequest {
                    invocation,
                    overrides: CheckOverrides::default(),
                },
                options.clone(),
            );

            let report = server
                .task(task, &options, token, "Checking Luau source")
                .await?;

            if !server
                .state
                .lock()
                .expect("session state")
                .current(&server.project, &report.inputs)
            {
                return Err(Error::content_modified());
            }

            if report.completion != Completion::Complete {
                return Err(failure(&report.completion.error()));
            }

            server
                .client
                .log_message(
                    MessageType::Info,
                    format!(
                        "Checked {} sources: {} findings",
                        report.sources.len(),
                        report.findings.len()
                    ),
                )
                .await;

            if report.failed {
                reveal_findings(server, &report.snapshot, source).await?;
            }

            Ok(Some(
                json!({"failed":report.failed,"findings":report.findings.len(),"completion":report.completion}),
            ))
        }

        "instar.lint" => {
            let task = server.project.lint(
                LintRequest {
                    invocation,
                    ..LintRequest::default()
                },
                options.clone(),
            );

            let report = server
                .task(task, &options, token, "Linting Luau source")
                .await?;

            if !server
                .state
                .lock()
                .expect("session state")
                .current(&server.project, &report.inputs)
            {
                return Err(Error::content_modified());
            }

            if report.completion != Completion::Complete {
                return Err(failure(&report.completion.error()));
            }

            server
                .client
                .show_message(
                    if report.failed {
                        MessageType::Warning
                    } else {
                        MessageType::Info
                    },
                    format!("Lint: {} findings", report.findings.len()),
                )
                .await;

            Ok(Some(
                json!({"failed":report.failed,"findings":report.findings.len(),"completion":report.completion}),
            ))
        }

        "instar.showSettings" => {
            let task = server.project.settings(
                SettingsRequest {
                    invocation,
                    overrides: OperationOverrides::Check(CheckOverrides::default()),
                    snapshot: Some(snapshot),
                },
                options.clone(),
            );

            let report = server
                .task(task, &options, token, "Resolving Luau settings")
                .await?;

            if !server
                .state
                .lock()
                .expect("session state")
                .current(&server.project, &report.inputs)
            {
                return Err(Error::content_modified());
            }

            if report.completion != Completion::Complete {
                return Err(failure(&report.completion.error()));
            }

            let settings = report
                .settings
                .iter()
                .map(|settings| {
                    serde_json::to_value(&settings.settings).map_err(|_| Error::internal_error())
                })
                .collect::<Result<Vec<_>>>()?;

            Ok(Some(
                json!({"settings":settings,"schema":report.schema_identity}),
            ))
        }

        "instar.format" => {
            let task = server.project.format(
                FormatRequest {
                    invocation,
                    scope: FormatScope::Whole,
                    overrides: FormatOverrides::default(),
                    mode: FormatMode::Preview,
                    snapshot: Some(snapshot),
                },
                options.clone(),
            );

            let report = server
                .task(task, &options, token, "Formatting Luau source")
                .await?;

            if report.completion != Completion::Complete {
                return Err(failure(&report.completion.error()));
            }

            if !server
                .state
                .lock()
                .expect("session state")
                .current(&server.project, &report.inputs)
            {
                return Err(Error::content_modified());
            }

            let mut result = Vec::new();

            for formatted in report.results {
                if let Some(proposal) = formatted.proposal {
                    let edit = server.workspace_edit(&report.snapshot, &proposal, true)?;

                    result.push(
                        server
                            .apply_edit("Format Luau source".to_owned(), edit)
                            .await?,
                    );
                } else if formatted.result.completion != Completion::Complete {
                    return Err(failure(&formatted.result.completion.error()));
                }
            }

            Ok(Some(
                serde_json::to_value(result).map_err(|_| Error::internal_error())?,
            ))
        }

        "instar.organizeImports" | "instar.extractLiteral" => {
            let request = if parameters.command == "instar.organizeImports" {
                EditRequest::Imports(ImportRequest { snapshot, source })
            } else {
                let selected = controls.get("range").ok_or_else(|| {
                    Error::invalid_params("literal extraction requires a byte range")
                })?;

                let span: Span = serde_json::from_value(selected.clone())
                    .map_err(|error| Error::invalid_params(error.to_string()))?;

                span.validate(
                    server
                        .project
                        .snapshot()
                        .source(source.identity)
                        .ok_or_else(Error::content_modified)?,
                )
                .map_err(|error| failure(&error))?;

                EditRequest::Extract(ExtractionRequest {
                    snapshot,
                    source,
                    range: span,
                })
            };

            apply_report(
                server,
                edit(server, request, token, &options).await?,
                parameters.command,
            )
            .await
        }

        "instar.applyFix" | "instar.fixAll" => {
            let task = server.project.lint(
                LintRequest {
                    invocation,
                    overrides: LintOverrides::default(),
                    fixes: FixMode::Preview,
                    fix_rules: None,
                },
                options.clone(),
            );

            let report = server
                .task(task, &options, token, "Preparing Luau fixes")
                .await?;

            if !server
                .state
                .lock()
                .expect("session state")
                .current(&server.project, &report.inputs)
            {
                return Err(Error::content_modified());
            }

            if report.completion != Completion::Complete {
                return Err(failure(&report.completion.error()));
            }

            let fixes = if parameters.command == "instar.applyFix" {
                let identity = controls.get("fix").and_then(Value::as_u64).ok_or_else(|| {
                    Error::invalid_params("apply-fix requires a proposal identity")
                })?;

                report
                    .fixes
                    .into_iter()
                    .filter(|fix| fix.proposal.identity.get() == identity)
                    .collect::<Vec<_>>()
            } else {
                report.fixes
            };

            if fixes.is_empty() {
                return Err(Error::invalid_params("no current matching fixes"));
            }

            apply_report(
                server,
                edit(
                    server,
                    EditRequest::Fixes(FixRequest {
                        snapshot: report.snapshot,
                        fixes,
                    }),
                    None,
                    &options,
                )
                .await?,
                parameters.command,
            )
            .await
        }

        _ => Err(Error::invalid_params("unknown command")),
    }
}

async fn files(server: &Server, changes: Result<Vec<FileChange>>) -> Result<Option<WorkspaceEdit>> {
    let options = server.options.child();

    let report = edit(
        server,
        EditRequest::Files(FileRequest {
            snapshot: server.project.snapshot(),
            changes: changes?,
        }),
        None,
        &options,
    )
    .await?;

    let validated = report.validated.as_ref().ok_or_else(|| {
        Error::invalid_params("file preflight did not establish required guarantees")
    })?;

    server
        .workspace_edit(&report.snapshot, validated, false)
        .map(Some)
}

fn invalidate(server: &Server, paths: Result<Vec<PathBuf>>) -> Result<()> {
    let changes = vec![ProjectChange::Invalidate { paths: paths? }];

    server
        .project
        .change(ChangeBatch { changes })
        .map_err(|error| failure(&error))?;

    Ok(())
}

fn command_source(
    server: &Server,
    arguments: &Value,
    command: &str,
) -> Result<(ProjectSnapshot, SourceVersion, Value)> {
    if arguments.get("identity").is_some() {
        let record = lazy(server, Some(arguments), "command")?;

        let RetainedResult::Command {
            command: retained_command,
            ..
        } = record.result
        else {
            return Err(Error::internal_error());
        };

        if retained_command != command {
            return Err(Error::invalid_params("identity belongs to another command"));
        }

        return Ok((
            record.snapshot,
            record
                .source
                .ok_or_else(|| Error::invalid_params("command requires a source"))?,
            Value::Null,
        ));
    }

    let identity = arguments
        .get("source")
        .and_then(Value::as_u64)
        .map(SourceIdentity::new)
        .ok_or_else(|| Error::invalid_params("missing source identity"))?;

    let revision = arguments
        .get("revision")
        .and_then(Value::as_u64)
        .ok_or_else(|| Error::invalid_params("missing source revision"))?;

    let source = SourceVersion { identity, revision };
    let state = server.state.lock().expect("session state");

    if !state
        .documents
        .values()
        .any(|document| document.revision == source && document.synchronized && document.writable)
    {
        return Err(Error::invalid_params(
            "command source is not a current writable synchronized document",
        ));
    }

    let snapshot = server.project.snapshot();

    if snapshot
        .source(identity)
        .is_none_or(|current| current.version != source)
    {
        return Err(Error::content_modified());
    }

    Ok((snapshot, source, arguments.clone()))
}

async fn apply_report(server: &Server, report: EditReport, label: String) -> Result<Option<Value>> {
    let proposal = report.validated.as_ref().ok_or_else(|| {
        Error::invalid_params("command did not establish required edit guarantees")
    })?;

    let edit = server.workspace_edit(&report.snapshot, proposal, true)?;
    let result = server.apply_edit(label, edit).await?;

    serde_json::to_value(result)
        .map(Some)
        .map_err(|_| Error::internal_error())
}

async fn reveal_findings(
    server: &Server,
    snapshot: &ProjectSnapshot,
    source: SourceVersion,
) -> Result<()> {
    let choice = server
        .client_request(server.client.show_message_request(
            MessageType::Warning,
            "Checking found source errors",
            Some(vec![MessageActionItem {
                title: "Show source".to_owned(),
                properties: HashMap::new(),
            }]),
        ))
        .await?;

    if choice.is_some_and(|choice| choice.title == "Show source")
        && server
            .state
            .lock()
            .expect("session state")
            .capabilities
            .client
            .window
            .as_ref()
            .and_then(|window| window.show_document.as_ref())
            .is_some_and(|document| document.support)
    {
        if server
            .project
            .snapshot()
            .source(source.identity)
            .is_none_or(|current| current.version != source)
        {
            return Err(Error::content_modified());
        }

        let uri = server
            .state
            .lock()
            .expect("session state")
            .uri(snapshot, source.identity)?;

        let accepted = server
            .client_request(server.client.show_document(ShowDocumentParams {
                uri,
                external: Some(false),
                take_focus: Some(true),
                selection: None,
            }))
            .await?;

        if !accepted {
            return Err(Error::invalid_params("client declined to show the source"));
        }
    }

    Ok(())
}
