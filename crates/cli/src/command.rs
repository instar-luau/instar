use std::process::ExitCode;

use clap::Parser as _;

use super::{
    arguments::{Arguments, Command},
    execution, generate, interrupt, report,
};

pub(super) fn run() -> ExitCode {
    let arguments = Arguments::parse();

    let result = report::initialize(arguments.color, arguments.verbose).and_then(|()| {
        interrupt::initialize()?;

        let result = match &arguments.command {
            Command::Generate(options) => generate::run(&arguments, options),
            Command::Files(operation) => execution::run(&arguments, operation),
            Command::Server => instar_server::run(interrupt::requested),
        };

        if interrupt::requested() {
            report::interrupted(&arguments);

            let watching = matches!(
                &arguments.command,
                Command::Files(operation) if operation.options().watch
            );

            Ok(if watching { 0 } else { 130 })
        } else {
            result
        }
    });

    match result {
        Ok(status) => ExitCode::from(status),

        Err(error) => {
            report::failure(&error);

            ExitCode::from(2)
        }
    }
}
