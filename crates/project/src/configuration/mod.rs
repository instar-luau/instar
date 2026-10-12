mod layers;
mod settings;

pub(crate) use layers::worker_options;
pub use layers::{ConfigurationLayer, ConfigurationState, EffectiveSettings};
pub(crate) use settings::validate_namespace;

pub use settings::{
    BindingStyle, CheckOverrides, CheckingMode, CheckingPatch, CompletionContent,
    DeclarationSettings, DiagnosticPolicy, DiagnosticScope, EditingPatch, EditingSettings,
    EnvironmentPatch, EnvironmentSettings, FormatOverrides, FormattingPatch, HintPatch,
    HintSettings, LanguagePatch, LintOverrides, LintPatch, ObviousHints, Operation,
    OperationOverrides, ProjectSettings, ProjectSettingsPatch, RequirePatch, RequireStyle,
    ResolutionPatch, ResolutionSettings, Security, SelectionOverrides, SelectionSettings,
};
