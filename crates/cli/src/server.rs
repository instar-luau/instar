use std::{env, io, sync::Arc, time::Duration};

use eyre::Result;
use instar_analysis::{Options, process};
use tracing::Level;
use tracing_subscriber::fmt;

use crate::arguments::Arguments;

pub(crate) fn run(arguments: &Arguments) -> Result<bool> {
    process::set_host(&env::current_exe()?)?;

    let subscriber = fmt()
        .with_writer(io::stderr)
        .with_ansi(false)
        .with_max_level(if arguments.verbose {
            Level::DEBUG
        } else {
            Level::WARN
        });

    let result = if arguments.json {
        subscriber.json().try_init()
    } else {
        subscriber.try_init()
    };

    result.map_err(|error| eyre::eyre!("{error}"))?;
    let mut options = Options::new(Duration::from_secs(arguments.timeout));

    options.progress = Some(Arc::new(|progress| {
        tracing::debug!(phase = progress.phase, "Analysis progress");
    }));

    Ok(instar_server::stdio(options)?)
}
