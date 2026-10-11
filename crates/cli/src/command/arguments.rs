use std::{num::NonZeroUsize, path::PathBuf};

use clap::{Args, Parser, Subcommand, ValueEnum};

#[derive(Parser)]
#[command(version, about = "Format, lint, and check Luau sources")]
pub(crate) struct Arguments {
    #[command(flatten)]
    pub(crate) execution: ExecutionArguments,

    #[command(flatten)]
    pub(crate) reporting: ReportingArguments,

    #[command(subcommand)]
    pub(crate) command: Command,
}

#[derive(Args)]
pub(crate) struct ExecutionArguments {
    #[arg(long, global = true, default_value = "30", value_parser = clap::value_parser!(u64).range(1..), help = "Total invocation budget in seconds")]
    pub(crate) timeout: u64,

    #[arg(long, global = true)]
    pub(crate) jobs: Option<NonZeroUsize>,

    #[arg(long, global = true)]
    pub(crate) watch: bool,

    #[arg(long, global = true, help = "Write operational debug events to stderr")]
    pub(crate) verbose: bool,

    #[arg(
        long,
        global = true,
        value_name = "PATH",
        help = "Stdin configuration/import context; never a write destination"
    )]
    pub(crate) stdin_filepath: Option<PathBuf>,
}

#[derive(Args)]
pub(crate) struct ReportingArguments {
    #[arg(long = "report", global = true, value_enum, default_value = "human")]
    pub(crate) style: ReportStyle,

    #[arg(long, global = true)]
    pub(crate) json: bool,

    #[arg(long, global = true, value_enum, default_value = "auto")]
    pub(crate) color: Color,

    #[arg(long, global = true)]
    pub(crate) quiet: bool,

    #[arg(long = "output", global = true, value_name = "PATH")]
    pub(crate) destination: Option<PathBuf>,

    #[arg(long, global = true)]
    pub(crate) streaming: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum ReportStyle {
    Human,
    Json,
}

#[derive(Clone, Copy, ValueEnum)]
pub(crate) enum Color {
    Auto,
    Always,
    Never,
}

#[derive(Subcommand)]
pub(crate) enum Command {
    #[command(about = "Typecheck selected sources and their dependencies")]
    Check(CheckArguments),

    #[command(about = "Check, preview, emit, or explicitly write validated formatting")]
    Format(FormatArguments),

    #[command(about = "Lint selected sources and their dependencies")]
    Lint(LintArguments),

    #[command(about = "Run the language server over stdio")]
    Server,
}

#[derive(Args, Clone)]
pub(crate) struct InputArguments {
    #[arg(
        help = "Files, directories, quoted patterns, or '-' for stdin; project sources when omitted"
    )]
    pub(crate) files: Vec<PathBuf>,

    #[arg(long, value_name = "PATH")]
    pub(crate) project_root: Option<PathBuf>,

    #[arg(long, value_name = "PATH")]
    pub(crate) configuration: Option<PathBuf>,

    #[arg(long, value_name = "PATTERN", action = clap::ArgAction::Append)]
    pub(crate) include: Vec<String>,

    #[arg(long, value_name = "PATTERN", action = clap::ArgAction::Append)]
    pub(crate) exclude: Vec<String>,

    #[arg(long)]
    pub(crate) override_exclusions: bool,

    #[command(flatten)]
    pub(crate) explanations: ExplanationArguments,

    #[arg(long = "set", value_name = "FIELD=JSON", action = clap::ArgAction::Append)]
    pub(crate) settings: Vec<String>,
}

#[derive(Args, Clone)]
pub(crate) struct ExplanationArguments {
    #[arg(long)]
    pub(crate) explain_selection: bool,

    #[arg(long)]
    pub(crate) explain_settings: bool,

    #[arg(long)]
    pub(crate) explain_dependencies: bool,
}

#[derive(Args, Clone)]
pub(crate) struct DiagnosticArguments {
    #[arg(long, value_enum)]
    pub(crate) failure_threshold: Option<DiagnosticSeverity>,

    #[arg(long)]
    pub(crate) warnings_as_errors: bool,

    #[arg(long = "diagnostic-code", action = clap::ArgAction::Append)]
    pub(crate) codes: Vec<String>,

    #[arg(long = "diagnostic-category", action = clap::ArgAction::Append)]
    pub(crate) categories: Vec<String>,

