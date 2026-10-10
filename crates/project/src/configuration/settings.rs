use std::{
    collections::{BTreeMap, BTreeSet},
    io,
    num::NonZeroUsize,
    path::{Path, PathBuf},
    sync::Arc,
};

use instar_format::{
    FormatSettings,
    settings::{BlankLines, Group, IndentStyle, LineEnding, Order, QuoteStyle, Requires},
};

use instar_lint::{
    LintSettings, Preset, RuleConfiguration, RuleIdentity, RuleProducer, RuleSettings,
};

use instar_luau::{
    LanguageFeatures,
    flags::{self, FlagValue, LanguageProfile},
};

use instar_shared::{DiagnosticCategory, Options, Severity};
use instar_syntax::SyntaxSettings;
use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::{Deserialize, Serialize};

use super::layers::TYPE_FUNCTION_MEMORY;
use crate::{inputs::normalize, selection::Matcher};

/// Operation whose selection and overrides are being resolved.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Operation {
    /// Type checking.
    #[default]
    Check,

    /// Source formatting.
    Format,

    /// Rule-based linting.
    Lint,
}

/// Project checking mode for sources without an overriding directive.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum CheckingMode {
    /// Strict type inference.
    Strict,

    /// Nonstrict type inference.
    #[default]
    Nonstrict,

    /// Disable ordinary type findings.
    Nocheck,
}

/// Explicit operation target-selection controls.
#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct SelectionOverrides {
    /// Replacement inclusion patterns.
    pub include: Option<Vec<String>>,

    /// Replacement exclusion patterns.
    pub exclude: Option<Vec<String>>,

    /// Explicitly bypass configured target exclusions.
    pub override_exclusions: bool,
}

impl SelectionOverrides {
    /// Validates all inclusion and exclusion patterns.
    ///
    /// # Errors
    /// Rejects malformed glob patterns.
    pub fn validate(&self) -> io::Result<()> {
        Matcher::new(self.include.as_deref())?;
        Matcher::new(self.exclude.as_deref())?;

        Ok(())
    }
}

/// Common and responsibility-specific target-selection policy.
#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct SelectionSettings {
    /// Common inclusion patterns.
    pub include: Option<Vec<String>>,

    /// Common exclusion patterns.
    pub exclude: Option<Vec<String>>,

    /// Explicitly bypass configured exclusions.
    pub override_exclusions: bool,

    /// Checking-specific selection.
    pub checking: SelectionOverrides,

    /// Formatting-specific selection.
    pub formatting: SelectionOverrides,

    /// Linting-specific selection.
    pub linting: SelectionOverrides,

    /// Workspace index selection.
    pub index: SelectionOverrides,

    /// Import completion selection.
    pub imports: SelectionOverrides,
}

impl SelectionSettings {
    /// Validates every responsibility's selection patterns.
    ///
    /// # Errors
    /// Rejects malformed glob patterns.
    pub fn validate(&self) -> io::Result<()> {
        self.common().validate()?;

        for selection in [
            &self.checking,
            &self.formatting,
            &self.linting,
            &self.index,
            &self.imports,
        ] {
            selection.validate()?;
        }

        Ok(())
    }

    pub(crate) fn common(&self) -> SelectionOverrides {
        SelectionOverrides {
            include: self.include.clone(),
            exclude: self.exclude.clone(),
            override_exclusions: self.override_exclusions,
        }
    }

    pub(crate) fn operation(&self, operation: Operation) -> SelectionOverrides {
        let selected = match operation {
            Operation::Check => &self.checking,
            Operation::Format => &self.formatting,
            Operation::Lint => &self.linting,
        };

        SelectionOverrides {
            include: selected.include.clone().or_else(|| self.include.clone()),
            exclude: selected.exclude.clone().or_else(|| self.exclude.clone()),
            override_exclusions: selected.override_exclusions || self.override_exclusions,
        }
    }

    fn merge(&self, parent: &mut Self) {
        merge_selection(
            &self.common(),
            &mut parent.include,
            &mut parent.exclude,
            &mut parent.override_exclusions,
        );

        for (child, parent) in [
            (&self.checking, &mut parent.checking),
            (&self.formatting, &mut parent.formatting),
            (&self.linting, &mut parent.linting),
            (&self.index, &mut parent.index),
            (&self.imports, &mut parent.imports),
        ] {
            merge_selection(
                child,
                &mut parent.include,
                &mut parent.exclude,
                &mut parent.override_exclusions,
            );
        }
    }
}

/// Partial checking mode and reporting controls.
#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct CheckingPatch {
    /// Checking mode override.
    pub mode: Option<CheckingMode>,

    /// Ordinary type-diagnostic reporting override.
    pub type_errors: Option<bool>,
}

