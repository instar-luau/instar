use std::{collections::BTreeSet, path::PathBuf, sync::Arc};

use instar_format::FormatScope;
use instar_lint::RuleIdentity;
use instar_luau::ModuleIdentity;
use instar_shared::SourceIdentity;

use crate::{
    configuration::{CheckOverrides, FormatOverrides, LintOverrides, OperationOverrides},
    inputs::ProjectSnapshot,
};

/// A command input or an already captured source or module selection.
#[derive(Clone, Debug)]
pub enum Input {
    /// A source file or directory to discover.
    Path(PathBuf),

    /// Files matching a glob under an explicit discovery base.
    Pattern {
        /// Directory from which the pattern is anchored and discovery starts.
        base: PathBuf,
        /// Glob pattern selecting discovered paths.
        pattern: String,
    },

    /// Standard-input text with an optional filesystem context.
    Stdin {
        /// Exact standard-input contents.
        text: Arc<str>,
        /// Source path supplying configuration and resolution context.
        context: Option<PathBuf>,
    },

    /// An existing captured source identity.
    Source(SourceIdentity),

    /// An existing captured source in a particular module context.
    Module(ModuleIdentity),
}

/// Explanation categories requested by an invocation.
#[derive(Clone, Debug, Default)]
pub struct ExplanationSelection {
    /// Request input selection explanations.
    pub selection: bool,

    /// Request effective settings provenance.
    pub settings: bool,

    /// Request import and dependency resolution explanations.
    pub dependencies: bool,
}

/// Discovery and configuration context shared by command operations.
#[derive(Clone, Debug, Default)]
pub struct Invocation {
    /// Explicit inputs; an empty list discovers from the explicit or workspace roots.
    pub inputs: Vec<Input>,

    /// Explicit project root used for discovery and configuration context.
    pub project_root: Option<PathBuf>,

    /// Explicit configuration file applied to the invocation.
    pub configuration: Option<PathBuf>,

    /// Explanation categories requested for reporting.
    pub explanations: ExplanationSelection,
}

/// Dependency-aware checking with operation-specific overrides.
#[derive(Clone, Debug, Default)]
pub struct CheckRequest {
    /// Inputs and configuration context.
    pub invocation: Invocation,

    /// Overrides applied after discovered configuration.
    pub overrides: CheckOverrides,
}

/// Formatting scope, settings overrides, and requested output behavior.
#[derive(Clone, Debug)]
pub struct FormatRequest {
    /// Inputs and configuration context.
    pub invocation: Invocation,

    /// Whole-source or range-based formatting scope.
    pub scope: FormatScope,

    /// Formatting defaults and overrides around discovered configuration.
    pub overrides: FormatOverrides,

    /// Requested formatting output or write behavior.
    pub mode: FormatMode,

    /// Historical snapshot to format from; absent uses live project state.
    pub snapshot: Option<ProjectSnapshot>,
}

/// Linting and optional fix processing for selected inputs.
#[derive(Clone, Debug, Default)]
pub struct LintRequest {
    /// Inputs and configuration context.
    pub invocation: Invocation,

    /// Overrides applied after discovered configuration.
    pub overrides: LintOverrides,

    /// Requested fix output or application behavior.
    pub fixes: FixMode,

    /// Rules eligible for fixing; absent means all enabled fix-producing rules.
    pub fix_rules: Option<BTreeSet<RuleIdentity>>,
}

/// Requested handling of formatter results.
#[derive(Clone, Copy, Debug, Default)]
pub enum FormatMode {
    /// Report formatting differences as a failed check.
    #[default]
    Check,

    /// Produce previews without applying edits.
    Preview,

    /// Request formatted source output.
    Source,

    /// Request protected disk writes, with standard input emitted as source.
    Write,
}

/// Requested handling of lint fixes.
#[derive(Clone, Copy, Debug, Default)]
pub enum FixMode {
    /// Report findings without preparing fixes.
    #[default]
    None,

    /// Validate fixes and produce previews without applying them.
    Preview,

    /// Request protected disk writes, with standard input emitted as source.
    Write,

    /// Emit fixed standard-input text without writing disk sources.
    Stdin,
}

/// Effective settings capture for one operation.
#[derive(Clone, Debug)]
pub struct SettingsRequest {
    /// Inputs and configuration context.
    pub invocation: Invocation,

    /// Operation and overrides whose effective settings are requested.
    pub overrides: OperationOverrides,

    /// Historical snapshot to capture from; absent uses live project state.
    pub snapshot: Option<ProjectSnapshot>,
}
