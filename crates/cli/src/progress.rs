use std::{
    collections::BTreeSet,
    fmt::Write as _,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use eyre::{Result, WrapErr as _};
use indicatif::{ProgressBar, ProgressStyle};

use super::{
    arguments::{Arguments, Operation},
    report,
};

#[derive(Clone)]
pub(super) struct Progress {
    bar: ProgressBar,
    state: Arc<Mutex<State>>,
    root: PathBuf,
    action: &'static str,
    verbose: bool,
    concurrent: bool,
}

#[derive(Default)]
struct State {
    active: BTreeSet<String>,
    completed: BTreeSet<String>,
}

impl Progress {
    pub(super) fn new(arguments: &Arguments, operation: &Operation, root: &Path) -> Result<Self> {
        let action = match operation {
            Operation::Check(_) => "checking",
            Operation::Format(_) => "formatting",
            Operation::Lint(_) => "linting",
        };

        let bar = if arguments.quiet {
            ProgressBar::hidden()
        } else {
            ProgressBar::new_spinner()
        };

        let bar = bar.with_message(console::style("preparing").for_stderr().dim().to_string());

        let template = if console::colors_enabled_stderr() {
            "{prefix} [{bar:24.blue}] {wide_msg}"
        } else {
            "{prefix} [{bar:24}] {wide_msg}"
        };

        let bar = bar.with_style(
            ProgressStyle::with_template(template)
                .wrap_err("cannot configure progress display")?
                .progress_chars("━━─"),
        );

        bar.set_prefix(
            console::style(action)
                .for_stderr()
                .cyan()
                .bold()
                .to_string(),
        );

        if arguments.verbose {
            eprintln!(
                "{}: `{}`",
                console::style("root").for_stderr().cyan(),
                console::style(root.display()).for_stderr().magenta()
            );
        }

        Ok(Self {
            bar,
            state: Arc::default(),
            root: root.to_owned(),
            action,
            verbose: arguments.verbose,
            concurrent: !matches!(operation, Operation::Check(_)),
        })
    }

    pub(super) fn reset(&self, files: usize) {
        *self.state.lock().expect("progress state") = State::default();
        self.bar.reset();

        if self.concurrent {
            self.bar.set_length(files as u64);
        } else {
            self.bar.unset_length();
        }

        self.update(&State::default());
    }

    pub(super) fn start(&self, module: &str) {
        let mut state = self.state.lock().expect("progress state");

        if !self.concurrent {
            state.active.clear();
        }

        if state.active.insert(module.to_owned()) && self.verbose {
            self.bar.suspend(|| {
                eprintln!(
                    "{} `{}`",
                    console::style(self.action).for_stderr().cyan().bold(),
                    console::style(report::relative(Path::new(module), &self.root).display())
                        .for_stderr()
                        .magenta()
                );
            });
        }

        self.update(&state);
    }

    pub(super) fn finish(&self, module: &str) {
        let mut state = self.state.lock().expect("progress state");
        state.active.remove(module);
        state.completed.insert(module.to_owned());

        if self.concurrent {
            self.bar.set_position(state.completed.len() as u64);
        }

        self.update(&state);
    }

    pub(super) fn process<Value>(&self, module: &str, execute: impl FnOnce() -> Value) -> Value {
        self.start(module);
        let result = execute();
        self.finish(module);

        result
    }

    pub(super) fn advance(&self, completed: usize, total: usize) {
        let mut state = self.state.lock().expect("progress state");
        state.active.clear();
        self.bar.set_length(total as u64);
        self.bar.set_position(completed as u64);
        self.update(&state);
    }

    pub(super) fn files(&self) -> usize {
        self.state.lock().expect("progress state").completed.len()
    }

    pub(super) fn clear(&self) {
        self.bar.finish_and_clear();
    }

    fn update(&self, state: &State) {
        if self.bar.is_hidden() {
            return;
        }

        let mut message = state.active.first().map_or_else(
            || {
                let phase = if self
                    .bar
                    .length()
                    .is_some_and(|total| total != 0 && self.bar.position() == total)
                {
                    "finalizing"
                } else {
                    "preparing"
                };

                console::style(phase).for_stderr().dim().to_string()
            },
            |module| {
                console::style(report::relative(Path::new(module), &self.root).display())
                    .for_stderr()
                    .magenta()
                    .to_string()
            },
        );

        if state.active.len() > 1 {
            write!(
                message,
                "; {} {}",
                console::style(state.active.len())
                    .for_stderr()
                    .cyan()
                    .bold(),
                console::style("active").for_stderr().dim()
            )
            .expect("internal error: cannot format progress");
        }

        self.bar.set_message(message);
    }
}