/// Partial validated Luau language profile.
#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct LanguagePatch {
    /// Registry-validated boolean or signed integer flags.
    #[schemars(schema_with = "flag_schema")]
    pub flags: BTreeMap<String, FlagValue>,

    /// Declaration-syntax override.
    pub declaration_syntax: Option<bool>,
}

/// Partial module resolution policy anchored to its defining base.
#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct ResolutionPatch {
    /// Named alias replacements or explicit removals.
    pub aliases: BTreeMap<String, Option<PathBuf>>,

    /// Replacement sourcemap list; an empty list disables discovery.
    pub sourcemaps: Option<Vec<PathBuf>>,
}

impl ResolutionPatch {
    fn validate_aliases(&self) -> io::Result<()> {
        for (name, target) in &self.aliases {
            if name
                .strip_prefix('@')
                .unwrap_or(name)
                .eq_ignore_ascii_case("self")
            {
                return Err(invalid("alias @self is reserved"));
            }

            if name.is_empty()
                || matches!(name.as_str(), "." | "..")
                || !name.bytes().all(|character| {
                    character.is_ascii_alphanumeric() || b".-_".contains(&character)
                })
            {
                return Err(invalid(&format!("invalid module alias name {name}")));
            }

            if self
                .aliases
                .keys()
                .any(|other| other != name && other.eq_ignore_ascii_case(name))
            {
                return Err(invalid(&format!(
                    "duplicate case-insensitive module alias {name}"
                )));
            }

            if let Some(target) = target {
                validate_path(target)?;
            }
        }

        Ok(())
    }
}

/// One ordered declaration and documentation group.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DeclarationSettings {
    /// Unique custom namespace, excluding builtins.
    pub namespace: String,

    /// Ordered declaration source paths.
    #[serde(default)]
    pub definitions: Vec<PathBuf>,

    /// Ordered documentation paths.
    #[serde(default)]
    pub documentation: Vec<PathBuf>,
}

/// Partial environment policy anchored to its defining base.
#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct EnvironmentPatch {
    /// Replacement ordered declaration groups.
    pub groups: Option<Vec<DeclarationSettings>>,

    /// Replacement declared-global names.
    pub globals: Option<Vec<String>>,

    /// Explicit Roblox environment selection.
    pub roblox: Option<bool>,

    /// Roblox security profile override.
    pub security: Option<Security>,

    /// Verified asset cache directory override.
    pub asset_directory: Option<PathBuf>,

    /// Custom capability input path.
    pub capabilities: Option<PathBuf>,
}

/// Partial formatting policy retaining omitted inherited fields.
#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct FormattingPatch {
    /// Target line width override.
    pub width: Option<NonZeroUsize>,

    /// Indentation style override.
    pub indent_style: Option<IndentStyle>,

    /// Indentation width override.
    pub indent_width: Option<NonZeroUsize>,

    /// Output line ending override.
    pub line_ending: Option<LineEnding>,

    /// String quote policy override.
    pub quote_style: Option<QuoteStyle>,

    /// Partial require ordering and grouping policy.
    pub requires: Option<RequirePatch>,

    /// Final newline override.
    pub final_newline: Option<bool>,
}

impl FormattingPatch {
    pub(crate) fn apply(&self, settings: &mut FormatSettings) {
        if let Some(value) = self.width {
            settings.width = value;
        }

        if let Some(value) = self.indent_style {
            settings.indent_style = value;
        }

        if let Some(value) = self.indent_width {
            settings.indent_width = value;
        }

        if let Some(value) = self.line_ending {
            settings.line_ending = value;
        }

        if let Some(value) = self.quote_style {
            settings.quote_style = value;
        }

        if let Some(value) = &self.requires {
            value.apply(&mut settings.requires);
        }

        if let Some(value) = self.final_newline {
            settings.final_newline = value;
        }
    }
}

/// Partial require ordering and grouping settings.
#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct RequirePatch {
    /// Ordering override.
    pub order: Option<Order>,

    /// Replacement ordered group list.
    pub groups: Option<Vec<Group>>,

    /// Blank line override.
    pub blank_lines: Option<BlankLines>,
}

impl RequirePatch {
    fn apply(&self, settings: &mut Requires) {
        if let Some(value) = self.order {
            settings.order = value;
        }

        if let Some(value) = &self.groups {
            settings.groups.clone_from(value);
        }

        if let Some(value) = self.blank_lines {
            settings.blank_lines = value;
        }
    }
}

