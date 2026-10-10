use std::{
    collections::BTreeSet,
    env, fs,
    io::{self, Read},
    path::{self, Path, PathBuf},
    sync::{Arc, Mutex, mpsc},
    thread,
    time::{Duration, Instant},
};

use eyre::Result;
use instar_format::FormatScope;
use instar_lint::RuleIdentity;

use instar_project::{
    CheckOverrides, CheckRequest, ExplanationSelection, FixMode, FormatMode, FormatOverrides,
    FormatRequest, Input, Invocation, LintOverrides, LintRequest, Operation, OperationOverrides,
    Project, SettingsRequest, WatchCommandReport, WatchOperation, WatchRequest,
};

use instar_shared::{
    Completion, DiagnosticCategory, EventPayload, OperationFailure, Options, Severity, Span,
    process,
};

use serde_json::{Value, json};
use tracing::Level;
use tracing_subscriber::fmt;

use super::arguments::{
    Arguments, CheckArguments, CheckingMode, Command, DiagnosticArguments, DiagnosticSeverity,
    FormatArguments, ImportOrder, Indentation, InputArguments, LineEnding, LintArguments,
    OperationKind, QuoteStyle, ReportStyle,
};

use crate::{
    completion,
    report::{CommandReport, Reporter},
};

pub(crate) struct CommandOutcome {
    pub(crate) status: CommandStatus,
    pub(crate) completion: Completion,
}

#[derive(Clone, Copy)]
pub(crate) enum CommandStatus {
    Clean,
    Findings,
    InvalidInvocation,
    OperationalFailure,
    Incomplete,
    UnsuccessfulFormatting,
}

impl CommandStatus {
    pub(crate) fn code(self) -> u8 {
        match self {
            Self::Clean => 0,
            Self::Findings => 1,
            Self::InvalidInvocation => 2,
            Self::OperationalFailure => 3,
            Self::Incomplete => 4,
            Self::UnsuccessfulFormatting => 5,
        }
    }
}

pub(crate) async fn run(arguments: Arguments) -> Result<CommandOutcome> {
    let deadline = Instant::now()
        .checked_add(Duration::from_secs(arguments.execution.timeout))
        .ok_or_else(|| invalid("timeout exceeds supported clock range"))?;

    let mut options = Options::new(Some(deadline));

    if let Some(jobs) = arguments.execution.jobs {
        options.jobs = jobs;
    }

    process::set_host(&env::current_exe()?)?;
    let project = Arc::new(Project::new(vec![env::current_dir()?]));
    let cancellation = options.cancellation.clone();
    let signal_project = Arc::clone(&project);

    let signal = tokio::spawn(async move {
        match tokio::signal::ctrl_c().await {
            Ok(()) => {
                cancellation.cancel();
                signal_project.stop();
            }

            Err(error) => {
                tracing::error!(%error, "Cannot observe interrupt signal");
                cancellation.cancel();
                signal_project.stop();
            }
        }
    });

    let result = if matches!(arguments.command, Command::Server) {
        serve(&arguments, Arc::clone(&project), options.clone()).await
    } else {
        execute(arguments, &project, &mut options).await
    };

    signal.abort();
    let cleanup = project.finish().await;

    match (result, cleanup) {
        (Ok(outcome), Ok(())) => Ok(outcome),
        (Err(error), Ok(())) => Err(error),
        (Ok(_), Err(error)) => Err(error.into()),

        (Err(error), Err(cleanup)) => {
            Err(error.wrap_err(format!("project cleanup failed: {cleanup}")))
        }
    }
}

