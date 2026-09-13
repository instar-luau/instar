use clap::{Args, Subcommand};
use std::{io, path::PathBuf, process::ExitCode};

#[derive(Args)]
pub(super) struct Graft {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Install configured local and GitHub graft projects.
    Install {
        /// Project directory or instar.toml path.
        path: Option<PathBuf>,
    },
}

impl Graft {
    pub(super) fn run(self) -> io::Result<ExitCode> {
        let Command::Install { path } = self.command;
        let path = path.map_or_else(std::env::current_dir, Ok)?;

        for project in instar_core::graft::install(&path)? {
            println!("{}", project.display());
        }

        Ok(ExitCode::SUCCESS)
    }
}