/// Partial rule selection and validated rule-specific settings.
#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct LintPatch {
    /// Named rule overrides; named parameter maps merge by key.
    pub rules: Option<BTreeMap<RuleIdentity, RuleConfiguration>>,

    /// Ordered preset selection.
    pub presets: Option<Vec<Preset>>,

    /// Sole selected rule set.
    pub selected: Option<BTreeSet<RuleIdentity>>,

    /// Report upstream warnings as errors when enabled.
    pub upstream_errors: Option<bool>,

    /// Upstream warning enablement overrides.
    pub warnings: Option<BTreeMap<RuleIdentity, bool>>,
}

impl LintPatch {
    pub(crate) fn apply(&self, settings: &mut LintSettings) -> io::Result<()> {
        if let Some(errors) = self.upstream_errors {
            for definition in instar_lint::rules() {
                if matches!(definition.producer, RuleProducer::Upstream(_))
                    && let Some(configuration) = settings.rules.get_mut(&definition.identity)
                {
                    configuration.severity = if errors {
                        Severity::Error
                    } else {
                        Severity::Warning
                    };
                }
            }
        }

        if let Some(warnings) = &self.warnings {
            for (identity, enabled) in warnings {
                let definition = instar_lint::rules()
                    .iter()
                    .find(|definition| definition.identity == *identity)
                    .ok_or_else(|| invalid("unknown upstream warning"))?;

                if !matches!(definition.producer, RuleProducer::Upstream(_)) {
                    return Err(invalid("Luau warning settings cannot select Instar rules"));
                }

                let configuration = settings
                    .rules
                    .get_mut(identity)
                    .ok_or_else(|| invalid("upstream warning has no configuration"))?;

                configuration.enabled = *enabled;
            }
        }

        if let Some(presets) = &self.presets {
            for preset in presets {
                settings.apply_preset(*preset);
            }
        }

        if let Some(rules) = &self.rules {
            for (identity, configuration) in rules {
                let mut configuration = configuration.clone();

                if configuration.settings.identity() != *identity {
                    return Err(invalid("lint settings identity does not match rule"));
                }

                if let Some(previous) = settings.rules.get(identity) {
                    match (&previous.settings, &mut configuration.settings) {
                        (RuleSettings::Deprecated(parent), RuleSettings::Deprecated(child)) => {
                            let mut names = parent.paths.clone();
                            names.extend(child.paths.clone());
                            child.paths = names;
                        }

                        (
                            RuleSettings::RestrictedGlobals(parent),
                            RuleSettings::RestrictedGlobals(child),
                        ) => {
                            let mut names = parent.names.clone();
                            names.extend(child.names.clone());
                            child.names = names;
                        }

                        (
                            RuleSettings::RestrictedModulePaths(parent),
                            RuleSettings::RestrictedModulePaths(child),
                        ) => {
                            let mut names = parent.paths.clone();
                            names.extend(child.paths.clone());
                            child.paths = names;
                        }

                        _ => {}
                    }
                }

                settings.rules.insert(*identity, configuration);
            }
        }

        if let Some(selected) = &self.selected {
            for (identity, configuration) in &mut settings.rules {
                configuration.enabled = selected.contains(identity);
            }
        }

        settings.validate()
    }
}

/// Failure threshold and display filtering, independent of analysis coverage.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct DiagnosticPolicy {
    /// Minimum severity causing failure.
    pub threshold: Severity,

    /// Treat warnings as failures.
    pub warnings_as_errors: bool,

    /// Optional displayed diagnostic code set.
    pub codes: Option<BTreeSet<String>>,

    /// Optional displayed diagnostic category set.
    pub categories: Option<BTreeSet<DiagnosticCategory>>,

    /// Optional displayed severity set.
    pub severities: Option<BTreeSet<Severity>>,
}

impl Default for DiagnosticPolicy {
    fn default() -> Self {
        Self {
            threshold: Severity::Error,
            warnings_as_errors: false,
            codes: None,
            categories: None,
            severities: None,
        }
    }
}

/// Sole operation override record for checking.
#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct CheckOverrides {
    /// Checking target-selection overrides.
    pub selection: SelectionOverrides,

    /// Checking policy overrides.
    pub checking: CheckingPatch,

    /// Luau language overrides.
    pub language: LanguagePatch,

    /// Resolution overrides.
    pub resolution: ResolutionPatch,

    /// Environment overrides.
    pub environment: EnvironmentPatch,

    /// Failure and display policy.
    pub diagnostics: DiagnosticPolicy,
}

/// Sole operation override record for formatting.
#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct FormatOverrides {
    /// Formatting target-selection overrides.
    pub selection: SelectionOverrides,

    /// Formatting policy overrides.
    pub formatting: FormattingPatch,

    /// Luau language overrides.
    pub language: LanguagePatch,
}

