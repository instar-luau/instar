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

        match &arguments.command {
            Command::Generate(options) => {
                let result = generate::run(&arguments, options);

                if interrupt::requested() {
                    report::generation_interrupted(&arguments);

                    Ok(130)
                } else {
                    result
                }
            }

            Command::Files(operation) => {
                let result = execution::run(&arguments, operation);

                if interrupt::requested() {
                    report::interrupted(&arguments, operation);

                    return Ok(if operation.options().watch { 0 } else { 130 });
                }

                result
            }
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
