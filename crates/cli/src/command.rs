use std::process::ExitCode;

use super::{
    arguments::{Arguments, Command},
    execution, generate, interrupt, output,
};

pub(super) fn main() -> ExitCode {
    let arguments = Arguments::parse();

    let result = output::initialize(&arguments).and_then(|()| {
        interrupt::initialize()?;

        let result = match &arguments.command {
            Command::Generate(options) => generate::run(&arguments, options),
            Command::Files(operation) => execution::run(&arguments, operation),
            Command::Server => instar_server::run(interrupt::requested),
        };

        if interrupt::requested() {
            output::interrupted(&arguments);

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
            output::failure(&error);

            ExitCode::from(2)
        }
    }
}