async fn execute(
    arguments: Arguments,
    project: &Arc<Project>,
    options: &mut Options,
) -> Result<CommandOutcome> {
    let Arguments {
        command,
        execution,
        reporting,
    } = arguments;

    let json_output = reporting.json || reporting.style == ReportStyle::Json;
    let report_destination = reporting.destination.is_some();
    let reporter = Arc::new(Mutex::new(Reporter::new(reporting)?));

    if execution.verbose {
        reporter.lock().expect("reporter lock").trace()?;
    }

    let observer_reporter = Arc::clone(&reporter);
    let observer_project = Arc::clone(project);
    let output_cancellation = options.cancellation.clone();

    options.observer = Some(Arc::new(move |event| {
        let mut reporter = observer_reporter.lock().expect("reporter lock");
        reporter.capture(&observer_project.snapshot());

        if reporter.output_failed() {
            output_cancellation.cancel();

            return;
        }

        if let Err(error) = reporter.event(&event) {
            reporter.failed_output(error);
            output_cancellation.cancel();
        }
    }));

    if let Command::Completions { shell } = &command {
        if execution.watch || execution.stdin_filepath.is_some() {
            return Err(invalid("completions does not accept watch or stdin context").into());
        }

        let output = completion::generate(shell)?;
        let mut reporter = reporter.lock().expect("reporter lock");
        reporter.document(&output)?;
        reporter.publish()?;

        return Ok(CommandOutcome {
            status: CommandStatus::Clean,
            completion: Completion::Complete,
        });
    }

    let request = match command {
        Command::Check(arguments) => {
            let overrides = check_overrides(&arguments)?;

            let invocation = invocation(
                &arguments.inputs,
                execution.stdin_filepath.as_deref(),
                options,
            )
            .await?;

            WatchOperation::Check(CheckRequest {
                invocation,
                overrides,
            })
        }

        Command::Format(arguments) => {
            let stdin = arguments
                .inputs
                .files
                .iter()
                .any(|path| path == Path::new("-"));

            let mode = if arguments.write {
                FormatMode::Write
            } else if arguments.preview {
                FormatMode::Preview
            } else if arguments.source || (stdin && !arguments.check && !json_output) {
                FormatMode::Source
            } else {
                FormatMode::Check
            };

            if arguments.source && arguments.write && !stdin {
                return Err(invalid("combined --source and --write requires stdin").into());
            }

            if (matches!(mode, FormatMode::Source) || (stdin && arguments.write))
                && json_output
                && !report_destination
            {
                return Err(invalid(
                    "JSON source output requires --output for the report, or --check",
                )
                .into());
            }

            if execution.watch && (matches!(mode, FormatMode::Source) || (stdin && arguments.write))
            {
                return Err(
                    invalid("watch cannot be combined with formatted-source output").into(),
                );
            }

            let scope = format_scope(&arguments)?;
            let overrides = format_overrides(&arguments)?;

            let invocation = invocation(
                &arguments.inputs,
                execution.stdin_filepath.as_deref(),
                options,
            )
            .await?;

            WatchOperation::Format(FormatRequest {
                invocation,
                scope,
                overrides,
                mode,
                snapshot: None,
            })
        }

        Command::Lint(arguments) => {
            let stdin = arguments
                .inputs
                .files
                .iter()
                .any(|path| path == Path::new("-"));

            if arguments.fix_stdin && !stdin {
                return Err(invalid("--fix-stdin requires '-' as an input").into());
            }

            if !arguments.fix_rules.is_empty()
                && !(arguments.fix || arguments.fix_preview || arguments.fix_stdin)
            {
                return Err(invalid("--fix-rule requires a fix mode").into());
            }

            if stdin && (arguments.fix || arguments.fix_stdin) && json_output && !report_destination
            {
                return Err(invalid("JSON fixed stdin requires --output for the report").into());
            }

            if execution.watch && stdin && (arguments.fix || arguments.fix_stdin) {
                return Err(invalid("watch cannot be combined with stdin fixing").into());
            }

            let overrides = lint_overrides(&arguments)?;

            let fixes = if arguments.fix {
                FixMode::Write
            } else if arguments.fix_stdin {
                FixMode::Stdin
            } else if arguments.fix_preview {
                FixMode::Preview
            } else {
                FixMode::None
            };

            let fix_rules = if arguments.fix_rules.is_empty() {
                None
            } else {
                Some(
                    arguments
                        .fix_rules
                        .iter()
                        .map(|name| rule(name))
                        .collect::<io::Result<BTreeSet<_>>>()?,
                )
            };

            let invocation = invocation(
                &arguments.inputs,
                execution.stdin_filepath.as_deref(),
                options,
            )
            .await?;

            WatchOperation::Lint(LintRequest {
                invocation,
                overrides,
                fixes,
                fix_rules,
            })
        }

        Command::Settings(arguments) => {
            if execution.watch {
                return Err(invalid("settings does not accept watch").into());
            }

            let value = overrides(&arguments.inputs)?;

            let overrides = match arguments.operation {
                OperationKind::Check => {
                    OperationOverrides::Check(serde_json::from_value(value).map_err(invalid_json)?)
                }

                OperationKind::Format => {
                    OperationOverrides::Format(serde_json::from_value(value).map_err(invalid_json)?)
                }

                OperationKind::Lint => {
                    OperationOverrides::Lint(serde_json::from_value(value).map_err(invalid_json)?)
                }
            };

            let invocation = invocation(
                &arguments.inputs,
                execution.stdin_filepath.as_deref(),
                options,
            )
            .await?;

            let report = project
                .settings(
                    SettingsRequest {
                        invocation,
                        overrides,
                        snapshot: None,
                    },
                    options.clone(),
                )
                .await;

            reporter.lock().expect("reporter lock").check_output()?;
            let report = report?;

            return finish(&reporter, CommandReport::Settings(report));
        }

        Command::Schema(arguments) => {
            if execution.watch || execution.stdin_filepath.is_some() {
                return Err(invalid("schema does not accept watch or stdin context").into());
            }

            return finish(
                &reporter,
                CommandReport::Schema(Project::schema(operation(arguments.operation))),
            );
        }

        Command::Rules(arguments) => {
            if execution.watch || execution.stdin_filepath.is_some() {
                return Err(invalid("rules does not accept watch or stdin context").into());
            }

            let mut rules = Project::rules();

            if let Some(name) = arguments.rule {
                let selected = rule(&name)?;
                rules.retain(|description| description.identity == selected);
            }

            return finish(&reporter, CommandReport::Rules(rules));
        }

        Command::Completions { .. } | Command::Server => {
            unreachable!("command dispatched before operation translation")
        }
    };

    let explanations = match &request {
        WatchOperation::Check(request) => &request.invocation.explanations,
        WatchOperation::Format(request) => &request.invocation.explanations,
        WatchOperation::Lint(request) => &request.invocation.explanations,
    };

    reporter
        .lock()
        .expect("reporter lock")
        .explain(explanations.clone());

    if !execution.watch {
        let report = operation_report(project, request, options.clone()).await;
        reporter.lock().expect("reporter lock").check_output()?;

        return match report {
            Ok(report) => finish(&reporter, report),

            Err(failure)
                if !failure.diagnostics.is_empty()
                    || !failure.completed_sources.is_empty()
                    || !failure.completed_edits.is_empty() =>
            {
                reporter
                    .lock()
                    .expect("reporter lock")
                    .capture(&project.snapshot());

                finish(&reporter, CommandReport::Failure(failure))
            }

            Err(failure) => Err(failure.into()),
        };
    }

    let mut outcome = CommandOutcome {
        status: CommandStatus::Clean,
        completion: Completion::Complete,
    };

    let mut published = false;

    loop {
        let watched = project
            .watch(
                WatchRequest {
                    operation: request.clone(),
                    poll_interval: Duration::from_millis(100),
                },
                options.clone(),
            )
            .await;

        reporter.lock().expect("reporter lock").check_output()?;

        let mut watched = match watched {
            Ok(report) => report,

            Err(failure)
                if published
                    || !failure.diagnostics.is_empty()
                    || !failure.completed_sources.is_empty()
                    || !failure.completed_edits.is_empty() =>
            {
                reporter
                    .lock()
                    .expect("reporter lock")
                    .capture(&project.snapshot());

                let terminal = finish(&reporter, CommandReport::Failure(failure))?;
                outcome.status = precedence(outcome.status, terminal.status);
                outcome.completion = outcome.completion.combine(terminal.completion);

                return Ok(outcome);
            }

            Err(failure) => return Err(failure.into()),
        };

        reporter
            .lock()
            .expect("reporter lock")
            .events(std::mem::take(&mut watched.events));

        for iteration in std::mem::take(&mut watched.historical) {
            reporter.lock().expect("reporter lock").historical(
                &iteration.snapshot,
                iteration.changed,
                watch_report(iteration.report, &request),
            )?;

            published = true;
        }

        for iteration in watched.iterations.drain(..) {
            if !project.current(iteration.report.inputs()) {
                reporter.lock().expect("reporter lock").historical(
                    &iteration.snapshot,
                    iteration.changed,
                    watch_report(iteration.report, &request),
                )?;

                published = true;
                continue;
            }

            reporter
                .lock()
                .expect("reporter lock")
                .watch(&iteration.snapshot, iteration.changed)?;

            let report = watch_report(iteration.report, &request);
            let iteration = finish(&reporter, report)?;
            outcome.status = precedence(outcome.status, iteration.status);
            outcome.completion = outcome.completion.combine(iteration.completion);
            published = true;
        }

        outcome.completion = outcome.completion.combine(watched.completion.clone());

        if watched.failure.is_some() || outcome.completion != Completion::Complete {
            watched.completion = outcome.completion.clone();
            let terminal = finish(&reporter, CommandReport::Watch(watched))?;
            outcome.status = precedence(outcome.status, terminal.status);

            return Ok(outcome);
        }
    }
}

