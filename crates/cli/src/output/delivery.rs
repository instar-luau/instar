use std::{
    env, fmt,
    path::Path,
    sync::{OnceLock, atomic::AtomicBool},
    time::Duration,
};

use clap::{
    ColorChoice,
    builder::styling::{AnsiColor, Styles},
};

use eyre::{Result, WrapErr as _, bail};
use indicatif::{ProgressBar, ProgressStyle};

use super::logging;
use crate::arguments::Arguments;

pub(super) static HAS_DETAILS: AtomicBool = AtomicBool::new(false);

static DISPLAY: OnceLock<Display> = OnceLock::new();

#[derive(Copy, Clone, Eq, PartialEq)]
pub(super) enum Mode {
    Quiet,
    Normal,
    Verbose,
    Debug,
}

impl Mode {
    pub(super) fn is_detailed(self) -> bool {
        matches!(self, Self::Verbose | Self::Debug)
    }
}

struct Display {
    mode: Mode,
    progress: ProgressBar,
}

pub(crate) fn initialize(arguments: &Arguments) -> Result<()> {
    match arguments.color.unwrap_or_default() {
        ColorChoice::Always => console::set_colors_enabled_stderr(true),
        ColorChoice::Never => console::set_colors_enabled_stderr(false),
        ColorChoice::Auto => {}
    }

    let mode = if arguments.is_quiet {
        Mode::Quiet
    } else if arguments.is_debug {
        Mode::Debug
    } else if arguments.is_verbose {
        Mode::Verbose
    } else {
        Mode::Normal
    };

    if DISPLAY
        .set(Display {
            mode,
            progress: if mode == Mode::Quiet || !is_interactive() {
                ProgressBar::hidden()
            } else {
                ProgressBar::new_spinner()
            },
        })
        .is_err()
    {
        bail!("Internal error: output is already initialized.");
    }

    logging::initialize(mode)
}

pub(super) fn mode() -> Mode {
    display().mode
}

pub(super) fn progress() -> ProgressBar {
    display().progress.clone()
}

pub(super) fn suspend<Value>(execute: impl FnOnce() -> Value) -> Value {
    display().progress.suspend(execute)
}

pub(super) fn message(message: impl fmt::Display) {
    suspend(|| eprintln!("{message}"));
}

pub(super) fn is_interactive() -> bool {
    console::Term::stderr().is_term()
        && env::var_os("TERM").is_none_or(|terminal| terminal != "dumb")
}

pub(super) fn ellipsis() -> &'static str {
    if is_interactive() { "…" } else { "..." }
}

pub(super) fn separator() -> &'static str {
    if is_interactive() { " · " } else { "; " }
}

pub(crate) fn root(path: &Path) {
    file("Project", path, 9);
}

pub(super) fn file(action: &str, path: &Path, width: usize) {
    if mode().is_detailed() {
        message(format_args!(
            "{}{:padding$}{}",
            console::style(action).for_stderr().cyan().bold(),
            "",
            path.display(),
            padding = width.saturating_sub(console::measure_text_width(action)),
        ));
    }
}

pub(crate) fn workers(count: usize) {
    if mode().is_detailed() {
        message(format_args!(
            "{}  {count}\n",
            console::style("Workers").for_stderr().cyan().bold(),
        ));
    }
}

pub(crate) fn watching() {
    if mode() != Mode::Quiet {
        message(format_args!(
            "\n{}{}",
            console::style("Watching for changes")
                .for_stderr()
                .cyan()
                .bold(),
            ellipsis(),
        ));
    }
}

pub(crate) fn generator(_: tracing::Level, source: &str, line: &str) {
    let name = source.rsplit(['/', '\\']).next().unwrap_or(source);
    let name = neutralize(name);
    tracing::debug!(generator = source, "Received generator output");

    for line in line.split('\n') {
        let line = neutralize(line);
        message(format_args!("[{name}] {line}"));
    }
}

fn neutralize(text: &str) -> String {
    let text = console::strip_ansi_codes(text);

    let mut output = String::new();

    for character in text.chars() {
        if character.is_control() && character != '\t' {
            output.extend(character.escape_default());
        } else {
            output.push(character);
        }
    }

    output
}

pub(crate) fn phase(phase: &str, target: Option<&str>) -> Result<()> {
    let bar = progress();

    bar.set_style(
        ProgressStyle::with_template("{msg}")
            .wrap_err("Could not configure the progress display.")?,
    );

    let message = target.map_or_else(
        || format!("{phase}{}", ellipsis()),
        |target| {
            let width = usize::from(console::Term::stderr().size().1);
            let available = width.saturating_sub(console::measure_text_width(phase) + 2);

            if available <= ellipsis().len() {
                phase.to_owned()
            } else {
                format!(
                    "{phase}  {}",
                    console::truncate_str(target, available, ellipsis())
                )
            }
        },
    );

    bar.set_message(
        console::style(message)
            .for_stderr()
            .cyan()
            .bold()
            .to_string(),
    );

    if !bar.is_hidden() {
        bar.enable_steady_tick(Duration::from_millis(100));
    }

    Ok(())
}

pub(super) fn plural(count: usize, singular: &'static str, plural: &'static str) -> &'static str {
    if count == 1 { singular } else { plural }
}

pub(crate) fn styles() -> Styles {
    Styles::styled()
        .header(AnsiColor::Cyan.on_default().bold())
        .usage(AnsiColor::Cyan.on_default().bold())
        .literal(AnsiColor::Green.on_default().bold())
        .error(AnsiColor::Red.on_default().bold())
        .valid(AnsiColor::Green.on_default())
        .invalid(AnsiColor::Yellow.on_default())
}

fn display() -> &'static Display {
    DISPLAY
        .get()
        .expect("Internal error: output is not initialized.")
}
