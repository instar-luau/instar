use std::{
    collections::{BTreeMap, BTreeSet},
    env,
    fs::{self, File},
    io::{self, BufWriter, IsTerminal, Seek, Write},
    path::{self, Component, Path, PathBuf},
    time::Duration,
};

use codespan_reporting::term::termcolor::ColorChoice;
use indicatif::{ProgressBar, ProgressDrawTarget, ProgressFinish, ProgressStyle};
use instar_format::FormatOutcome;

use instar_project::{
    CheckReport, Counts, ExplanationSelection, FixMode, FormatMode, FormatReport, InputIdentity,
    InputVersion, LintReport, ProjectSnapshot, RuleDescription, SelectionDecision, SettingsReport,
    SourceOrigin, WatchReport, WriteOutcome,
};

use instar_shared::{
    Completion, Diagnostic, Event, EventPayload, FindingIdentity, LogLevel, OperationFailure,
    Severity,
};

use serde_json::{Value, json};
use tempfile::{NamedTempFile, TempPath};
use tracing::Level as TraceLevel;
use tracing_subscriber::{filter::Targets, fmt, layer::SubscriberExt, util::SubscriberInitExt};

use super::{diagnostic::Diagnostics, difference};
use crate::command::{Color, CommandStatus, ReportStyle, ReportingArguments};

pub(crate) enum CommandReport {
    Check(CheckReport),
    Format(FormatReport, FormatMode),
    Lint(LintReport, FixMode),
    Settings(SettingsReport),
    Watch(WatchReport),
    Schema(Value),
    Rules(Vec<RuleDescription>),
    Failure(OperationFailure),
}

impl CommandReport {
    pub(crate) fn completion(&self) -> &Completion {
        match self {
            Self::Check(report) => &report.completion,
            Self::Format(report, _) => &report.completion,
            Self::Lint(report, _) => &report.completion,
            Self::Settings(report) => &report.completion,
            Self::Watch(report) => &report.completion,
            Self::Failure(report) => &report.completion,
            Self::Schema(_) | Self::Rules(_) => &Completion::Complete,
        }
    }

    pub(crate) fn status(&self) -> CommandStatus {
        let failure = match self {
            Self::Failure(failure) => Some(failure),
            Self::Watch(report) => report.failure.as_ref(),
            _ => None,
        };

        if let Some(failure) = failure {
            return CommandStatus::failure(failure);
        }

        let failed_write = match self {
            Self::Format(report, _) => report
                .results
                .iter()
                .any(|source| matches!(source.write, WriteOutcome::Failed { .. })),

            Self::Lint(report, _) => report
                .writes
                .iter()
                .any(|write| matches!(write, WriteOutcome::Failed { .. })),

            _ => false,
        };

        if failed_write {
            return CommandStatus::OperationalFailure;
        }

        if self.completion() != &Completion::Complete {
            return CommandStatus::Incomplete;
        }

        match self {
            Self::Check(report) => {
                if report.failed {
                    CommandStatus::Findings
                } else {
                    CommandStatus::Clean
                }
            }

            Self::Format(report, mode) => {
                if report.counts.unsuccessful != 0 {
                    CommandStatus::UnsuccessfulFormatting
                } else if matches!(mode, FormatMode::Check) && report.counts.changed != 0 {
                    CommandStatus::Findings
                } else {
                    CommandStatus::Clean
                }
            }

            Self::Lint(report, _) => {
                if report.failed {
                    CommandStatus::Findings
                } else {
                    CommandStatus::Clean
                }
            }

            Self::Settings(report) => {
                if report
                    .diagnostics
                    .iter()
                    .any(|diagnostic| diagnostic.severity == Severity::Error)
                {
                    CommandStatus::InvalidInvocation
                } else {
                    CommandStatus::Clean
                }
            }

            Self::Schema(_) | Self::Rules(_) | Self::Watch(_) => CommandStatus::Clean,
            Self::Failure(_) => unreachable!("failure classified before report outcomes"),
        }
    }

    fn snapshot(&self) -> Option<&ProjectSnapshot> {
        match self {
            Self::Check(report) => Some(&report.snapshot),
            Self::Format(report, _) => Some(&report.snapshot),
            Self::Lint(report, _) => Some(&report.snapshot),
            Self::Settings(report) => Some(&report.snapshot),
            Self::Watch(report) => Some(&report.snapshot),
            Self::Schema(_) | Self::Rules(_) | Self::Failure(_) => None,
        }
    }