async fn operation_report(
    project: &Project,
    request: WatchOperation,
    options: Options,
) -> std::result::Result<CommandReport, OperationFailure> {
    match request {
        WatchOperation::Check(request) => project
            .check(request, options)
            .await
            .map(CommandReport::Check),

        WatchOperation::Format(request) => {
            let mode = request.mode;

            project
                .format(request, options)
                .await
                .map(|report| CommandReport::Format(report, mode))
        }

        WatchOperation::Lint(request) => {
            let fixes = request.fixes;

            project
                .lint(request, options)
                .await
                .map(|report| CommandReport::Lint(report, fixes))
        }
    }
}

fn watch_report(report: WatchCommandReport, request: &WatchOperation) -> CommandReport {
    match report {
        WatchCommandReport::Check(report) => CommandReport::Check(report),

        WatchCommandReport::Format(report) => CommandReport::Format(
            report,
            match request {
                WatchOperation::Format(request) => request.mode,
                _ => unreachable!("format watch request"),
            },
        ),

        WatchCommandReport::Lint(report) => CommandReport::Lint(
            report,
            match request {
                WatchOperation::Lint(request) => request.fixes,
                _ => unreachable!("lint watch request"),
            },
        ),
    }
}

fn finish(reporter: &Mutex<Reporter>, report: CommandReport) -> Result<CommandOutcome> {
    let status = report.status();
    let completion = report.completion().clone();
    let mut reporter = reporter.lock().expect("reporter lock");
    reporter.finish(&report)?;
    reporter.publish()?;

    Ok(CommandOutcome { status, completion })
}

