use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};

use eyre::{Result, WrapErr as _};
use indicatif::{ProgressBar, ProgressStyle};

use super::{
    arguments::{Arguments, Command},
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
    pub(super) fn new(arguments: &Arguments, root: &Path) -> Result<Self> {
        let action = match arguments.command {
            Command::Check(_) => "checking",
            Command::Format(_) => "formatting",
            Command::Lint(_) => "linting",
        };

        let bar = if arguments.quiet {
            ProgressBar::hidden()
        } else {
            ProgressBar::new_spinner()
        };

        let bar = bar.with_style(
            ProgressStyle::with_template("{spinner} {prefix} {wide_msg}")
                .wrap_err("cannot configure progress display")?,
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
            concurrent: !matches!(arguments.command, Command::Check(_)),
        })
    }

    pub(super) fn reset(&self) {
        *self.state.lock().expect("progress state") = State::default();
        self.bar.reset();
        self.bar.set_message(String::new());

        if !self.bar.is_hidden() {
            self.bar.enable_steady_tick(Duration::from_millis(100));
        }
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
        self.update(&state);
    }

    pub(super) fn process<Value>(&self, module: &str, execute: impl FnOnce() -> Value) -> Value {
        self.start(module);
        let result = execute();
        self.finish(module);

        result
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

        let path = state.active.first().map_or_else(PathBuf::new, |module| {
            report::relative(Path::new(module), &self.root)
        });

        let mut message = format!(
            "{}  {} {}",
            console::style(path.display()).for_stderr().magenta(),
            console::style(state.completed.len())
                .for_stderr()
                .blue()
                .bold(),
            console::style("completed").for_stderr().dim()
        );

        if state.active.len() > 1 {
            message.push_str(&format!(
                "; {} {}",
                console::style(state.active.len())
                    .for_stderr()
                    .cyan()
                    .bold(),
                console::style("active").for_stderr().dim()
            ));
        }

        self.bar.set_message(message);
    }
}
