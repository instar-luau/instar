mod layers;
mod settings;

pub use layers::{ConfigurationLayer, ConfigurationState, EffectiveSettings};
pub(crate) use layers::{TYPE_FUNCTION_MEMORY, worker_options};
pub(crate) use settings::validate_namespace;

pub use settings::{
    BindingStyle, CheckOverrides, CheckingMode, CheckingPatch, CompletionContent,
    DeclarationSettings, DiagnosticPolicy, DiagnosticScope, EditingPatch, EditingSettings,
    EnvironmentPatch, EnvironmentSettings, FormatOverrides, FormattingPatch, HintPatch,
    HintSettings, LanguagePatch, LintOverrides, LintPatch, ObviousHints, Operation,
    OperationOverrides, ProjectSettings, ProjectSettingsPatch, RequirePatch, RequireStyle,
    ResolutionPatch, ResolutionSettings, Security, SelectionOverrides, SelectionSettings,
};
