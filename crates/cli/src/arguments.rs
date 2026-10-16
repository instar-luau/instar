use std::{num::NonZeroUsize, path::PathBuf};

use clap::{ColorChoice, Parser, Subcommand};

#[derive(Parser)]
#[command(version, about = "Check Luau projects")]
pub(super) struct Arguments {
    #[arg(
        long,
        global = true,
        help = "Show selected files and timing",
        conflicts_with = "quiet"
    )]
    pub(super) verbose: bool,

    #[arg(long, global = true, help = "Suppress progress and success output")]
    pub(super) quiet: bool,

    #[arg(
        long,
        global = true,
        value_enum,
        value_name = "WHEN",
        help = "Control diagnostic colors"
    )]
    pub(super) color: Option<ColorChoice>,

    #[arg(
        long,
        global = true,
        value_name = "PATH",
        help = "Use this module path for stdin"
    )]
    pub(super) stdin_filepath: Option<PathBuf>,

    #[command(subcommand)]
    pub(super) command: Command,
}

#[derive(Subcommand)]
pub(super) enum Command {
    #[command(about = "Check files, directories, patterns, or stdin (-)")]
    Check(Check),
}

#[derive(clap::Args)]
pub(super) struct Options {
    #[arg(
        value_name = "INPUT",
        help = "Files, directories, patterns, or -; defaults to ."
    )]
    pub(super) inputs: Vec<PathBuf>,

    #[arg(
        long,
        value_name = "COUNT",
        help = "Worker limit; defaults to available parallelism"
    )]
    pub(super) jobs: Option<NonZeroUsize>,

    #[arg(long, help = "Run again after filesystem changes")]
    pub(super) watch: bool,

    #[arg(
        long,
        value_name = "MILLISECONDS",
        default_value_t = 75,
        help = "Wait for filesystem quiet before running; 0 disables the delay",
        requires = "watch"
    )]
    pub(super) debounce: u64,

    #[arg(
        long,
        value_name = "PATH",
        help = "Resolve inputs relative to this project directory"
    )]
    pub(super) project_root: Option<PathBuf>,

    #[arg(
        long,
        value_name = "PATH",
        help = "Use an explicit configuration file; repeat to layer files"
    )]
    pub(super) configuration: Vec<PathBuf>,

    #[arg(long, value_name = "PATTERN", num_args = 1.., help = "Override inclusion patterns")]
    pub(super) include: Vec<String>,

    #[arg(long, value_name = "PATTERN", num_args = 1.., help = "Override exclusion patterns")]
    pub(super) exclude: Vec<String>,

    #[arg(
        long,
        value_name = "FIELD=JSON",
        help = "Override a configuration field; may be repeated"
    )]
    pub(super) set: Vec<String>,
}

#[derive(clap::Args)]
pub(super) struct Check {
    #[command(flatten)]
    pub(super) options: Options,

    #[arg(long, value_name = "MODE", value_parser = ["strict", "nonstrict", "nocheck"], help = "Override the default checking mode")]
    pub(super) mode: Option<String>,
}

impl Command {
    pub(super) fn options(&self) -> &Options {
        let Self::Check(check) = self;
        &check.options
    }

    pub(super) fn overrides(&self) -> Result<Vec<String>, serde_json::Error> {
        let options = self.options();
        let mut overrides = options.set.clone();

        for (field, values) in [("include", &options.include), ("exclude", &options.exclude)] {
            if !values.is_empty() {
                overrides.push(format!("check.{field}={}", serde_json::to_string(values)?));
            }
        }

        if let Self::Check(Check {
            mode: Some(mode), ..
        }) = self
        {
            overrides.push(format!("check.mode={}", serde_json::to_string(mode)?));
        }

        Ok(overrides)
    }
}
