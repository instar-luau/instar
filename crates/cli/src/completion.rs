use std::io::{self, Write};

use clap::CommandFactory;
use eyre::Result;

use crate::arguments::Arguments;

pub(crate) fn generate(shell: &str) -> Result<()> {
    let mut command = Arguments::command();
    let mut output = Vec::new();

    if shell == "nushell" {
        clap_complete::generate(
            clap_complete_nushell::Nushell,
            &mut command,
            "instar",
            &mut output,
        );
    } else {
        let shell = shell
            .parse::<clap_complete::Shell>()
            .map_err(eyre::Report::msg)?;

        clap_complete::generate(shell, &mut command, "instar", &mut output);
    }

    io::stdout().lock().write_all(&output)?;

    Ok(())
}