/// Sole operation override record for linting.
#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct LintOverrides {
    /// Linting target-selection overrides.
    pub selection: SelectionOverrides,

    /// Rule policy overrides.
    pub linting: LintPatch,

    /// Luau language overrides.
    pub language: LanguagePatch,

    /// Resolution overrides.
    pub resolution: ResolutionPatch,

    /// Environment overrides.
    pub environment: EnvironmentPatch,

    /// Failure and display policy.
    pub diagnostics: DiagnosticPolicy,
}

/// Typed controls shared by settings display and the operation itself.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
pub enum OperationOverrides {
    /// Checking controls.
    Check(CheckOverrides),

    /// Formatting controls.
    Format(FormatOverrides),

    /// Linting controls.
    Lint(LintOverrides),
}

impl OperationOverrides {
    /// Returns the selected operation.
    #[must_use]
    pub fn operation(&self) -> Operation {
        match self {
            Self::Check(_) => Operation::Check,
            Self::Format(_) => Operation::Format,
            Self::Lint(_) => Operation::Lint,
        }
    }

    /// Returns the selected operation's target controls.
    #[must_use]
    pub fn selection(&self) -> &SelectionOverrides {
        match self {
            Self::Check(overrides) => &overrides.selection,
            Self::Format(overrides) => &overrides.selection,
            Self::Lint(overrides) => &overrides.selection,
        }
    }

    pub(crate) fn diagnostics(&self) -> Option<&DiagnosticPolicy> {
        match self {
            Self::Check(overrides) => Some(&overrides.diagnostics),
            Self::Format(_) => None,
            Self::Lint(overrides) => Some(&overrides.diagnostics),
        }
    }

    pub(crate) fn patch(&self) -> ProjectSettingsPatch {
        let mut selection = SelectionSettings::default();

        match self {
            Self::Check(overrides) => {
                selection.checking = overrides.selection.clone();

                ProjectSettingsPatch {
                    selection,
                    checking: overrides.checking.clone(),
                    language: overrides.language.clone(),
                    resolution: overrides.resolution.clone(),
                    environment: overrides.environment.clone(),
                    ..ProjectSettingsPatch::default()
                }
            }

            Self::Format(overrides) => {
                selection.formatting = overrides.selection.clone();

                ProjectSettingsPatch {
                    selection,
                    formatting: overrides.formatting.clone(),
                    language: overrides.language.clone(),
                    ..ProjectSettingsPatch::default()
                }
            }

            Self::Lint(overrides) => {
                selection.linting = overrides.selection.clone();

                ProjectSettingsPatch {
                    selection,
                    linting: overrides.linting.clone(),
                    language: overrides.language.clone(),
                    resolution: overrides.resolution.clone(),
                    environment: overrides.environment.clone(),
                    ..ProjectSettingsPatch::default()
                }
            }
        }
    }
}

/// Content included in editor completion responses, encoded as the snippet boolean.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(from = "bool", into = "bool")]
pub enum CompletionContent {
    /// Plain completion text.
    Plain,

    /// Completion text including snippets.
    #[default]
    Snippets,
}

impl From<bool> for CompletionContent {
    fn from(enabled: bool) -> Self {
        if enabled { Self::Snippets } else { Self::Plain }
    }
}

impl From<CompletionContent> for bool {
    fn from(content: CompletionContent) -> Self {
        matches!(content, CompletionContent::Snippets)
    }
}

/// Visibility of obvious type hints, encoded as the hide-obvious boolean.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(from = "bool", into = "bool")]
pub enum ObviousHints {
    /// Show obvious type hints.
    Visible,

    /// Hide obvious type hints.
    #[default]
    Hidden,
}

impl From<bool> for ObviousHints {
    fn from(hidden: bool) -> Self {
        if hidden { Self::Hidden } else { Self::Visible }
    }
}

impl From<ObviousHints> for bool {
    fn from(hints: ObviousHints) -> Self {
        matches!(hints, ObviousHints::Hidden)
    }
}

/// Effective editor policy after inheritance and overlays.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct EditingSettings {
    /// Format on save.
    pub format_on_save: bool,

    /// Fix on save.
    pub fix_on_save: bool,

    /// Diagnostic source scope.
    pub diagnostic_scope: DiagnosticScope,

    /// Include completion snippets.
    #[schemars(with = "bool")]
    pub completion_snippets: CompletionContent,

    /// Enable import completions.
    pub imports_enabled: bool,

    /// Import require-expression style.
    pub import_require: RequireStyle,

    /// Import binding declaration style.
    pub import_binding: BindingStyle,

    /// Effective hint policy.
    pub hints: HintSettings,
}

