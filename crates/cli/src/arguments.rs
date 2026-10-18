use std::{collections::BTreeMap, num::NonZeroUsize, path::PathBuf};

use clap::{ColorChoice, Parser, Subcommand};
use instar_core::{assets::Target, lint};

use super::association::Association;

#[derive(Parser)]
#[command(version, about = "check, format, and lint Luau projects")]
pub(super) struct Arguments {
    #[arg(
        long,
        global = true,
        help = "show selected files and timing",
        conflicts_with = "quiet"
    )]
    pub(super) verbose: bool,

    #[arg(long, global = true, help = "suppress progress and success output")]
    pub(super) quiet: bool,

    #[arg(
        long,
        global = true,
        value_enum,
        value_name = "WHEN",
        help = "control diagnostic colors"
    )]
    pub(super) color: Option<ColorChoice>,

    #[arg(
        long,
        global = true,
        value_name = "PATH",
        help = "use this module path for stdin"
    )]
    pub(super) stdin_filepath: Option<PathBuf>,

    #[command(subcommand)]
    pub(super) command: Command,
}

#[derive(Subcommand)]
pub(super) enum Command {
    #[command(flatten)]
    Files(Box<Operation>),

    #[command(about = "generate local schema, Luau, and Roblox files")]
    Generate(Generate),
}

#[derive(Subcommand)]
pub(super) enum Operation {
    #[command(about = "check files, directories, patterns, or stdin (-)")]
    Check(Check),

    #[command(about = "format files, directories, patterns, or stdin (-)")]
    Format(Format),

    #[command(about = "lint files, directories, patterns, or stdin (-)")]
    Lint(Lint),
}

#[derive(clap::Args)]
pub(super) struct Generate {
    #[arg(
        value_name = "TARGET",
        help = "schema, luau, or roblox; defaults to all"
    )]
    pub(super) targets: Vec<Target>,

    #[arg(
        long,
        value_name = "DIRECTORY",
        help = "write selected files to this directory"
    )]
    pub(super) output: PathBuf,

    #[arg(
        long,
        value_name = "PATH",
        help = "resolve output and TOML tool configuration relative to this project directory"
    )]
    pub(super) root: Option<PathBuf>,

    #[arg(
        long,
        value_enum,
        value_name = "TOOL",
        help = "associate the generated schema in the project's TOML tool configuration"
    )]
    pub(super) associate: Option<Association>,

    #[arg(
        long,
        value_name = "COMMIT",
        help = "generate upstream files from this 40-character tracker commit"
    )]
    pub(super) revision: Option<String>,
}

impl Operation {
    pub(super) fn options(&self) -> &Options {
        match self {
            Self::Check(check) => &check.options,
            Self::Format(format) => &format.options,
            Self::Lint(lint) => &lint.options,
        }
    }

    pub(super) fn overrides(&self) -> Result<Vec<String>, serde_json::Error> {
        let options = self.options();
        let mut overrides = options.set.clone();

        let purpose = match self {
            Self::Check(_) => "check",
            Self::Format(_) => "format",
            Self::Lint(_) => "lint",
        };

        for (field, values) in [("include", &options.include), ("exclude", &options.exclude)] {
            if !values.is_empty() {
                overrides.push(format!(
                    "{purpose}.{field}={}",
                    serde_json::to_string(values)?
                ));
            }
        }

        if let Self::Check(Check {
            mode: Some(mode), ..
        }) = self
        {
            overrides.push(format!("check.mode={}", serde_json::to_string(mode)?));
        }

        if let Self::Lint(lint) = self
            && !lint.rule.is_empty()
        {
            let rules: BTreeMap<_, _> = lint::rules()
                .map(str::to_owned)
                .chain(lint.rule.iter().cloned())
                .map(|name| {
                    let enabled = lint.rule.contains(&name);

                    (name, BTreeMap::from([("enabled", enabled)]))
                })
                .collect();

            overrides.push(format!("lint.rules={}", serde_json::to_string(&rules)?));
        }

        Ok(overrides)
    }
}

#[derive(clap::Args)]
pub(super) struct Options {
    #[arg(
        value_name = "INPUT",
        help = "files, directories, patterns, or -; defaults to ."
    )]
    pub(super) inputs: Vec<PathBuf>,

    #[arg(long, help = "run again after filesystem changes")]
    pub(super) watch: bool,

    #[arg(
        long,
        value_name = "MILLISECONDS",
        default_value_t = 75,
        help = "wait for filesystem quiet before running; 0 disables the delay",
        requires = "watch"
    )]
    pub(super) debounce: u64,

    #[arg(
        long,
        value_name = "PATH",
        help = "resolve inputs relative to this project directory"
    )]
    pub(super) root: Option<PathBuf>,

    #[arg(
        long,
        value_name = "PATH",
        help = "use an explicit configuration file; repeat to layer files"
    )]
    pub(super) configuration: Vec<PathBuf>,

    #[arg(long, value_name = "PATTERN", num_args = 1.., help = "override inclusion patterns")]
    pub(super) include: Vec<String>,

    #[arg(long, value_name = "PATTERN", num_args = 1.., help = "override exclusion patterns")]
    pub(super) exclude: Vec<String>,

    #[arg(
        long,
        value_name = "FIELD=JSON",
        help = "override a configuration field; may be repeated"
    )]
    pub(super) set: Vec<String>,
}

#[derive(clap::Args)]
pub(super) struct Check {
    #[command(flatten)]
    pub(super) options: Options,

    #[arg(long, value_name = "MODE", value_parser = ["strict", "nonstrict", "nocheck"], help = "override the default checking mode")]
    pub(super) mode: Option<String>,
}

#[derive(clap::Args)]
pub(super) struct Format {
    #[command(flatten)]
    pub(super) options: Options,

    #[arg(
        long,
        value_name = "COUNT",
        help = "worker limit; defaults to available parallelism"
    )]
    pub(super) jobs: Option<NonZeroUsize>,

    #[arg(
        long,
        conflicts_with = "preview",
        help = "report unformatted files without writing"
    )]
    pub(super) check: bool,

    #[arg(long, help = "print a unified diff without writing")]
    pub(super) preview: bool,
}

#[derive(clap::Args)]
pub(super) struct Lint {
    #[command(flatten)]
    pub(super) options: Options,

    #[arg(
        long,
        value_name = "COUNT",
        help = "worker limit; defaults to available parallelism"
    )]
    pub(super) jobs: Option<NonZeroUsize>,

    #[arg(long, value_name = "RULE", num_args = 1.., help = "select only these lint rules")]
    pub(super) rule: Vec<String>,

    #[arg(long, help = "apply safe corrections")]
    pub(super) fix: bool,

    #[arg(
        long,
        requires = "fix",
        help = "print safe corrections as a unified diff without writing"
    )]
    pub(super) preview: bool,
}
