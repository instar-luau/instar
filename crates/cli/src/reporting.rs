use std::{
    io::{self, IsTerminal, Write},
    path::Path,
};

use codespan_reporting::{
    diagnostic::{Diagnostic, Label},
    files::SimpleFiles,
    term::{
        self,
        termcolor::{ColorChoice, StandardStream},
    },
};

use eyre::{Result, ensure};
use indicatif::{ProgressBar, ProgressDrawTarget, ProgressFinish, ProgressStyle};
use instar_analysis::{Completion, Location, Related};
use instar_core::{analysis::Origin, project::Project};
use serde_json::{Value, json};
use tracing_subscriber::{filter::Targets, layer::SubscriberExt, util::SubscriberInitExt};

use crate::arguments::Command;

pub(crate) struct Reporter {
    files: SimpleFiles<String, String>,
    diagnostics: Vec<Diagnostic<usize>>,
    records: Vec<Value>,
    progress: ProgressBar,
    json: bool,
    color: ColorChoice,
    pub(crate) failed: bool,
    pub(crate) processed: usize,
    pub(crate) formatted: usize,
    pub(crate) unchanged: usize,
    pub(crate) skipped: usize,
    pub(crate) unsuccessful: usize,
}

impl Reporter {
    pub(crate) fn new(json: bool, color: ColorChoice) -> Self {
        let progress = ProgressBar::new_spinner().with_finish(ProgressFinish::AndClear);

        if json || !io::stderr().is_terminal() {
            progress.set_draw_target(ProgressDrawTarget::hidden());
        }

        Self {
            files: SimpleFiles::new(),
            diagnostics: Vec::new(),
            records: Vec::new(),
            progress,
            json,
            color,
            failed: false,
            processed: 0,
            formatted: 0,
            unchanged: 0,
            skipped: 0,
            unsuccessful: 0,
        }
    }

    pub(crate) fn trace(&self) -> Result<()> {
        let progress = self.progress.clone();

        let output = tracing_subscriber::fmt::layer()
            .without_time()
            .with_ansi(!self.json && !matches!(self.color, ColorChoice::Never))
            .with_writer(move || Writer(progress.clone()));

        let filter = Targets::new().with_target("instar", tracing::Level::DEBUG);

        if self.json {
            tracing_subscriber::registry()
                .with(output.json())
                .with(filter)
                .try_init()
                .map_err(|error| eyre::eyre!("{error}"))?;
        } else {
            tracing_subscriber::registry()
                .with(output)
                .with(filter)
                .try_init()
                .map_err(|error| eyre::eyre!("{error}"))?;
        }

        Ok(())
    }

    pub(crate) fn phase(&self, name: &str, total: Option<usize>) -> Result<()> {
        self.progress.set_position(0);
        self.progress.set_message(String::new());

        self.progress
            .set_prefix(console::style(name).for_stderr().bold().to_string());

        let template = if let Some(total) = total {
            self.progress.set_length(total as u64);

            "{spinner} {prefix} {pos}/{len} {wide_msg}"
        } else {
            "{spinner} {prefix} {wide_msg}"
        };

        self.progress
            .set_style(ProgressStyle::with_template(template)?);

        self.progress.tick();

        self.progress
            .enable_steady_tick(std::time::Duration::from_millis(100));

        Ok(())
    }

    pub(crate) fn progress(&self) -> ProgressBar {
        self.progress.clone()
    }

    pub(crate) fn observer(
        &self,
    ) -> Result<std::sync::Arc<dyn Fn(instar_analysis::Progress) + Send + Sync>> {
        let progress = self.progress.clone();
        let measured = ProgressStyle::with_template("{spinner} {prefix} {pos}/{len} {wide_msg}")?;
        let unmeasured = ProgressStyle::with_template("{spinner} {prefix} {wide_msg}")?;

        Ok(std::sync::Arc::new(move |event| {
            progress.set_style(if let Some(total) = event.total {
                progress.set_length(total as u64);

                measured.clone()
            } else {
                unmeasured.clone()
            });

            progress.set_prefix(console::style(event.phase).for_stderr().bold().to_string());
            progress.set_position(event.completed as u64);
            progress.set_message(event.source.unwrap_or_default());
        }))
    }

    pub(crate) fn flush(&mut self) -> Result<()> {
        if self.json {
            self.diagnostics.clear();
            self.files = SimpleFiles::new();

            return Ok(());
        }

        let writer = StandardStream::stderr(self.color);

        self.progress.suspend(|| {
            let mut writer = writer.lock();

            for diagnostic in self.diagnostics.drain(..) {
                term::emit_to_write_style(
                    &mut writer,
                    &term::Config::default(),
                    &self.files,
                    &diagnostic,
                )?;
            }

            writer.flush()?;

            Ok::<_, eyre::Report>(())
        })?;

        self.files = SimpleFiles::new();

        Ok(())
    }

