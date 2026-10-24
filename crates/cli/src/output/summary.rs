use std::{
    path::{Path, PathBuf},
    sync::atomic::Ordering,
    time::Instant,
};

use instar_core::assets::Generated;

use super::delivery::{self, HAS_DETAILS, Mode};

use crate::{
    arguments::{Arguments, Command, Operation},
    paths::relative,
};

#[derive(Default)]
pub(crate) struct Summary {
    pub(crate) errors: usize,
    pub(crate) failures: usize,
    pub(crate) warnings: usize,
    pub(crate) changed: usize,
}

impl Summary {
    pub(crate) fn status(&self, operation: &Operation) -> u8 {
        if self.failures != 0 {
            2
        } else {
            u8::from(
                self.errors != 0
                    || matches!(operation, Operation::Format(format) if format.should_check && self.changed != 0),
            )
        }
    }
}

pub(crate) fn summary(summary: &Summary, operation: &Operation, files: usize, started: Instant) {
    if delivery::mode() == Mode::Quiet {
        return;
    }

    let errors = summary.errors + summary.failures;

    let action = match operation {
        Operation::Check(_) => "Checked",
        Operation::Format(format) if format.should_check => "Checked formatting of",
        Operation::Format(format) if format.should_preview => "Previewed formatting for",
        Operation::Format(_) => "Formatted",
        Operation::Lint(lint) if lint.should_preview => "Previewed fixes for",
        Operation::Lint(_) => "Linted",
    };

    let action = console::style(action).for_stderr().bold();

    let action = if errors != 0 {
        action.red()
    } else if summary.warnings != 0
        || (matches!(operation, Operation::Format(format) if format.should_check)
            && summary.changed != 0)
    {
        action.yellow()
    } else {
        action.green()
    };

    let mut details = Vec::new();

    if errors != 0 {
        details.push(format!(
            "{errors} {}",
            delivery::plural(errors, "error", "errors")
        ));
    }

    if summary.warnings != 0 {
        details.push(format!(
            "{} {}",
            summary.warnings,
            delivery::plural(summary.warnings, "warning", "warnings"),
        ));
    }

    if summary.changed != 0 {
        let count = summary.changed;
        let word = delivery::plural(count, "file", "files");

        details.push(match operation {
            Operation::Format(format) if format.should_check => format!(
                "{count} {word} {} formatting",
                delivery::plural(count, "needs", "need"),
            ),

            Operation::Format(format) if format.should_preview => {
                format!("{count} {word} would change")
            }

            Operation::Lint(lint) if lint.should_preview => format!("{count} {word} would change"),

            Operation::Check(_) | Operation::Format(_) | Operation::Lint(_) => {
                format!("{count} {word} changed")
            }
        });
    }

    let detail = if details.is_empty() {
        if files == 0 {
            "No files selected.".to_owned()
        } else {
            match operation {
                Operation::Check(_) => "No errors.",
                Operation::Format(format) if format.should_check => "All files are formatted.",
                Operation::Format(_) => "No changes needed.",
                Operation::Lint(lint) if lint.should_preview => "No changes needed.",
                Operation::Lint(_) => "No errors or warnings.",
            }
            .to_owned()
        }
    } else {
        format!("{}.", details.join(delivery::separator()))
    };

    if HAS_DETAILS.swap(false, Ordering::Relaxed) {
        delivery::message("");
    }

    delivery::message(format_args!(
        "{action} {files} {} in {:.2}s\n  {detail}",
        delivery::plural(files, "file", "files"),
        started.elapsed().as_secs_f64(),
    ));
}

pub(crate) fn generated(
    generated: &Generated,
    root: &Path,
    association: Option<&Path>,
    started: Instant,
) {
    delivery::progress().finish_and_clear();

    if delivery::mode() == Mode::Quiet {
        return;
    }

    let width = if association.is_some() {
        "Associated".len()
    } else {
        "Generated".len()
    } + 2;

    for path in &generated.files {
        delivery::file("Generated", &relative(path, root), width);
    }

    if let Some(path) = association {
        delivery::file("Associated", &relative(path, root), width);
    }

    let detail = if generated.changed == 0 {
        "No changes needed.".to_owned()
    } else {
        format!(
            "{} {} changed.",
            generated.changed,
            delivery::plural(generated.changed, "file", "files"),
        )
    };

    delivery::message(format_args!(
        "{} {} {} in {:.2}s\n  {detail}",
        console::style("Generated").for_stderr().green().bold(),
        generated.files.len(),
        delivery::plural(generated.files.len(), "file", "files"),
        started.elapsed().as_secs_f64(),
    ));
}

pub(crate) fn associated(path: &Path, root: &Path, started: Instant) {
    delivery::progress().finish_and_clear();

    if delivery::mode() == Mode::Quiet {
        return;
    }

    delivery::file("Associated", &relative(path, root), "Associated".len() + 2);

    delivery::message(format_args!(
        "{} schema in {:.2}s",
        console::style("Associated").for_stderr().green().bold(),
        started.elapsed().as_secs_f64(),
    ));
}

pub(crate) fn refreshed(files: &[PathBuf], started: Instant) {
    delivery::progress().finish_and_clear();

    if delivery::mode() == Mode::Quiet {
        return;
    }

    for path in files {
        delivery::file("Refreshed", path, "Refreshed".len() + 2);
    }

    delivery::message(format_args!(
        "{} {} cached {} in {:.2}s",
        console::style("Refreshed").for_stderr().green().bold(),
        files.len(),
        delivery::plural(files.len(), "asset", "assets"),
        started.elapsed().as_secs_f64(),
    ));
}

pub(crate) fn interrupted(arguments: &Arguments) {
    delivery::progress().finish_and_clear();

    if delivery::mode() == Mode::Quiet {
        return;
    }

    let message = if matches!(
        &arguments.command,
        Command::Files(operation) if operation.options().should_watch
    ) {
        "Stopped watching."
    } else {
        match &arguments.command {
            Command::Assets(options) if options.should_generate => "Generation interrupted.",
            Command::Assets(options) if options.should_refresh => "Asset refresh interrupted.",
            Command::Assets(_) => "Schema association interrupted.",
            Command::Server => "Server interrupted.",

            Command::Files(operation) => match operation.as_ref() {
                Operation::Check(_) => "Check interrupted.",
                Operation::Format(_) => "Formatting interrupted.",
                Operation::Lint(_) => "Linting interrupted.",
            },
        }
    };

    delivery::message(console::style(message).for_stderr().yellow().bold());
}

pub(crate) fn unformatted(path: &Path) {
    delivery::message(format_args!(
        "{}  {}",
        console::style("Needs formatting").for_stderr().yellow(),
        path.display(),
    ));

    HAS_DETAILS.store(true, Ordering::Relaxed);
}