impl Default for EditingSettings {
    fn default() -> Self {
        Self {
            format_on_save: false,
            fix_on_save: false,
            diagnostic_scope: DiagnosticScope::OpenFiles,
            completion_snippets: CompletionContent::Snippets,
            imports_enabled: true,
            import_require: RequireStyle::String,
            import_binding: BindingStyle::Const,
            hints: HintSettings::default(),
        }
    }
}

/// Partial editor policy retaining omitted inherited fields.
#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct EditingPatch {
    /// Format-on-save override.
    pub format_on_save: Option<bool>,

    /// Fix-on-save override.
    pub fix_on_save: Option<bool>,

    /// Diagnostic scope override.
    pub diagnostic_scope: Option<DiagnosticScope>,

    /// Completion snippet override.
    pub completion_snippets: Option<bool>,

    /// Import completion override.
    pub imports_enabled: Option<bool>,

    /// Require expression style override.
    pub import_require: Option<RequireStyle>,

    /// Import binding style override.
    pub import_binding: Option<BindingStyle>,

    /// Partial hint policy.
    pub hints: Option<HintPatch>,
}

impl EditingPatch {
    fn apply(&self, settings: &mut EditingSettings) {
        if let Some(value) = self.format_on_save {
            settings.format_on_save = value;
        }

        if let Some(value) = self.fix_on_save {
            settings.fix_on_save = value;
        }

        if let Some(value) = self.diagnostic_scope {
            settings.diagnostic_scope = value;
        }

        if let Some(value) = self.completion_snippets {
            settings.completion_snippets = value.into();
        }

        if let Some(value) = self.imports_enabled {
            settings.imports_enabled = value;
        }

        if let Some(value) = self.import_require {
            settings.import_require = value;
        }

        if let Some(value) = self.import_binding {
            settings.import_binding = value;
        }

        if let Some(value) = &self.hints {
            value.apply(&mut settings.hints);
        }
    }
}

/// Editor diagnostic source scope.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticScope {
    /// Synchronized open sources.
    #[default]
    OpenFiles,

    /// The indexed workspace.
    Workspace,
}

/// Import completion require-expression style.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum RequireStyle {
    /// String module paths.
    #[default]
    String,

    /// Mapped instance expressions.
    Instance,
}

/// Import completion binding style.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum BindingStyle {
    /// Mutable local binding.
    Local,

    /// Immutable binding.
    #[default]
    Const,
}

/// Effective editor hint policy.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct HintSettings {
    /// Show inferred variable types.
    pub variable_types: bool,

    /// Show parameter names.
    pub parameter_names: bool,

    /// Show inferred return types.
    pub return_types: bool,

    /// Hide obvious type hints.
    #[schemars(with = "bool")]
    pub hide_obvious: ObviousHints,
}

impl Default for HintSettings {
    fn default() -> Self {
        Self {
            variable_types: false,
            parameter_names: false,
            return_types: false,
            hide_obvious: ObviousHints::Hidden,
        }
    }
}

/// Partial hint policy retaining omitted inherited fields.
#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct HintPatch {
    /// Variable-type hint override.
    pub variable_types: Option<bool>,

    /// Parameter-name hint override.
    pub parameter_names: Option<bool>,

    /// Return-type hint override.
    pub return_types: Option<bool>,

    /// Obvious-type hint visibility override.
    pub hide_obvious: Option<bool>,
}

impl HintPatch {
    fn apply(&self, settings: &mut HintSettings) {
        if let Some(value) = self.variable_types {
            settings.variable_types = value;
        }

        if let Some(value) = self.parameter_names {
            settings.parameter_names = value;
        }

        if let Some(value) = self.return_types {
            settings.return_types = value;
        }

        if let Some(value) = self.hide_obvious {
            settings.hide_obvious = value.into();
        }
    }
}

/// Roblox capability security profile.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Security {
    /// Standard script capabilities.
    #[default]
    None,

    /// Local-script capabilities.
    Local,

    /// Plugin capabilities.
    Plugin,

    /// Roblox-internal capabilities.
    Roblox,
}

/// Strict partial project configuration with defining-base path semantics.
#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct ProjectSettingsPatch {
    /// Luau language settings.
    pub language: LanguagePatch,

    /// Checking settings.
    pub checking: CheckingPatch,

    /// Formatting settings.
    pub formatting: FormattingPatch,

    /// Rule settings.
    pub linting: LintPatch,

    /// Common and operation-specific selection settings.
    pub selection: SelectionSettings,

    /// Resolution settings.
    pub resolution: ResolutionPatch,

    /// Environment settings.
    pub environment: EnvironmentPatch,

    /// Partial editor settings.
    pub editing: Option<EditingPatch>,
}