    pub(crate) fn difference(&self, path: &Path) {
        if !self.json {
            self.progress
                .suspend(|| eprintln!("Would reformat {}", path.display()));
        }
    }

    fn label(
        &mut self,
        project: &mut Project,
        location: &Location<Origin>,
        primary: bool,
    ) -> Result<Option<Label<usize>>> {
        if location.revision == 0 {
            return Ok(None);
        }

        let source = project.source(location.module.source())?;

        ensure!(
            source.revision == location.revision,
            "diagnostic source revision changed"
        );

        let index = self.files.add(
            location.module.source().display().to_string(),
            source.text.to_string(),
        );

        let range = location.range[0]..location.range[1];

        Ok(Some(if primary {
            Label::primary(index, range)
        } else {
            Label::secondary(index, range)
        }))
    }

    pub(crate) fn diagnostic(
        &mut self,
        project: &mut Project,
        diagnostic: &instar_lint::Diagnostic<Origin>,
    ) -> Result<()> {
        let error = diagnostic.level == instar_lint::Level::Deny;
        self.failed |= error;
        let mut labels = Vec::new();

        if let Some(label) = self.label(project, &diagnostic.location, true)? {
            labels.push(label);
        }

        for related in &diagnostic.related {
            if let Some(label) = self.label(project, &related.location, false)? {
                labels.push(label.with_message(&related.message));
            }
        }

        let record = if error {
            Diagnostic::error()
        } else {
            Diagnostic::warning()
        };

        self.diagnostics.push(
            record
                .with_message(&diagnostic.message)
                .with_labels(labels)
                .with_notes(vec![
                    diagnostic.location.module.source().display().to_string(),
                ]),
        );

        self.records.push(json!({"path": diagnostic.location.module.source(), "revision": diagnostic.location.revision, "range": diagnostic.location.range, "kind": format!("{:?}", diagnostic.kind), "severity": if error {"error"} else {"warning"}, "message": diagnostic.message, "related": diagnostic.related.iter().map(related).collect::<Vec<_>>() }));

        Ok(())
    }

    pub(crate) fn formatting(&mut self, path: &Path, source: &str, result: &instar_format::Result) {
        if result.diagnostics.is_empty() {
            return;
        }

        let index = self
            .files
            .add(path.display().to_string(), source.to_owned());

        for diagnostic in &result.diagnostics {
            self.failed = true;

            self.diagnostics.push(
                Diagnostic::error()
                    .with_message(&diagnostic.message)
                    .with_labels(vec![Label::primary(
                        index,
                        diagnostic.range[0]..diagnostic.range[1],
                    )]),
            );

            self.records.push(json!({"path": path, "range": diagnostic.range, "severity": "error", "message": diagnostic.message}));
        }
    }

    pub(crate) fn finish(
        &mut self,
        command: &Command,
        completion: Completion,
        changed: &[String],
    ) -> Result<()> {
        self.progress.finish_and_clear();
        self.flush()?;

        if self.json {
            serde_json::to_writer(
                io::stdout().lock(),
                &json!({"diagnostics": self.records, "completion": completion, "changed": changed}),
            )?;

            writeln!(io::stdout().lock())?;
        } else {
            let operation = match command {
                Command::Format { .. } => "Formatting",
                Command::Check { .. } => "Checking",
                Command::Lint { .. } => "Linting",
            };

            let state = if completion == Completion::Complete {
                "complete"
            } else {
                "incomplete"
            };

            let details = match command {
                Command::Format { check, .. } => format!(
                    ", {} {}, {} unchanged, {} unsuccessful, {} skipped",
                    self.formatted,
                    if *check { "would change" } else { "formatted" },
                    self.unchanged,
                    self.unsuccessful,
                    self.skipped,
                ),

                _ => String::new(),
            };

            let files = self.processed + self.skipped;

            eprintln!(
                "{operation} {state}: {files} file{}{details}; {} diagnostic{}; {:.2}s",
                if files == 1 { "" } else { "s" },
                self.records.len(),
                if self.records.len() == 1 { "" } else { "s" },
                self.progress.elapsed().as_secs_f64(),
            );
        }

        Ok(())
    }
}

struct Writer(ProgressBar);

impl Write for Writer {
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

fn related(value: &Related<Origin>) -> Value {
    json!({"path": value.location.module.source(), "revision": value.location.revision, "range": value.location.range, "message": value.message})
}