    fn findings(&self) -> Vec<&Diagnostic> {
        match self {
            Self::Check(report) => report
                .findings
                .iter()
                .chain(&report.environment_diagnostics)
                .collect(),

            Self::Format(report, _) => report
                .diagnostics
                .iter()
                .chain(
                    report
                        .results
                        .iter()
                        .flat_map(|source| &source.result.diagnostics),
                )
                .collect(),

            Self::Lint(report, _) => report.final_findings.iter().collect(),
            Self::Settings(report) => report.diagnostics.iter().collect(),
            Self::Failure(report) => report.diagnostics.iter().collect(),

            Self::Watch(report) => report
                .failure
                .iter()
                .flat_map(|failure| &failure.diagnostics)
                .collect(),

            Self::Schema(_) | Self::Rules(_) => Vec::new(),
        }
    }
}

pub(crate) struct Reporter {
    diagnostics: Diagnostics,
    progress: ProgressBar,
    destination: Option<BufWriter<File>>,
    destination_path: Option<PathBuf>,
    temporary: Option<TempPath>,
    explanations: ExplanationSelection,
    json: bool,
    error_output: bool,
    quiet: bool,
    streaming: bool,
    color: ColorChoice,
    emitted: BTreeSet<FindingIdentity>,
    output_failure: Option<io::Error>,
    iteration: u64,
    watch_snapshot: Option<u64>,
    changed_inputs: Vec<InputVersion>,
    protected_paths: BTreeMap<PathBuf, PathBuf>,
    historical: Vec<Value>,
    watch_events: Vec<Event>,
}

