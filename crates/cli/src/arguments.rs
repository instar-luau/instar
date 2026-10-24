use std::{collections::BTreeMap, env, num::NonZeroUsize, path::PathBuf};

use clap::{ColorChoice, CommandFactory as _, FromArgMatches as _, Parser, Subcommand};
use instar_core::{assets::Target, lint};

use super::{assets::Association, output};

#[derive(Parser)]
#[command(
    name = "instar",
    version,
    about = "Check, format, lint, and serve Luau projects",
    styles = output::styles()
)]
pub(super) struct Arguments {
    #[arg(
        id = "verbose",
        long = "verbose",
        global = true,
        help = "Show project details and file activity",
        conflicts_with = "quiet"
    )]
    pub(super) is_verbose: bool,

    #[arg(
        id = "debug",
        long = "debug",
        global = true,
        help = "Show verbose output and internal debug logs",
        conflicts_with = "quiet"
    )]
    pub(super) is_debug: bool,

    #[arg(
        id = "quiet",
        long = "quiet",
        global = true,
        help = "Hide progress and summaries"
    )]
    pub(super) is_quiet: bool,

    #[arg(
        long,
        global = true,
        value_enum,
        value_name = "WHEN",
        help = "Control terminal colors: auto, always, never"
    )]
    pub(super) color: Option<ColorChoice>,

    #[command(subcommand)]
    pub(super) command: Command,
}

impl Arguments {
    pub(super) fn parse() -> Self {
        let arguments: Vec<_> = env::args_os().collect();
        let mut color = ColorChoice::Auto;
        let mut values = arguments.iter().skip(1);

        while let Some(value) = values.next() {
            if value == "--" {
                break;
            }

            let value = if value == "--color" {
                values.next().and_then(|value| value.to_str())
            } else {
                value
                    .to_str()
                    .and_then(|value| value.strip_prefix("--color="))
            };

            if let Some(value) = value {
                color = match value {
                    "always" => ColorChoice::Always,
                    "never" => ColorChoice::Never,
                    _ => ColorChoice::Auto,
                };
            }
        }

        let matches = Self::command()
            .color(color)
            .try_get_matches_from(arguments)
            .unwrap_or_else(|error| error.exit());

        Self::from_arg_matches(&matches).unwrap_or_else(|error| error.exit())
    }
}

#[derive(Subcommand)]
pub(super) enum Command {
    #[command(flatten)]
    Files(Box<Operation>),

    #[command(about = "Generate assets, associate schemas, or refresh the downloaded asset cache")]
    Assets(Assets),

    #[command(about = "Serve the Language Server Protocol over stdin and stdout")]
    Server,
}

#[derive(Subcommand)]
pub(super) enum Operation {
    #[command(about = "Check files, directories, patterns, or stdin (-)")]
    Check(Check),

    #[command(about = "Format files, directories, patterns, or stdin (-)")]
    Format(Format),

    #[command(about = "Lint files, directories, patterns, or stdin (-)")]
    Lint(Lint),
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
                    let is_enabled = lint.rule.contains(&name);

                    (name, BTreeMap::from([("enabled", is_enabled)]))
                })
                .collect();

            overrides.push(format!("lint.rules={}", serde_json::to_string(&rules)?));
        }

        Ok(overrides)
    }
}

#[derive(clap::Args)]
#[command(group(clap::ArgGroup::new("action").required(true).multiple(true).args(["generate", "refresh", "associate"])))]
pub(super) struct Assets {
    #[arg(
        id = "generate",
        long = "generate",
        requires = "output",
        help = "Generate local schema, Luau, and Roblox files"
    )]
    pub(super) should_generate: bool,

    #[arg(
        id = "refresh",
        long = "refresh",
        conflicts_with_all = ["generate", "targets", "output", "root", "associate"],
        help = "Download fresh assets into the disk cache"
    )]
    pub(super) should_refresh: bool,

    #[arg(
        value_name = "TARGET",
        requires = "generate",
        help = "Select schema or environments. Defaults to all targets"
    )]
    pub(super) targets: Vec<Target>,

    #[arg(
        long,
        value_name = "DIRECTORY",
        requires = "generate",
        help = "Write generated files to this directory"
    )]
    pub(super) output: Option<PathBuf>,

    #[arg(
        long,
        value_name = "PATH",
        help = "Resolve output and TOML tool configuration from this project directory"
    )]
    pub(super) root: Option<PathBuf>,

    #[arg(
        long,
        value_enum,
        value_name = "TOOL",
        help = "Associate a generated or published schema with the project's TOML tool configuration"
    )]
    pub(super) associate: Option<Association>,
}

#[derive(clap::Args)]
pub(super) struct Options {
    #[arg(
        value_name = "INPUT",
        help = "Select files, directories, patterns, or - for stdin. Defaults to ."
    )]
    pub(super) inputs: Vec<PathBuf>,

    #[arg(long, value_name = "PATH", help = "Use this module path for stdin")]
    pub(super) stdin_filepath: Option<PathBuf>,

    #[arg(id = "watch", long = "watch", help = "Run again when files change")]
    pub(super) should_watch: bool,

    #[arg(
        long,
        value_name = "MILLISECONDS",
        default_value_t = 75,
        help = "Wait for filesystem changes to settle before running. Use 0 for no delay",
        requires = "watch"
    )]
    pub(super) debounce: u64,

    #[arg(
        long,
        value_name = "PATH",
        help = "Resolve inputs from this project directory"
    )]
    pub(super) root: Option<PathBuf>,

    #[arg(
        long,
        value_name = "PATH",
        help = "Use a configuration file. Repeat to layer files"
    )]
    pub(super) configuration: Vec<PathBuf>,

    #[arg(long, value_name = "PATTERN", num_args = 1.., help = "Override inclusion patterns")]
    pub(super) include: Vec<String>,

    #[arg(long, value_name = "PATTERN", num_args = 1.., help = "Override exclusion patterns")]
    pub(super) exclude: Vec<String>,

    #[arg(
        long,
        value_name = "FIELD=JSON",
        help = "Override a configuration field with JSON. Repeat for multiple fields"
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
        help = "Limit worker count. Defaults to available parallelism"
    )]
    pub(super) jobs: Option<NonZeroUsize>,

    #[arg(
        id = "check",
        long = "check",
        conflicts_with = "preview",
        help = "Report files that need formatting without writing changes"
    )]
    pub(super) should_check: bool,

    #[arg(
        id = "preview",
        long = "preview",
        help = "Show formatting changes as a unified diff without writing"
    )]
    pub(super) should_preview: bool,
}

#[derive(clap::Args)]
pub(super) struct Lint {
    #[command(flatten)]
    pub(super) options: Options,

    #[arg(
        long,
        value_name = "COUNT",
        help = "Limit worker count. Defaults to available parallelism"
    )]
    pub(super) jobs: Option<NonZeroUsize>,

    #[arg(long, value_name = "RULE", num_args = 1.., help = "Run only the selected lint rules")]
    pub(super) rule: Vec<String>,

    #[arg(id = "corrections", long = "fix", help = "Apply safe fixes")]
    pub(super) should_fix: bool,

    #[arg(
        id = "preview",
        long = "preview",
        requires = "corrections",
        help = "Show safe fixes as a unified diff without writing"
    )]
    pub(super) should_preview: bool,
}
