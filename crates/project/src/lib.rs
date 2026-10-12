//! Project orchestration for the Luau toolchain.

mod acquisition;
mod assets;
mod cache;
mod capture;
mod change;
mod checking;
mod configuration;
mod documentation;
mod editing;
mod environments;
mod execution;
mod extract;
mod formatting;
mod graph;
mod inputs;
mod linting;
mod preparation;
mod project;
mod queries;
mod report;
mod request;
mod resolution;
mod roblox;
mod selection;
mod settings;
mod sourcemap;
mod structure;
mod symbols;
mod tokens;
mod watch;

pub use acquisition::run_asset_worker;

pub use assets::{
    AssetRequest, AssetSnapshot, documentation_cache_directory, roblox_cache_directory,
};

pub use change::{ChangeBatch, ProjectChange, SourceChange, SourceEdit, WorkerAffinity};

pub use configuration::{
    BindingStyle, CheckOverrides, CheckingMode, CheckingPatch, CompletionContent,
    ConfigurationLayer, ConfigurationState, DeclarationSettings, DiagnosticPolicy, DiagnosticScope,
    EditingPatch, EditingSettings, EffectiveSettings, EnvironmentPatch, EnvironmentSettings,
    FormatOverrides, FormattingPatch, HintPatch, HintSettings, LanguagePatch, LintOverrides,
    LintPatch, ObviousHints, Operation, OperationOverrides, ProjectSettings, ProjectSettingsPatch,
    RequirePatch, RequireStyle, ResolutionPatch, ResolutionSettings, Security, SelectionOverrides,
    SelectionSettings,
};

pub use documentation::{DocumentationEntry, DocumentationParameter};

pub use editing::{
    ApplyRequest, EditGuarantee, EditRequest, ExtractionRequest, FileChange, FileRequest,
    FixRequest, ImportRequest, LinkedRequest, RenameRequest, SaveRequest, ValidatedEdit,
    WriteAuthorization, WriteOutcome,
};

pub use execution::Task;
pub use graph::DependencyEdge;

pub use inputs::{
    ContextStore, InputIdentity, InputState, InputVersion, ProjectSnapshot, SourceOrigin,
    SourceRecord, SourceStore,
};

pub use project::Project;

pub use queries::{
    AssistanceOutput, ColorRepresentation, CompletionQuery, DiagnosticQuery, HierarchyCall,
    HierarchyOutput, HierarchyQuery, HierarchyQueryKind, LinkQuery, PositionQuery,
    PositionQueryKind, PresentationKind, PresentationOutput, PresentationQuery, ProjectCandidate,
    ProjectCompletions, ProjectHint, ProjectInlineValue, ProjectLens, ProjectSymbol, Query,
    QueryOutput, QueryRequest, ReferenceQuery, SourceColor, SymbolQuery,
};

pub use report::{
    ApplyReport, CheckReport, Counts, EditReport, FormatReport, FormattedSource, ImportExplanation,
    ImportFailure, LintReport, ModuleDiagnostic, ProjectLink, QueryReport, SettingsReport,
};

pub use request::{
    CheckRequest, ExplanationSelection, FixMode, FormatMode, FormatRequest, Input, Invocation,
    LintRequest, SettingsRequest,
};

pub use selection::SelectionDecision;
pub use structure::Fold;
pub use symbols::{Symbol, SymbolKind};
pub use tokens::{Token, TokenKind, TokenModifier};
pub use watch::{WatchCommandReport, WatchIteration, WatchOperation, WatchReport, WatchRequest};