fn precedence(current: CommandStatus, next: CommandStatus) -> CommandStatus {
    fn priority(status: CommandStatus) -> u8 {
        match status {
            CommandStatus::Clean => 0,
            CommandStatus::Findings => 1,
            CommandStatus::UnsuccessfulFormatting => 2,
            CommandStatus::Incomplete => 3,
            CommandStatus::OperationalFailure => 4,
            CommandStatus::InvalidInvocation => 5,
        }
    }

    if priority(next) > priority(current) {
        next
    } else {
        current
    }
}

async fn serve(
    arguments: &Arguments,
    project: Arc<Project>,
    mut options: Options,
) -> Result<CommandOutcome> {
    if arguments.execution.watch
        || arguments.execution.stdin_filepath.is_some()
        || arguments.reporting.destination.is_some()
        || arguments.reporting.streaming
    {
        return Err(invalid(
            "server does not accept command reporting, watch, or stdin context controls",
        )
        .into());
    }

    let subscriber = fmt()
        .with_writer(io::stderr)
        .with_ansi(false)
        .with_max_level(if arguments.execution.verbose {
            Level::DEBUG
        } else {
            Level::WARN
        });

    let result = if arguments.reporting.json || arguments.reporting.style == ReportStyle::Json {
        subscriber.json().try_init()
    } else {
        subscriber.try_init()
    };

    result.map_err(|error| eyre::eyre!("{error}"))?;

    options.observer = Some(Arc::new(|event| {
        if let EventPayload::Progress(progress) = event.payload {
            tracing::debug!(phase = progress.phase, "Analysis progress");
        }
    }));

    let result = instar_server::stdio(project, options.clone()).await?;
    result.cleanup?;
    let completion = options.interrupted().unwrap_or(Completion::Complete);

    let status = if completion != Completion::Complete {
        CommandStatus::Incomplete
    } else if result.shutdown_requested && result.exit_received {
        CommandStatus::Clean
    } else {
        CommandStatus::OperationalFailure
    };

    Ok(CommandOutcome { status, completion })
}