impl ProjectSettingsPatch {
    /// Validates the complete patch before accepting a layer or overlay.
    ///
    /// # Errors
    /// Rejects unsupported flags, invalid rules, patterns, names, paths, namespaces, and formatting policy.
    pub fn validate(&self) -> io::Result<()> {
        self.selection.validate()?;
        let mut settings = ProjectSettings::default();
        self.apply(&mut settings, None)?;

        Ok(())
    }

    pub(crate) fn apply(
        &self,
        settings: &mut ProjectSettings,
        scope: Option<(&Path, &Options)>,
    ) -> io::Result<()> {
        settings
            .language
            .flags
            .extend(canonical_flags(&self.language.flags)?);

        settings.language.flags =
            LanguageProfile::new(&settings.language.flags, TYPE_FUNCTION_MEMORY)?.flags;

        if let Some(value) = self.language.declaration_syntax {
            settings.language.declaration_syntax = value;
        }

        if let Some(value) = self.checking.mode {
            settings.checking = value;
        }

        if let Some(value) = self.checking.type_errors {
            settings.type_errors = value;
        }

        self.formatting.apply(&mut settings.formatting);
        self.linting.apply(&mut settings.linting)?;
        self.selection.merge(&mut settings.selection);

        self.resolution.validate_aliases()?;

        settings
            .resolution
            .apply_aliases(&self.resolution.aliases, scope)?;

        if let Some(value) = &self.resolution.sourcemaps {
            settings.resolution.sourcemaps.clone_from(value);
            settings.resolution.sourcemaps_explicit = true;
        }

        if let Some(value) = &self.environment.groups {
            settings.environment.groups.clone_from(value);
        }

        if let Some(value) = &self.environment.globals {
            settings.environment.globals.clone_from(value);
        }

        if let Some(value) = self.environment.roblox {
            settings.environment.roblox = value;
            settings.environment.roblox_explicit = true;
        }

        if let Some(value) = self.environment.security {
            settings.environment.security = value;
        }

        if let Some(value) = &self.environment.asset_directory {
            settings.environment.asset_directory = Some(value.clone());
        }

        if let Some(value) = &self.environment.capabilities {
            settings.environment.capabilities = Some(value.clone());
        }

        if let Some(value) = &self.editing {
            value.apply(&mut settings.editing);
        }

        settings.selection.validate()?;
        settings.formatting.validate()?;
        let mut namespaces = BTreeSet::new();

        for group in &settings.environment.groups {
            validate_namespace(&group.namespace)?;

            if !group.namespace.starts_with('@')
                || group.namespace.len() < 2
                || matches!(group.namespace.as_str(), "@luau" | "@roblox")
                || !namespaces.insert(&group.namespace)
            {
                return Err(invalid(
                    "declaration namespaces must be unique and cannot use @luau or @roblox",
                ));
            }

            for path in group.definitions.iter().chain(&group.documentation) {
                validate_path(path)?;
            }
        }

        for path in settings
            .resolution
            .aliases
            .values()
            .chain(&settings.resolution.sourcemaps)
            .chain(settings.environment.capabilities.iter())
            .chain(settings.environment.asset_directory.iter())
        {
            validate_path(path)?;
        }

        settings
            .linting
            .globals
            .clone_from(&settings.environment.globals);

        settings.linting.roblox = settings.environment.roblox;
        settings.linting.validate()?;

        Ok(())
    }
}

/// Effective module-resolution settings with anchored defining-layer alias bindings.
#[derive(Clone, Debug, Default, Serialize)]
pub struct ResolutionSettings {
    /// Visible alias definitions; references retain their defining layer's scope.
    pub aliases: BTreeMap<String, PathBuf>,

    /// Ordered effective sourcemap paths.
    pub sourcemaps: Vec<PathBuf>,

    /// Whether a layer explicitly selected the sourcemap list.
    pub sourcemaps_explicit: bool,

    #[serde(skip)]
    targets: BTreeMap<String, AliasTarget>,
}

