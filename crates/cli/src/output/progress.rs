use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};

use eyre::{Result, WrapErr as _};
use indicatif::{ProgressBar, ProgressStyle};

use super::{delivery, report};
use crate::arguments::Operation;

#[derive(Clone)]
pub(crate) struct Progress {
    bar: ProgressBar,
    state: Arc<Mutex<State>>,
    root: PathBuf,
    action: &'static str,
    operation: &'static str,
    concurrent: bool,
    width: usize,
}

#[derive(Default)]
struct State {
    active: BTreeSet<String>,
    completed: BTreeSet<String>,
    total: Option<usize>,
    position: usize,
    begun: bool,
}

impl Progress {
    pub(crate) fn new(operation: &Operation, root: &Path) -> Result<Self> {
        let (action, operation_name) = match operation {
            Operation::Check(_) => ("Checking", "check"),
            Operation::Format(_) => ("Formatting", "formatting"),
            Operation::Lint(_) => ("Linting", "linting"),
        };

        let concurrent = !matches!(operation, Operation::Check(_));
        let bar = delivery::progress();

        bar.set_style(
            ProgressStyle::with_template("{msg}")
                .wrap_err("Could not configure the progress display.")?,
        );

        delivery::root(root);

        let progress = Self {
            bar,
            state: Arc::default(),
            root: root.to_owned(),
            action,
            operation: operation_name,
            concurrent,
            width: action.len().max("Writing".len()) + 2,
        };

        progress.update(&State::default());

        Ok(progress)
    }

    pub(crate) fn reset(&self, files: usize) {
        let state = State {
            total: self.concurrent.then_some(files),
            begun: self.action == "Writing",
            ..State::default()
        };

        self.bar.reset();
        self.update(&state);

        *self
            .state
            .lock()
            .expect("Internal error: the progress-state lock is poisoned.") = state;

        if !self.bar.is_hidden() {
            self.bar.enable_steady_tick(Duration::from_millis(100));
        }
    }

    pub(crate) fn writing(&self, files: usize) -> Self {
        let progress = Self {
            state: Arc::default(),
            action: "Writing",
            operation: "writes",
            concurrent: true,
            ..self.clone()
        };

        progress.reset(files);
        progress.bar.force_draw();

        progress
    }

    pub(crate) fn start(&self, module: &str) {
        let mut state = self
            .state
            .lock()
            .expect("Internal error: the progress-state lock is poisoned.");

        state.begun = true;

        if !self.concurrent {
            state.active.clear();
        }

        if state.active.insert(module.to_owned()) {
            delivery::file(
                self.action,
                &report::relative(Path::new(module), &self.root),
                self.width,
            );
        }

        self.update(&state);
    }

    pub(crate) fn finish(&self, module: &str) {
        let mut state = self
            .state
            .lock()
            .expect("Internal error: the progress-state lock is poisoned.");

        state.active.remove(module);
        state.completed.insert(module.to_owned());

        if self.concurrent {
            state.position = state.completed.len();
        }

        self.update(&state);
    }

    pub(crate) fn process<Value>(&self, module: &str, execute: impl FnOnce() -> Value) -> Value {
        self.start(module);
        let result = execute();
        self.finish(module);

        result
    }

    pub(crate) fn advance(&self, completed: usize, total: usize) {
        let mut state = self
            .state
            .lock()
            .expect("Internal error: the progress-state lock is poisoned.");

        state.active.clear();
        state.begun = true;
        state.total = Some(total);
        state.position = completed;
        self.update(&state);
    }

    pub(crate) fn files(&self) -> usize {
        self.state
            .lock()
            .expect("Internal error: the progress-state lock is poisoned.")
            .completed
            .len()
    }

    pub(crate) fn clear(&self) {
        self.bar.finish_and_clear();
    }

    fn update(&self, state: &State) {
        if self.bar.is_hidden() {
            return;
        }

        let complete =
            state.active.is_empty() && state.total.is_some_and(|total| state.position == total);

        let mut message = if !state.begun {
            console::style(format!(
                "Preparing {}{}",
                self.operation,
                delivery::ellipsis()
            ))
            .for_stderr()
            .cyan()
            .bold()
            .to_string()
        } else if complete {
            console::style(format!(
                "Finalizing {}{}",
                self.operation,
                delivery::ellipsis()
            ))
            .for_stderr()
            .cyan()
            .bold()
            .to_string()
        } else {
            let counts = state.total.map_or_else(String::new, |total| {
                let unit = if self.concurrent { "file" } else { "module" };

                let plural = if self.concurrent { "files" } else { "modules" };

                format!(
                    "{}/{total} {}",
                    state.position,
                    delivery::plural(total, unit, plural),
                )
            });

            let action = console::style(self.action).for_stderr().cyan().bold();

            if counts.is_empty() {
                action.to_string()
            } else {
                format!("{action}  {counts}")
            }
        };

        let suffix = if state.active.len() > 1 {
            format!("{}{} active", delivery::separator(), state.active.len())
        } else {
            String::new()
        };

        if let Some(module) = state.active.first() {
            let path = report::relative(Path::new(module), &self.root)
                .display()
                .to_string();

            let width = usize::from(console::Term::stderr().size().1);

            let available = width.saturating_sub(
                console::measure_text_width(&message) + console::measure_text_width(&suffix) + 2,
            );

            if available > console::measure_text_width(delivery::ellipsis()) {
                message.push_str("  ");

                message.push_str(&console::truncate_str(
                    &path,
                    available,
                    delivery::ellipsis(),
                ));
            }
        }

        message.push_str(&suffix);
        self.bar.set_message(message);
    }
}