async fn invocation(
    arguments: &InputArguments,
    context: Option<&Path>,
    options: &Options,
) -> io::Result<Invocation> {
    options.check()?;

    let stdin_count = arguments
        .files
        .iter()
        .filter(|path| *path == Path::new("-"))
        .count();

    if stdin_count > 1 {
        return Err(invalid("stdin may be selected only once"));
    }

    if stdin_count == 0 && context.is_some() {
        return Err(invalid("--stdin-filepath requires '-' as an input"));
    }

    let text = if stdin_count == 1 {
        Some(stdin(options).await?)
    } else {
        None
    };

    let base = env::current_dir()?;
    let context = context.map(path::absolute).transpose()?;
    let mut inputs = Vec::new();

    for input in &arguments.files {
        options.check()?;

        if input == Path::new("-") {
            inputs.push(Input::Stdin {
                text: Arc::from(text.as_deref().expect("selected stdin was read")),
                context: context.clone(),
            });
        } else if input.to_string_lossy().contains(['*', '?', '[']) && fs::metadata(input).is_err()
        {
            let (base, pattern) = if input.is_absolute() {
                let base = input
                    .components()
                    .take_while(|component| {
                        !component
                            .as_os_str()
                            .to_string_lossy()
                            .contains(['*', '?', '['])
                    })
                    .collect::<PathBuf>();

                let pattern = input
                    .strip_prefix(&base)
                    .map_err(|_| invalid("absolute pattern has no selection base"))?
                    .to_path_buf();

                (base, pattern)
            } else {
                (base.clone(), input.clone())
            };

            inputs.push(Input::Pattern {
                base,
                pattern: pattern
                    .to_str()
                    .ok_or_else(|| invalid("patterns must be UTF-8"))?
                    .replace('\\', "/"),
            });
        } else {
            inputs.push(Input::Path(path::absolute(input)?));
        }
    }

    Ok(Invocation {
        inputs,
        project_root: arguments
            .project_root
            .as_deref()
            .map(path::absolute)
            .transpose()?,
        configuration: arguments
            .configuration
            .as_deref()
            .map(path::absolute)
            .transpose()?,
        explanations: ExplanationSelection {
            selection: arguments.explain_selection,
            settings: arguments.explain_settings,
            dependencies: arguments.explain_dependencies,
        },
    })
}

async fn stdin(options: &Options) -> io::Result<String> {
    options.check()?;
    let (sender, receiver) = mpsc::sync_channel(1);

    thread::Builder::new()
        .name("stdin".to_owned())
        .spawn(move || {
            let mut input = io::stdin().lock();
            let mut buffer = [0; 8192];

            loop {
                let result = match input.read(&mut buffer) {
                    Ok(count) => Ok(buffer[..count].to_vec()),
                    Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                    Err(error) => Err(error),
                };

                let finished = result.as_ref().map_or(true, Vec::is_empty);

                if sender.send(result).is_err() || finished {
                    break;
                }
            }
        })?;

    let mut bytes = Vec::new();

    loop {
        options.check()?;

        match receiver.try_recv() {
            Ok(result) => {
                let chunk = result?;

                if chunk.is_empty() {
                    return String::from_utf8(bytes)
                        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error));
                }

                bytes.extend(chunk);
            }

            Err(mpsc::TryRecvError::Empty) => {
                tokio::time::sleep(
                    options
                        .remaining()
                        .unwrap_or(Duration::from_millis(10))
                        .min(Duration::from_millis(10)),
                )
                .await
            }

            Err(mpsc::TryRecvError::Disconnected) => {
                return Err(io::Error::other("stdin reader stopped"));
            }
        }
    }
}

