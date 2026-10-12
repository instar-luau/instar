//! Command-line interface for the Luau toolchain.

mod command;
mod report;

use std::{
    env,
    io::{self, IsTerminal as _},
    process::ExitCode,
};

use clap::{ColorChoice, CommandFactory as _, FromArgMatches as _, Parser as _};
use color_eyre::config::{HookBuilder, Theme};
use instar_luau::{run_configuration_worker, run_worker};
use instar_shared::OperationFailure;
use tokio::runtime::Builder;

use crate::command::{Arguments, CommandStatus, WorkerKind};

fn main() -> ExitCode {
    let values = env::args_os().collect::<Vec<_>>();

    if values.get(1).is_some_and(|value| value == "worker") {
        let result = match WorkerKind::parse_from(values.into_iter().skip(1)) {
            WorkerKind::Configuration => run_configuration_worker(),
            WorkerKind::Analysis => run_worker(),
            WorkerKind::Format => instar_format::run_worker(),
            WorkerKind::ProjectAssets => instar_project::run_asset_worker(),
        };

        return match result {
            Ok(()) => ExitCode::SUCCESS,

            Err(error) => {
                eprintln!("{error}");

                ExitCode::from(3)
            }
        };
    }

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
        Some("always") => ColorChoice::Always,
        Some("never") => ColorChoice::Never,
        _ if env::var_os("NO_COLOR").is_some_and(|value| !value.is_empty()) => ColorChoice::Never,
        _ => ColorChoice::Auto,
    };

    let matches = Arguments::command().color(color).get_matches_from(values);
    let arguments = Arguments::from_arg_matches(&matches).unwrap_or_else(|error| error.exit());

    let theme = if color == ColorChoice::Always
        || (color == ColorChoice::Auto && io::stderr().is_terminal())
    {
        Theme::dark()
    } else {
        Theme::new()
    };

    if let Err(error) = HookBuilder::default().theme(theme).install() {
        eprintln!("{error}");

        return ExitCode::from(3);
    }

    let result = Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(eyre::Report::from)
        .and_then(|runtime| runtime.block_on(command::run(arguments)));

    match result {
        Ok(outcome) => {
            tracing::debug!(completion = ?outcome.completion, "Command finished");

            ExitCode::from(outcome.status.code())
        }

        Err(error) => {
            let code = if let Some(failure) = error.downcast_ref::<OperationFailure>() {
                CommandStatus::failure(failure).code()
            } else if let Some(error) = error.downcast_ref::<io::Error>() {
                match error.kind() {
                    io::ErrorKind::InvalidInput | io::ErrorKind::InvalidData => 2,
                    io::ErrorKind::TimedOut | io::ErrorKind::Interrupted => 4,
                    _ => 3,
                }
            } else {
                3
            };

            eprintln!("{error:?}");

            ExitCode::from(code)
        }
    }
}
