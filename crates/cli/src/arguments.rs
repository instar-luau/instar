use std::{collections::BTreeMap, num::NonZeroUsize, path::PathBuf};

use clap::{ColorChoice, Parser, Subcommand};

#[derive(Parser)]
#[command(version, about = "Check, format, and lint Luau projects")]
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

    #[command(about = "Format files, directories, patterns, or stdin (-)")]
    Format(Format),

    #[command(about = "Lint files, directories, patterns, or stdin (-)")]
    Lint(Lint),
}

#[derive(clap::Args)]
pub(super) struct Options {
    #[arg(
        value_name = "INPUT",
        help = "Files, directories, patterns, or -; defaults to ."
    )]
    pub(super) inputs: Vec<PathBuf>,

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

#[derive(clap::Args)]
pub(super) struct Format {
    #[command(flatten)]
    pub(super) options: Options,

    #[arg(
        long,
        value_name = "COUNT",
        help = "Worker limit; defaults to available parallelism"
    )]
    pub(super) jobs: Option<NonZeroUsize>,

    #[arg(
        long,
        conflicts_with = "preview",
        help = "Report unformatted files without writing"
    )]
    pub(super) check: bool,

    #[arg(long, help = "Print a unified diff without writing")]
    pub(super) preview: bool,
}

#[derive(clap::Args)]
pub(super) struct Lint {
    #[command(flatten)]
    pub(super) options: Options,

    #[arg(
        long,
        value_name = "COUNT",
        help = "Vermis worker limit; defaults to available parallelism"
    )]
    pub(super) jobs: Option<NonZeroUsize>,

    #[arg(long, value_name = "RULE", num_args = 1.., help = "Select only these lint rules")]
    pub(super) rule: Vec<String>,

    #[arg(long, conflicts_with = "fix_preview", help = "Apply safe corrections")]
    pub(super) fix: bool,

    #[arg(
        long,
        help = "Print safe corrections as a unified diff without writing"
    )]
    pub(super) fix_preview: bool,
}

impl Command {
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
            let rules: BTreeMap<_, _> = instar_lint::rules()
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
