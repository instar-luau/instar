//! Command-line interface for the Luau toolchain.

mod arguments;
mod completion;
mod execution;
mod inputs;
mod reporting;
mod writing;

use std::{
    io::{self, IsTerminal},
    process::ExitCode,
};

use arguments::{Arguments, Command};
use clap::{CommandFactory, FromArgMatches};

fn main() -> ExitCode {
    let values = std::env::args_os().collect::<Vec<_>>();

    let boundary = values
        .iter()
        .position(|value| value == "--")
        .unwrap_or(values.len());

    let color = values[..boundary]
        .iter()
        .enumerate()
        .filter_map(|(index, value)| {
            let text = value.to_str()?;

            text.strip_prefix("--color=")
                .map(str::to_owned)
                .or_else(|| {
                    (text == "--color")
                        .then(|| values.get(index + 1)?.to_str().map(str::to_owned))
                        .flatten()
                })
        })
        .next_back();

    let color = match color.as_deref() {
        Some("always") => clap::ColorChoice::Always,
        Some("never") => clap::ColorChoice::Never,

        _ if std::env::var_os("NO_COLOR").is_some_and(|value| !value.is_empty()) => {
            clap::ColorChoice::Never
        }

        _ => clap::ColorChoice::Auto,
    };

    let matches = Arguments::command().color(color).get_matches_from(values);
    let arguments = Arguments::from_arg_matches(&matches).unwrap_or_else(|error| error.exit());

    let theme = if !arguments.json
        && (color == clap::ColorChoice::Always
            || (color == clap::ColorChoice::Auto && io::stderr().is_terminal()))
    {
        color_eyre::config::Theme::dark()
    } else {
        color_eyre::config::Theme::new()
    };

    if let Err(error) = color_eyre::config::HookBuilder::default()
        .theme(theme)
        .install()
    {
        eprintln!("{error}");

        return ExitCode::from(2);
    }

    let result = match &arguments.command {
        Command::Analysis(command) => execution::run(&arguments, command),
        Command::Completions { shell } => completion::generate(shell).map(|()| true),
    };

    match result {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,

        Err(error) => {
            if arguments.json {
                eprintln!("{}", serde_json::json!({"error": format!("{error:#}")}));
            } else {
                eprintln!("{error:?}");
            }

            ExitCode::from(2)
        }
    }
}