impl Reporter {
    pub(crate) fn new(arguments: ReportingArguments) -> io::Result<Self> {
        let json = arguments.json || arguments.style == ReportStyle::Json;
        let progress = ProgressBar::new_spinner().with_finish(ProgressFinish::AndClear);

        if json || arguments.quiet || !io::stderr().is_terminal() {
            progress.set_draw_target(ProgressDrawTarget::hidden());
        }

        let color = match arguments.color {
            Color::Always => ColorChoice::Always,
            Color::Never => ColorChoice::Never,

            Color::Auto
                if arguments.destination.is_some()
                    || !io::stderr().is_terminal()
                    || env::var_os("NO_COLOR").is_some_and(|value| !value.is_empty()) =>
            {
                ColorChoice::Never
            }

            Color::Auto => ColorChoice::Always,
        };

        let destination_path = arguments.destination.as_deref().map(resolve).transpose()?;

        let (destination, temporary) = if let Some(destination) = &destination_path {
            let temporary = NamedTempFile::new_in(destination.parent().ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "report destination has no parent",
                )
            })?)?;

            let (file, path) = temporary.into_parts();

            (Some(BufWriter::new(file)), Some(path))
        } else {
            (None, None)
        };

        console::set_colors_enabled_stderr(!matches!(color, ColorChoice::Never));

        Ok(Self {
            diagnostics: Diagnostics::default(),
            progress,
            destination,
            destination_path,
            temporary,
            explanations: ExplanationSelection::default(),
            json,
            error_output: false,
            quiet: arguments.quiet,
            streaming: arguments.streaming,
            color,
            emitted: BTreeSet::new(),
            output_failure: None,
            iteration: 0,
            watch_snapshot: None,
            changed_inputs: Vec::new(),
            protected_paths: BTreeMap::new(),
            historical: Vec::new(),
            watch_events: Vec::new(),
        })
    }

    pub(crate) fn trace(&self) -> eyre::Result<()> {
        let progress = self.progress.clone();

        let output = fmt::layer()
            .without_time()
            .with_ansi(!self.json && !matches!(self.color, ColorChoice::Never))
            .with_writer(move || LogWriter(progress.clone()));

        let filter = Targets::new().with_target("instar", TraceLevel::DEBUG);

        let result = if self.json {
            tracing_subscriber::registry()
                .with(output.json())
                .with(filter)
                .try_init()
        } else {
            tracing_subscriber::registry()
                .with(output)
                .with(filter)
                .try_init()
        };

        result.map_err(|error| eyre::eyre!("{error}"))
    }

    pub(crate) fn capture(&mut self, snapshot: &ProjectSnapshot) {
        self.diagnostics.capture(snapshot);

        let protected = snapshot
            .sources()
            .filter_map(|source| snapshot.origin(source.version.identity))
            .filter_map(SourceOrigin::path)
            .map(PathBuf::from)
            .chain(
                snapshot
                    .input_versions
                    .keys()
                    .filter_map(|identity| match identity {
                        InputIdentity::Path(path) | InputIdentity::Directory(path) => {
                            Some(path.clone())
                        }

                        _ => None,
                    }),
            );

        for path in protected {
            if let Err(error) = self.protect(path) {
                self.failed_output(error);
                break;
            }
        }
    }

    pub(crate) fn explain(&mut self, explanations: ExplanationSelection) {
        self.explanations = explanations;
    }

    pub(crate) fn publish(&mut self) -> io::Result<()> {
        self.check_output()?;

        if let Some(destination) = &self.destination_path {
            let current = resolve(destination)?;

            for (input, captured) in &self.protected_paths {
                let resolved = resolve(input)?;

                if captured == destination
                    || captured == &current
                    || resolved == *destination
                    || resolved == current
                {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "report destination is an input to the operation",
                    ));
                }
            }
        }

        if let Some(destination) = &mut self.destination {
            destination.flush()?;
        }

        if let Some(temporary) = self.temporary.take() {
            temporary
                .persist(
                    self.destination_path
                        .as_ref()
                        .expect("staged report destination"),
                )
                .map_err(io::Error::other)?;
        }

        Ok(())
    }

    pub(crate) fn document(&mut self, bytes: &[u8]) -> io::Result<()> {
        if self.destination.is_some() {
            self.write_report(bytes)
        } else {
            io::stdout().lock().write_all(bytes)
        }
    }

    pub(crate) fn watch(
        &mut self,
        snapshot: &ProjectSnapshot,
        changed: Vec<InputVersion>,
    ) -> io::Result<()> {
        self.capture(snapshot);
        self.watch_snapshot = Some(snapshot.identity);
        self.changed_inputs = changed;

        if self.streaming && self.json {
            self.record(&json!({"event":"iteration","iteration":self.iteration,"snapshot":self.watch_snapshot,"changed":self.changed_inputs}))?;
        } else if !self.json && !self.quiet {
            self.write_report(
                format!(
                    "Watch iteration {}: {}\n",
                    self.iteration,
                    serde_json::to_string(&self.changed_inputs).map_err(io::Error::other)?
                )
                .as_bytes(),
            )?;
        }

        Ok(())
    }

    pub(crate) fn historical(
        &mut self,
        snapshot: &ProjectSnapshot,
        changed: Vec<InputVersion>,
        report: CommandReport,
    ) -> io::Result<()> {
        self.capture(snapshot);
        self.capture_report(&report);
        self.check_output()?;

        if !self.json {
            let mut identities = BTreeSet::new();

            let findings = report
                .findings()
                .into_iter()
                .filter(|finding| identities.insert(finding.identity.clone()))
                .collect::<Vec<_>>();

            if !findings.is_empty() {
                self.write_report(b"Historical watch findings (not current)\n")?;
            }

            for finding in findings {
                let output = self.diagnostics.render(finding, self.color)?;
                self.write_report(&output)?;
            }
        }

        let mut record = self.projection(&report);
        record["current"] = json!(false);
        record["snapshot"] = json!(snapshot.identity);
        record["invalidations"] = json!(changed);
        self.historical.push(record);
        self.watch_snapshot = Some(snapshot.identity);

        Ok(())
    }

    pub(crate) fn events(&mut self, events: Vec<Event>) {
        self.watch_events.extend(events);
    }

    fn capture_report(&mut self, report: &CommandReport) {
        if let CommandReport::Lint(report, _) = report {
            for snapshot in &report.snapshots {
                self.capture(snapshot);
            }
        }

        if let Some(snapshot) = report.snapshot() {
            self.capture(snapshot);
        }
    }

    pub(crate) fn failed_output(&mut self, error: io::Error) {
        if self.output_failure.is_none() {
            self.output_failure = Some(error);
        }
    }

    pub(crate) fn output_failed(&self) -> bool {
        self.output_failure.is_some()
    }

    pub(crate) fn check_output(&mut self) -> io::Result<()> {
        match self.output_failure.take() {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    pub(crate) fn event(&mut self, event: &Event) -> io::Result<()> {
        if self.output_failure.is_some() {
            return Ok(());
        }

        match &event.payload {
            EventPayload::InputPath(path) => self.protect(path.clone())?,

            EventPayload::Progress(progress) => {
                if !self.quiet {
                    let measured = progress.total.is_some();

                    self.progress.set_style(
                        ProgressStyle::with_template(if measured {
                            "{spinner} {prefix} {pos}/{len} {wide_msg}"
                        } else {
                            "{spinner} {prefix} {wide_msg}"
                        })
                        .map_err(io::Error::other)?,
                    );

                    if let Some(total) = progress.total {
                        self.progress.set_length(total as u64);
                    }

                    self.progress.set_prefix(
                        console::style(&progress.phase)
                            .for_stderr()
                            .bold()
                            .to_string(),
                    );

                    self.progress.set_position(progress.completed as u64);

                    self.progress.set_message(
                        progress
                            .source
                            .map_or_else(String::new, |source| self.diagnostics.name(source)),
                    );

                    self.progress.enable_steady_tick(Duration::from_millis(100));
                }
            }

            EventPayload::Diagnostic(diagnostic) => {
                if self.streaming
                    && (self.json || self.diagnostics.available(diagnostic))
                    && self.emitted.insert(diagnostic.identity.clone())
                {
                    if self.json {
                        self.record(&json!({"event":"diagnostic","invocation":event.invocation,"operation":event.operation,"iteration":self.iteration,"diagnostic":self.diagnostics.record(diagnostic)}))?;
                    } else {
                        let output = self.diagnostics.render(diagnostic, self.color)?;
                        self.write_report(&output)?;
                    }
                }
            }

            EventPayload::SourceOutcome(_) | EventPayload::EditOutcome(_) => {
                if self.streaming && self.json {
                    self.record(&json!({"event":event,"iteration":self.iteration}))?;
                }
            }

            EventPayload::Log(log) => {
                if matches!(log.level, LogLevel::Warning | LogLevel::Error) {
                    self.progress.suspend(|| eprintln!("{}", log.message));
                } else {
                    tracing::debug!(level = ?log.level, message = %log.message, "Operation event");
                }
            }
        }

        Ok(())
    }

    pub(crate) fn finish(&mut self, report: &CommandReport) -> io::Result<()> {
        self.error_output = matches!(report.status(), CommandStatus::InvalidInvocation);
        self.progress.finish_and_clear();
        self.check_output()?;
        self.capture_report(report);
        self.check_output()?;

        match report {
            CommandReport::Schema(schema) => {
                self.metadata(schema)?;

                return Ok(());
            }

            CommandReport::Rules(rules) => {
                self.metadata(&json!(rules))?;

                return Ok(());
            }

            _ => {}
        }

        let findings = report.findings();
        let mut identities = BTreeSet::new();

        let findings = findings
            .into_iter()
            .filter(|finding| identities.insert(finding.identity.clone()))
            .collect::<Vec<_>>();

        let mut record = self.projection(report);

        if let Some(snapshot) = self.watch_snapshot {
            let snapshot = match report {
                CommandReport::Watch(report) => report.snapshot.identity,
                _ => snapshot,
            };

            record["iteration"] = json!(self.iteration);
            record["snapshot"] = json!(snapshot);
            record["invalidations"] = json!(self.changed_inputs);
        }

        let source = self.source(report)?;

        if self.json {
            if self.streaming {
                self.record(&json!({"event":"report","iteration":self.iteration,"report":record}))?;
            } else {
                self.record(&record)?;
            }
        } else {
            for diagnostic in findings {
                if !self.emitted.contains(&diagnostic.identity) {
                    let output = self.diagnostics.render(diagnostic, self.color)?;
                    self.write_report(&output)?;
                }
            }

            self.human(report)?;
        }

        if let Some(source) = source {
            io::stdout().lock().write_all(source.as_bytes())?;
            io::stdout().lock().flush()?;
        }

        self.emitted.clear();
        self.iteration += 1;
        self.progress.reset();

        Ok(())
    }

    fn projection(&self, report: &CommandReport) -> Value {
        let mut identities = BTreeSet::new();

        let diagnostics = report
            .findings()
            .into_iter()
            .filter(|finding| identities.insert(finding.identity.clone()))
            .map(|finding| self.diagnostics.record(finding))
            .collect::<Vec<_>>();

        let mut record = match report {
            CommandReport::Check(report) => {
                json!({"operation":"check","sources":report.sources,"modules":report.modules,"module_findings":report.module_findings,"imports":report.imports,"selection":report.selection,"dependencies":report.dependencies,"settings":report.settings,"counts":report.counts,"failed":report.failed,"changed":[],"skipped":skipped(&report.snapshot,&report.selection)})
            }

            CommandReport::Format(report, mode) => {
                let changed = report
                    .results
                    .iter()
                    .filter_map(|source| match &source.result.outcome {
                        FormatOutcome::Validated(proposal)
                            if proposal.input().source.text.as_ref() != proposal.output() =>
                        {
                            Some(self.diagnostics.name(source.result.source))
                        }

                        _ => None,
                    })
                    .collect::<Vec<_>>();

                let sources = report.results.iter().map(|source| {
                    let outcome = match &source.result.outcome {
                        FormatOutcome::Validated(proposal) => json!({"state":"validated","output":proposal.output(),"selections":proposal.selections(),"preservation":proposal.preservation()}),
                        FormatOutcome::Unavailable { reason, selections, preservation } => json!({"state":"unavailable","reason":reason,"selections":selections,"preservation":preservation}),
                    };

                    json!({"source":source.result.source,"path":self.diagnostics.name(source.result.source),"outcome":outcome,"diagnostics":source.result.diagnostics,"completion":source.result.completion,"proposal":source.proposal.as_ref().map(|proposal| proposal.proposal()),"preview":source.preview,"write":source.write})
                }).collect::<Vec<_>>();

                json!({"operation":"format","mode":format!("{mode:?}"),"sources":sources,"selection":report.selection,"settings":report.settings,"counts":report.counts,"failed":report.failed,"changed":changed,"skipped":skipped(&report.snapshot,&report.selection)})
            }

            CommandReport::Lint(report, mode) => {
                json!({"operation":"lint","mode":format!("{mode:?}"),"sources":report.sources,"modules":report.modules,"module_findings":report.module_findings,"imports":report.imports,"outcomes":report.outcomes,"findings":report.findings.iter().map(|finding|self.diagnostics.record(finding)).collect::<Vec<_>>(),"final_findings":diagnostics,"availability":report.availability,"fixes":report.fixes,"proposals":report.proposals.iter().map(|proposal|proposal.proposal()).collect::<Vec<_>>(),"previews":report.previews,"writes":report.writes,"selection":report.selection,"settings":report.settings,"dependencies":report.dependencies,"counts":report.counts,"failed":report.failed,"changed":[],"skipped":skipped(&report.snapshot,&report.selection)})
            }

            CommandReport::Settings(report) => {
                json!({"operation":report.operation,"settings":report.settings,"schema_identity":report.schema_identity})
            }

            CommandReport::Watch(report) => {
                json!({"operation":"watch","iterations":[],"failure":report.failure})
            }

            CommandReport::Schema(schema) => schema.clone(),
            CommandReport::Rules(rules) => json!(rules),

            CommandReport::Failure(failure) => {
                let mut record = failure_record(failure);
                record["changed"] = json!([]);
                record["skipped"] = json!([]);

                record
            }
        };

        record["diagnostics"] = json!(diagnostics);
        record["completion"] = json!(report.completion());
        record["status"] = json!(report.status().code());

        if matches!(report, CommandReport::Watch(_))
            || (matches!(report, CommandReport::Failure(_)) && self.watch_snapshot.is_some())
        {
            record["historical"] = json!(self.historical);
            record["events"] = json!(self.watch_events);
        }

        record
    }

    fn source(&self, report: &CommandReport) -> io::Result<Option<String>> {
        match report {
            CommandReport::Format(report, mode)
                if matches!(mode, FormatMode::Source | FormatMode::Write) =>
            {
                let stdin = report
                    .snapshot
                    .sources()
                    .filter(|source| {
                        report
                            .selection
                            .iter()
                            .any(|decision| decision.source == Some(source.version.identity))
                            && matches!(
                                report.snapshot.origin(source.version.identity),
                                Some(SourceOrigin::Stdin { .. })
                            )
                    })
                    .collect::<Vec<_>>();

                let sources = report
                    .results
                    .iter()
                    .filter(|source| {
                        matches!(mode, FormatMode::Source)
                            || stdin.iter().any(|stdin| {
                                stdin.version.identity == source.result.source.identity
                            })
                    })
                    .collect::<Vec<_>>();

                if sources.len() > 1 {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "source output requires exactly one emitted source",
                    ));
                }

                if let Some(source) = sources.first() {
                    return Ok(match &source.result.outcome {
                        FormatOutcome::Validated(proposal)
                            if source.result.completion == Completion::Complete =>
                        {
                            Some(proposal.output().to_owned())
                        }

                        _ => None,
                    });
                }

                if stdin.len() == 1 && report.counts.excluded != 0 {
                    return Ok(Some(stdin[0].text.to_string()));
                }

                if matches!(mode, FormatMode::Source)
                    && report.completion == Completion::Complete
                    && report.counts.unsuccessful == 0
                {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "source output requires exactly one emitted source",
                    ));
                }

                Ok(None)
            }

            CommandReport::Lint(report, mode)
                if matches!(mode, FixMode::Stdin | FixMode::Write) =>
            {
                let stdin = report
                    .snapshot
                    .sources()
                    .filter(|source| {
                        report
                            .selection
                            .iter()
                            .any(|decision| decision.source == Some(source.version.identity))
                            && matches!(
                                report.snapshot.origin(source.version.identity),
                                Some(SourceOrigin::Stdin { .. })
                            )
                    })
                    .collect::<Vec<_>>();

                if stdin.len() > 1 {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "fixed stdin requires exactly one emitted source",
                    ));
                }

                Ok(stdin
                    .first()
                    .filter(|source| {
                        report
                            .outcomes
                            .iter()
                            .rev()
                            .find(|outcome| outcome.source == source.version)
                            .is_some_and(|outcome| outcome.completion == Completion::Complete)
                    })
                    .map(|source| source.text.to_string()))
            }

            _ => Ok(None),
        }
    }

    fn human(&mut self, report: &CommandReport) -> io::Result<()> {
        match report {
            CommandReport::Format(report, mode) => {
                for source in &report.results {
                    if let FormatOutcome::Validated(proposal) = &source.result.outcome {
                        let name = self.diagnostics.name(source.result.source);

                        if matches!(mode,FormatMode::Check) && proposal.input().source.text.as_ref() != proposal.output() { self.write_report(format!("Would reformat {name}\n").as_bytes())?; }

                        if matches!(mode,FormatMode::Preview) {
                            let mut patch = Vec::new(); difference::render(&mut patch, &name, &proposal.input().source.text, proposal.output())?; self.write_report(&patch)?;
                        }
                    }

                    if matches!(source.write,WriteOutcome::Failed { .. } | WriteOutcome::Unapplied { .. }) { self.write_report(format!("{:?}\n",source.write).as_bytes())?; }
                }

                if !self.quiet { self.summary("Formatting",&report.counts,&report.completion,Some(*mode))?; }
            }

            CommandReport::Lint(report, mode) => {
                if matches!(mode,FixMode::Preview) { for preview in &report.previews { for line in preview.lines().take(400) { self.write_report(format!("{line}\n").as_bytes())?; } if preview.lines().count() > 400 { self.write_report(b"... patch truncated after 400 lines\n")?; } } }

                for write in &report.writes { if matches!(write,WriteOutcome::Failed { .. } | WriteOutcome::Unapplied { .. }) { self.write_report(format!("{write:?}\n").as_bytes())?; } }

                if !self.quiet { self.summary("Linting",&report.counts,&report.completion,None)?; }
            }

            CommandReport::Check(report) => { if !self.quiet { self.summary("Checking",&report.counts,&report.completion,None)?; } }
            CommandReport::Settings(report) => self.metadata(&json!({"operation":report.operation,"settings":report.settings,"schema_identity":report.schema_identity,"completion":report.completion}))?,

            CommandReport::Watch(report) => {
                if let Some(failure) = &report.failure { self.write_report(format!("{}\n", failure.message).as_bytes())?; }

                self.watch_history()?;
                self.write_report(format!("Watch completion: {:?}\n", report.completion).as_bytes())?;
            }

            CommandReport::Failure(failure) => {
                self.watch_history()?;
                self.write_report(format!("{}\nCompletion: {:?}\n", failure.message, failure.completion).as_bytes())?;
            }

            CommandReport::Schema(_) | CommandReport::Rules(_) => {},
        }

        if !self.quiet {
            let decisions = match report {
                CommandReport::Check(report) => Some((&report.snapshot, &report.selection)),
                CommandReport::Format(report, _) => Some((&report.snapshot, &report.selection)),
                CommandReport::Lint(report, _) => Some((&report.snapshot, &report.selection)),
                _ => None,
            };

            if let Some((snapshot, decisions)) = decisions {
                for path in skipped(snapshot, decisions) {
                    self.write_report(format!("Skipped {path}\n").as_bytes())?;
                }
            }
        }

        let explanations = match report {
            CommandReport::Check(report) => Some((
                &report.selection,
                &report.settings,
                Some((&report.dependencies, &report.imports)),
            )),

            CommandReport::Format(report, _) => Some((&report.selection, &report.settings, None)),

            CommandReport::Lint(report, _) => Some((
                &report.selection,
                &report.settings,
                Some((&report.dependencies, &report.imports)),
            )),

            _ => None,
        };

        if let Some((selection, settings, dependencies)) = explanations {
            if self.explanations.selection {
                self.write_report(
                    format!(
                        "Selection: {}\n",
                        serde_json::to_string_pretty(selection).map_err(io::Error::other)?
                    )
                    .as_bytes(),
                )?;
            }

            if self.explanations.settings {
                self.write_report(
                    format!(
                        "Settings: {}\n",
                        serde_json::to_string_pretty(settings).map_err(io::Error::other)?
                    )
                    .as_bytes(),
                )?;
            }

            if self.explanations.dependencies
                && let Some((dependencies, imports)) = dependencies
            {
                self.write_report(
                    format!(
                        "Dependencies: {}\n",
                        serde_json::to_string_pretty(dependencies).map_err(io::Error::other)?
                    )
                    .as_bytes(),
                )?;

                self.write_report(
                    format!(
                        "Imports: {}\n",
                        serde_json::to_string_pretty(imports).map_err(io::Error::other)?
                    )
                    .as_bytes(),
                )?;
            }
        }

        Ok(())
    }

    fn watch_history(&mut self) -> io::Result<()> {
        if !self.quiet {
            if !self.historical.is_empty() {
                self.write_report(
                    format!(
                        "Historical watch outcomes (not current): {}\n",
                        serde_json::to_string(&self.historical).map_err(io::Error::other)?
                    )
                    .as_bytes(),
                )?;
            }

            if !self.watch_events.is_empty() {
                self.write_report(
                    format!(
                        "Watch events: {}\n",
                        serde_json::to_string(&self.watch_events).map_err(io::Error::other)?
                    )
                    .as_bytes(),
                )?;
            }
        }

        Ok(())
    }

    fn summary(
        &mut self,
        operation: &str,
        counts: &Counts,
        completion: &Completion,
        format: Option<FormatMode>,
    ) -> io::Result<()> {
        let state = if completion == &Completion::Complete {
            "complete"
        } else {
            "incomplete"
        };

        let files = counts.selected + counts.excluded;

        let details = if let Some(mode) = format {
            format!(
                ", {} {}, {} unchanged, {} unsuccessful, {} skipped",
                counts.changed,
                if matches!(mode, FormatMode::Check) {
                    "would change"
                } else {
                    "formatted"
                },
                counts.unchanged,
                counts.unsuccessful,
                counts.excluded
            )
        } else {
            format!(", {} skipped", counts.excluded)
        };

        self.write_report(format!("{operation} {state}: {files} file{}{details}; {} diagnostics; {} dependencies; {} written; {} fixes; completion {completion:?}\n",if files == 1 { "" } else { "s" },counts.findings,counts.dependencies,counts.written,counts.fixes).as_bytes())
    }

    fn metadata(&mut self, value: &Value) -> io::Result<()> {
        let output = serde_json::to_vec_pretty(value).map_err(io::Error::other)?;

        if self.destination.is_some() {
            self.write_report(&output)?;

            self.write_report(b"\n")
        } else {
            let mut output_stream = io::stdout().lock();
            output_stream.write_all(&output)?;

            writeln!(output_stream)
        }
    }

    fn record(&mut self, value: &Value) -> io::Result<()> {
        let mut bytes = serde_json::to_vec(value).map_err(io::Error::other)?;
        bytes.push(b'\n');

        self.write_report(&bytes)
    }

    fn protect(&mut self, path: PathBuf) -> io::Result<()> {
        let Some(destination) = &self.destination_path else {
            return Ok(());
        };

        if self.protected_paths.contains_key(&path) {
            return Ok(());
        }

        let resolved = resolve(&path)?;

        if resolved == *destination {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "report destination is an input to the operation",
            ));
        }

        self.protected_paths.insert(path, resolved);

        Ok(())
    }

    fn write_report(&mut self, bytes: &[u8]) -> io::Result<()> {
        if self.output_failure.is_some() {
            return Ok(());
        }

        if self.watch_snapshot.is_some() && self.temporary.is_none() && self.destination.is_some() {
            let parent = self
                .destination_path
                .as_ref()
                .and_then(|path| path.parent())
                .expect("staged report directory");

            let mut temporary = NamedTempFile::new_in(parent)?;

            let previous = self
                .destination
                .as_mut()
                .expect("published report")
                .get_mut();

            previous.rewind()?;
            io::copy(previous, temporary.as_file_mut())?;
            let (file, path) = temporary.into_parts();
            self.destination = Some(BufWriter::new(file));
            self.temporary = Some(path);
        }

        if let Some(destination) = &mut self.destination {
            destination.write_all(bytes)?;

            destination.flush()
        } else if self.json && !self.error_output {
            let mut output = io::stdout().lock();
            output.write_all(bytes)?;

            output.flush()
        } else {
            self.progress.suspend(|| {
                let mut output = io::stderr().lock();
                output.write_all(bytes)?;

                output.flush()
            })
        }
    }
}

