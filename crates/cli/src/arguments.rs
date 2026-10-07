use std::{num::NonZeroUsize, path::PathBuf};

use clap::{Parser, Subcommand, ValueEnum};

#[derive(Parser)]
#[command(version, about = "Format, lint, and check Luau sources")]
pub(crate) struct Arguments {
    #[arg(long, global = true, value_enum, default_value = "auto")]
    pub(crate) color: Color,

    #[arg(long, global = true)]
    pub(crate) json: bool,

    #[arg(long, global = true, default_value = "30", value_parser = clap::value_parser!(u64).range(1..))]
    pub(crate) timeout: u64,

    #[arg(
        long,
        global = true,
        value_name = "PATH",
        help = "Source path for stdin configuration and imports"
    )]
    pub(crate) stdin_filepath: Option<PathBuf>,

    #[command(subcommand)]
    pub(crate) command: Command,
}

#[derive(Clone, Copy, ValueEnum)]
pub(crate) enum Color {
    Auto,
    Always,
    Never,
}

#[derive(Subcommand)]
pub(crate) enum Command {
    #[command(about = "Typecheck source files and their dependencies")]
    Check { files: Vec<PathBuf> },

    #[command(about = "Format files in place, check formatting, or format stdin to stdout")]
    Format {
        files: Vec<PathBuf>,
        #[arg(long)]
        check: bool,
        #[arg(long)]
        jobs: Option<NonZeroUsize>,
    },

    #[command(about = "Lint source files and their dependencies")]
    Lint { files: Vec<PathBuf> },
}