impl ResolutionSettings {
    pub(crate) fn alias(&self, name: &str, current: &Path) -> io::Result<PathBuf> {
        if name.is_empty()
            || matches!(name, "." | "..")
            || !name
                .bytes()
                .all(|character| character.is_ascii_alphanumeric() || b".-_".contains(&character))
        {
            return Err(invalid(&format!("invalid alias name @{name}")));
        }

        let name = name.to_ascii_lowercase();

        if name == "self" {
            return Ok(current.to_path_buf());
        }

        match self.targets.get(&name) {
            Some(AliasTarget::Path(path)) => Ok(path.clone()),
            Some(AliasTarget::SelfRelative(suffix)) => Ok(current.join(suffix)),
            Some(AliasTarget::Failure(reason)) => Err(invalid(reason.as_ref())),
            None => Err(invalid(&format!("unknown alias @{name}"))),
        }
    }

    fn apply_aliases(
        &mut self,
        aliases: &BTreeMap<String, Option<PathBuf>>,
        scope: Option<(&Path, &Options)>,
    ) -> io::Result<()> {
        let definitions = aliases
            .iter()
            .map(|(name, target)| (name.to_ascii_lowercase(), target.clone()))
            .collect::<BTreeMap<_, _>>();

        if let Some((base, options)) = scope {
            let mut targets = alias_targets(&definitions, &self.targets, base, options)?;

            for (name, definition) in &definitions {
                options.check()?;

                if definition.is_some() {
                    let target = targets
                        .remove(name)
                        .ok_or_else(|| invalid(&format!("alias binding is missing for @{name}")))?;

                    self.targets.insert(name.clone(), target);
                } else {
                    self.targets.remove(name);
                }
            }
        }

        for (name, target) in definitions {
            if let Some(target) = target {
                let displayed = if scope.is_some() && !target.to_string_lossy().starts_with('@') {
                    normalize(&target)
                } else {
                    target
                };

                self.aliases.insert(name, displayed);
            } else {
                self.aliases.remove(&name);
            }
        }

        Ok(())
    }
}

#[derive(Clone, Debug)]
enum AliasTarget {
    Path(PathBuf),
    SelfRelative(PathBuf),
    Failure(Arc<str>),
}

/// Effective declaration and capability environment.
#[derive(Clone, Debug, Default, Serialize)]
pub struct EnvironmentSettings {
    /// Ordered custom declaration groups.
    pub groups: Vec<DeclarationSettings>,

    /// Declared global names.
    pub globals: Vec<String>,

    /// Whether Roblox support is selected.
    pub roblox: bool,

    /// Whether a layer explicitly selected Roblox support.
    pub roblox_explicit: bool,

    /// Roblox capability security profile.
    pub security: Security,

    /// Verified asset cache directory.
    pub asset_directory: Option<PathBuf>,

    /// Custom capability input path.
    pub capabilities: Option<PathBuf>,
}

/// Validated effective settings consumed by every operation.
#[derive(Clone, Debug, Serialize)]
pub struct ProjectSettings {
    /// Effective immutable language profile, including registry defaults.
    pub language: LanguageFeatures,

    /// Project checking mode.
    pub checking: CheckingMode,

    /// Whether ordinary type findings are reported.
    pub type_errors: bool,

    /// Effective formatting policy.
    pub formatting: FormatSettings,

    /// Effective rule policies.
    pub linting: LintSettings,

    /// Effective target-selection policy.
    pub selection: SelectionSettings,

    /// Effective module-resolution policy.
    pub resolution: ResolutionSettings,

    /// Effective declaration and capability environment.
    pub environment: EnvironmentSettings,

    /// Effective editor policy.
    pub editing: EditingSettings,

    #[serde(skip)]
    pub(crate) syntax: SyntaxSettings,
}

impl Default for ProjectSettings {
    fn default() -> Self {
        Self {
            language: LanguageFeatures {
                flags: BTreeMap::new(),
                declaration_syntax: true,
            },
            checking: CheckingMode::default(),
            type_errors: true,
            formatting: FormatSettings::default(),
            linting: LintSettings::default(),
            selection: SelectionSettings::default(),
            resolution: ResolutionSettings::default(),
            environment: EnvironmentSettings::default(),
            editing: EditingSettings::default(),
            syntax: SyntaxSettings::default(),
        }
    }
}