fn overrides(arguments: &InputArguments) -> io::Result<Value> {
    let mut value = json!({});

    for setting in &arguments.settings {
        let (field, text) = setting
            .split_once('=')
            .ok_or_else(|| invalid("--set requires FIELD=JSON"))?;

        let patch: Value = serde_json::from_str(text).map_err(invalid_json)?;
        set(&mut value, field, patch)?;
    }

    if !arguments.include.is_empty() {
        set(&mut value, "selection.include", json!(arguments.include))?;
    }

    if !arguments.exclude.is_empty() {
        set(&mut value, "selection.exclude", json!(arguments.exclude))?;
    }

    if arguments.override_exclusions {
        set(&mut value, "selection.override_exclusions", json!(true))?;
    }

    Ok(value)
}

fn check_overrides(arguments: &CheckArguments) -> io::Result<CheckOverrides> {
    let mut value = overrides(&arguments.inputs)?;

    if let Some(mode) = arguments.mode {
        set(
            &mut value,
            "checking.mode",
            json!(match mode {
                CheckingMode::Strict => "strict",
                CheckingMode::Nonstrict => "nonstrict",
                CheckingMode::Nocheck => "nocheck",
            }),
        )?;
    }

    diagnostic_overrides(&mut value, &arguments.diagnostics)?;

    serde_json::from_value(value).map_err(invalid_json)
}

fn format_overrides(arguments: &FormatArguments) -> io::Result<FormatOverrides> {
    let mut value = overrides(&arguments.inputs)?;

    if let Some(width) = arguments.width {
        set(&mut value, "formatting.width", json!(width))?;
    }

    if let Some(width) = arguments.indent_width {
        set(&mut value, "formatting.indent_width", json!(width))?;
    }

    if let Some(indentation) = arguments.indentation {
        set(
            &mut value,
            "formatting.indent_style",
            json!(match indentation {
                Indentation::Tabs => "tabs",
                Indentation::Spaces => "spaces",
            }),
        )?;
    }

    if let Some(ending) = arguments.line_ending {
        set(
            &mut value,
            "formatting.line_ending",
            json!(match ending {
                LineEnding::Lf => "lf",
                LineEnding::Crlf => "crlf",
            }),
        )?;
    }

    if let Some(quotes) = arguments.quotes {
        set(
            &mut value,
            "formatting.quote_style",
            json!(match quotes {
                QuoteStyle::Preserve => "preserve",
                QuoteStyle::PreferDouble => "prefer_double",
                QuoteStyle::PreferSingle => "prefer_single",
                QuoteStyle::ForceDouble => "double",
                QuoteStyle::ForceSingle => "single",
            }),
        )?;
    }

    if let Some(order) = arguments.import_order {
        set(
            &mut value,
            "formatting.requires.order",
            json!(match order {
                ImportOrder::Preserve => "preserve",
                ImportOrder::Alphabetical => "alphabetical",
            }),
        )?;
    }

    if let Some(newline) = arguments.final_newline {
        set(&mut value, "formatting.final_newline", json!(newline))?;
    }

    serde_json::from_value(value).map_err(invalid_json)
}

fn lint_overrides(arguments: &LintArguments) -> io::Result<LintOverrides> {
    let mut value = overrides(&arguments.inputs)?;

    if !arguments.rules.is_empty() {
        let selected = arguments
            .rules
            .iter()
            .map(|name| rule(name))
            .collect::<io::Result<BTreeSet<_>>>()?;

        set(&mut value, "linting.selected", json!(selected))?;
    }

    if !arguments.presets.is_empty() {
        set(&mut value, "linting.presets", json!(arguments.presets))?;
    }

    diagnostic_overrides(&mut value, &arguments.diagnostics)?;

    serde_json::from_value(value).map_err(invalid_json)
}