    #[arg(long = "diagnostic-severity", value_enum, action = clap::ArgAction::Append)]
    pub(crate) severities: Vec<DiagnosticSeverity>,
}

#[derive(Clone, Copy, ValueEnum)]
pub(crate) enum DiagnosticSeverity {
    Error,
    Warning,
    Information,
    Hint,
}

#[derive(Args, Clone)]
pub(crate) struct CheckArguments {
    #[command(flatten)]
    pub(crate) inputs: InputArguments,

    #[command(flatten)]
    pub(crate) diagnostics: DiagnosticArguments,

    #[arg(long, value_enum)]
    pub(crate) mode: Option<CheckingMode>,
}

#[derive(Clone, Copy, ValueEnum)]
pub(crate) enum CheckingMode {
    Strict,
    Nonstrict,
    Nocheck,
}

#[derive(Args, Clone)]
pub(crate) struct FormatArguments {
    #[command(flatten)]
    pub(crate) inputs: InputArguments,

    #[arg(long, conflicts_with_all = ["preview", "source", "write"])]
    pub(crate) check: bool,

    #[arg(long, conflicts_with_all = ["check", "source", "write"])]
    pub(crate) preview: bool,

    #[command(flatten)]
    pub(crate) output: FormatOutputArguments,

    #[arg(long = "range", value_name = "START:END", action = clap::ArgAction::Append, conflicts_with = "on_type")]
    pub(crate) ranges: Vec<String>,

    #[arg(long, value_name = "OFFSET:CHAR", conflicts_with = "ranges")]
    pub(crate) on_type: Option<String>,

    #[arg(long)]
    pub(crate) width: Option<usize>,

    #[arg(long, value_enum)]
    pub(crate) indentation: Option<Indentation>,

    #[arg(long)]
    pub(crate) indent_width: Option<usize>,

    #[arg(long, value_enum)]
    pub(crate) line_ending: Option<LineEnding>,

    #[arg(long, value_enum)]
    pub(crate) quotes: Option<QuoteStyle>,

    #[arg(long, value_enum)]
    pub(crate) import_order: Option<ImportOrder>,

    #[arg(long, action = clap::ArgAction::Set)]
    pub(crate) final_newline: Option<bool>,
}

#[derive(Args, Clone)]
pub(crate) struct FormatOutputArguments {
    #[arg(long, conflicts_with_all = ["check", "preview"])]
    pub(crate) source: bool,

    #[arg(long, conflicts_with_all = ["check", "preview"], help = "Authorize protected writes to selected disk sources")]
    pub(crate) write: bool,
}

#[derive(Clone, Copy, ValueEnum)]
pub(crate) enum Indentation {
    Tabs,
    Spaces,
}

#[derive(Clone, Copy, ValueEnum)]
pub(crate) enum LineEnding {
    Lf,
    Crlf,
}

#[derive(Clone, Copy, ValueEnum)]
pub(crate) enum QuoteStyle {
    Preserve,
    PreferDouble,
    PreferSingle,
    ForceDouble,
    ForceSingle,
}

#[derive(Clone, Copy, ValueEnum)]
pub(crate) enum ImportOrder {
    Preserve,
    Alphabetical,
}

#[derive(Args, Clone)]
pub(crate) struct LintArguments {
    #[command(flatten)]
    pub(crate) inputs: InputArguments,

    #[command(flatten)]
    pub(crate) diagnostics: DiagnosticArguments,

    #[arg(long = "rule", action = clap::ArgAction::Append)]
    pub(crate) rules: Vec<String>,

    #[arg(long = "preset", action = clap::ArgAction::Append)]
    pub(crate) presets: Vec<String>,

    #[arg(long, conflicts_with_all = ["fix", "fix_stdin"])]
    pub(crate) fix_preview: bool,

    #[arg(
        long,
        conflicts_with = "fix_preview",
        help = "Authorize protected writes of validated fixes"
    )]
    pub(crate) fix: bool,

    #[arg(long, conflicts_with = "fix_preview")]
    pub(crate) fix_stdin: bool,

    #[arg(long = "fix-rule", action = clap::ArgAction::Append)]
    pub(crate) fix_rules: Vec<String>,
}

#[derive(Parser)]
pub(crate) enum WorkerKind {
    Configuration,
    Analysis,
    Format,
    ProjectAssets,
}