fn alias_targets(
    definitions: &BTreeMap<String, Option<PathBuf>>,
    inherited: &BTreeMap<String, AliasTarget>,
    base: &Path,
    options: &Options,
) -> io::Result<BTreeMap<String, AliasTarget>> {
    let mut targets = BTreeMap::<String, AliasTarget>::new();

    for start in definitions.keys() {
        options.check()?;

        if targets.contains_key(start) {
            continue;
        }

        let mut name = start.clone();
        let mut chain = Vec::<(String, PathBuf)>::new();
        let mut positions = BTreeMap::<String, usize>::new();

        let mut target = loop {
            options.check()?;

            if name.is_empty()
                || matches!(name.as_str(), "." | "..")
                || !name.bytes().all(|character| {
                    character.is_ascii_alphanumeric() || b".-_".contains(&character)
                })
            {
                break AliasTarget::Failure(format!("invalid alias name @{name}").into());
            }

            if name == "self" {
                break AliasTarget::SelfRelative(PathBuf::new());
            }

            if let Some(target) = targets.get(&name) {
                break target.clone();
            }

            if let Some(position) = positions.get(&name) {
                let cycle = chain[*position..]
                    .iter()
                    .map(|(name, _)| format!("{}:@{name}", base.display()))
                    .chain([format!("{}:@{name}", base.display())])
                    .collect::<Vec<_>>()
                    .join(" -> ");

                break AliasTarget::Failure(format!("alias cycle: {cycle}").into());
            }

            match definitions.get(&name) {
                Some(Some(path)) => {
                    let text = path.to_string_lossy().replace('\\', "/");

                    if let Some(reference) = text.strip_prefix('@') {
                        let (next, suffix) = reference.split_once('/').unwrap_or((reference, ""));
                        positions.insert(name.clone(), chain.len());
                        chain.push((name, PathBuf::from(suffix)));
                        name = next.to_ascii_lowercase();
                    } else {
                        let target = AliasTarget::Path(base.join(path));
                        targets.insert(name, target.clone());
                        break target;
                    }
                }

                Some(None) => break AliasTarget::Failure(format!("unknown alias @{name}").into()),

                None => {
                    break inherited.get(&name).cloned().unwrap_or_else(|| {
                        AliasTarget::Failure(format!("unknown alias @{name}").into())
                    });
                }
            }
        };

        for (name, suffix) in chain.into_iter().rev() {
            options.check()?;

            if !suffix.as_os_str().is_empty() {
                target = match target {
                    AliasTarget::Path(path) => AliasTarget::Path(path.join(suffix)),

                    AliasTarget::SelfRelative(mut path) => {
                        path.push(suffix);

                        AliasTarget::SelfRelative(path)
                    }

                    AliasTarget::Failure(reason) => AliasTarget::Failure(reason),
                };
            }

            targets.insert(name, target.clone());
        }
    }

    Ok(targets)
}

pub(crate) fn canonical_flags(
    selected: &BTreeMap<String, FlagValue>,
) -> io::Result<BTreeMap<String, FlagValue>> {
    if selected.is_empty() {
        return Ok(BTreeMap::new());
    }

    let normalized = flags::normalize(selected)?;

    selected
        .keys()
        .map(|name| {
            let canonical = normalized
                .keys()
                .find(|canonical| *canonical == name)
                .or_else(|| {
                    normalized
                        .keys()
                        .filter(|canonical| {
                            name.strip_prefix(canonical.as_str())
                                .is_some_and(|version| {
                                    !version.is_empty()
                                        && version
                                            .bytes()
                                            .all(|character| character.is_ascii_digit())
                                })
                        })
                        .max_by_key(|canonical| canonical.len())
                })
                .ok_or_else(|| invalid("validated flag has no canonical registry name"))?;

            Ok((canonical.clone(), normalized[canonical]))
        })
        .collect()
}

pub(crate) fn validate_namespace(namespace: &str) -> io::Result<()> {
    if namespace.strip_prefix('@').is_some_and(|name| {
        !name.is_empty()
            && name
                .bytes()
                .all(|character| character.is_ascii_alphanumeric() || b"_-".contains(&character))
    }) {
        Ok(())
    } else {
        Err(invalid("invalid documentation or declaration namespace"))
    }
}

fn flag_schema(_: &mut SchemaGenerator) -> Schema {
    json_schema!({ "type": "object", "additionalProperties": { "oneOf": [{ "type": "boolean" }, { "type": "integer", "minimum": -2_147_483_648_i64, "maximum": 2_147_483_647_i64 }] } })
}

fn merge_selection(
    child: &SelectionOverrides,
    include: &mut Option<Vec<String>>,
    exclude: &mut Option<Vec<String>>,
    override_exclusions: &mut bool,
) {
    if let Some(value) = &child.include {
        *include = Some(value.clone());
    }

    if let Some(value) = &child.exclude {
        *exclude = Some(value.clone());
    }

    *override_exclusions |= child.override_exclusions;
}

fn validate_path(path: &Path) -> io::Result<()> {
    let text = path
        .to_str()
        .ok_or_else(|| invalid("configuration paths must be UTF-8"))?;

    if text.is_empty() || text.contains('\0') || text.contains("://") {
        return Err(invalid(
            "configuration paths must name nonempty local files",
        ));
    }

    Ok(())
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}