fn diagnostic_overrides(value: &mut Value, arguments: &DiagnosticArguments) -> io::Result<()> {
    if let Some(threshold) = arguments.failure_threshold {
        set(value, "diagnostics.threshold", json!(severity(threshold)))?;
    }

    if arguments.warnings_as_errors {
        set(value, "diagnostics.warnings_as_errors", json!(true))?;
    }

    if !arguments.codes.is_empty() {
        set(value, "diagnostics.codes", json!(arguments.codes))?;
    }

    if !arguments.categories.is_empty() {
        let categories = arguments
            .categories
            .iter()
            .map(|category| match category.to_ascii_lowercase().as_str() {
                "syntax" => Ok(DiagnosticCategory::Syntax),
                "type" => Ok(DiagnosticCategory::Type),
                "resolution" => Ok(DiagnosticCategory::Resolution),
                "analysis" => Ok(DiagnosticCategory::Analysis),
                "configuration" => Ok(DiagnosticCategory::Configuration),
                "lint" => Ok(DiagnosticCategory::Lint),
                "formatting" => Ok(DiagnosticCategory::Formatting),
                "edit" => Ok(DiagnosticCategory::Edit),
                _ => Err(invalid("unknown diagnostic category")),
            })
            .collect::<io::Result<Vec<_>>>()?;

        set(value, "diagnostics.categories", json!(categories))?;
    }

    if !arguments.severities.is_empty() {
        set(
            value,
            "diagnostics.severities",
            json!(
                arguments
                    .severities
                    .iter()
                    .copied()
                    .map(severity)
                    .collect::<Vec<_>>()
            ),
        )?;
    }

    Ok(())
}

fn format_scope(arguments: &FormatArguments) -> io::Result<FormatScope> {
    if let Some(selection) = &arguments.on_type {
        let (position, trigger) = selection
            .split_once(':')
            .ok_or_else(|| invalid("--on-type requires OFFSET:CHAR"))?;

        let offset = position
            .parse()
            .map_err(|_| invalid("on-type offset must be a byte position"))?;

        let mut characters = trigger.chars();

        let trigger = characters
            .next()
            .ok_or_else(|| invalid("on-type trigger must be one character"))?;

        if characters.next().is_some() {
            return Err(invalid("on-type trigger must be one character"));
        }

        return Ok(FormatScope::OnType { offset, trigger });
    }

    if arguments.ranges.is_empty() {
        return Ok(FormatScope::Whole);
    }

    let ranges = arguments
        .ranges
        .iter()
        .map(|range| {
            let (start, end) = range
                .split_once(':')
                .ok_or_else(|| invalid("--range requires START:END"))?;

            let start = start
                .parse()
                .map_err(|_| invalid("range start must be a byte position"))?;

            let end = end
                .parse()
                .map_err(|_| invalid("range end must be a byte position"))?;

            if start > end {
                return Err(invalid("range start exceeds range end"));
            }

            Ok(Span { start, end })
        })
        .collect::<io::Result<Vec<_>>>()?;

    Ok(FormatScope::Ranges(ranges))
}

fn set(root: &mut Value, field: &str, value: Value) -> io::Result<()> {
    let mut fields = field.split('.').peekable();
    let mut destination = root;

    while let Some(field) = fields.next() {
        if field.is_empty() {
            return Err(invalid("override field cannot be empty"));
        }

        let object = destination
            .as_object_mut()
            .ok_or_else(|| invalid("override field conflicts with a scalar value"))?;

        if fields.peek().is_none() {
            object.insert(field.to_owned(), value);

            return Ok(());
        }

        destination = object.entry(field).or_insert_with(|| json!({}));
    }

    Err(invalid("override field cannot be empty"))
}

fn operation(operation: OperationKind) -> Operation {
    match operation {
        OperationKind::Check => Operation::Check,
        OperationKind::Format => Operation::Format,
        OperationKind::Lint => Operation::Lint,
    }
}

fn severity(severity: DiagnosticSeverity) -> Severity {
    match severity {
        DiagnosticSeverity::Error => Severity::Error,
        DiagnosticSeverity::Warning => Severity::Warning,
        DiagnosticSeverity::Information => Severity::Information,
        DiagnosticSeverity::Hint => Severity::Hint,
    }
}

fn rule(name: &str) -> io::Result<RuleIdentity> {
    RuleIdentity::named(name).ok_or_else(|| invalid(format!("unknown rule: {name}")))
}

fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message.into())
}

fn invalid_json(error: serde_json::Error) -> io::Error {
    invalid(error.to_string())
}