struct LogWriter(ProgressBar);

impl Write for LogWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.suspend(|| {
            io::stderr().lock().write_all(bytes)?;

            Ok(bytes.len())
        })
    }

    fn flush(&mut self) -> io::Result<()> {
        self.0.suspend(|| io::stderr().lock().flush())
    }
}

pub(crate) fn failure_record(failure: &OperationFailure) -> Value {
    json!({"error":failure.message,"kind":failure.kind,"diagnostics":failure.diagnostics,"completion":failure.completion,"status":CommandStatus::failure(failure).code(),"completed_sources":failure.completed_sources,"completed_edits":failure.completed_edits})
}

fn skipped(snapshot: &ProjectSnapshot, decisions: &[SelectionDecision]) -> Vec<String> {
    decisions
        .iter()
        .filter(|decision| !decision.selected)
        .filter_map(|decision| decision.source)
        .filter_map(|source| snapshot.origin(source))
        .filter_map(SourceOrigin::path)
        .map(|path| path.display().to_string())
        .collect()
}

fn resolve(input: &Path) -> io::Result<PathBuf> {
    let absolute = path::absolute(input)?;
    let mut ancestor = absolute.as_path();
    let mut missing = Vec::new();

    loop {
        match fs::canonicalize(ancestor) {
            Ok(mut resolved) => {
                for component in missing.into_iter().rev() {
                    if component == ".." {
                        if resolved.parent().is_some() {
                            resolved.pop();
                        }
                    } else if component != "." {
                        resolved.push(component);
                    }
                }

                return Ok(resolved);
            }

            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
                ) =>
            {
                let component = ancestor.components().next_back().ok_or(error)?;

                if !matches!(
                    component,
                    Component::Normal(_) | Component::CurDir | Component::ParentDir
                ) {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "report path has no existing ancestor",
                    ));
                }

                missing.push(component.as_os_str().to_owned());

                ancestor = ancestor.parent().ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "report path has no existing ancestor",
                    )
                })?;
            }

            Err(error) => return Err(error),
        }
    }
}
