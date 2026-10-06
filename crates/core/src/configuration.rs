//! The Instar configuration contract and its JSON Schema.

use std::{
    collections::{BTreeMap, BTreeSet},
    io,
    path::PathBuf,
};

use instar_analysis::error::invalid;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// An Instar configuration, independent of native Luau settings.
#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct Configuration {
    /// Global include patterns; an empty list selects every source.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Vec<String>")]
    pub include: Option<Vec<String>>,

    /// Global exclusions; exclusions take precedence over includes.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Vec<String>")]
    pub exclude: Option<Vec<String>>,

    /// Checker file selection.
    pub check: Selection,

    /// Formatter style and file selection.
    pub format: instar_format::Configuration,

    /// Instar lint rules and linter file selection.
    pub lint: instar_lint::Configuration,

    /// Editor preferences, independent of protocol capability advertisement.
    pub editor: Editor,

    /// Ordered, namespaced declaration and documentation groups.
    pub environment: Vec<Environment>,

    /// Process-wide native Luau flag overrides.
    pub luau: Luau,

    /// Roblox integration.
    pub roblox: Roblox,
}

impl Configuration {
    /// Parses and validates a single configuration without resolving paths or inheritance.
    ///
    /// # Errors
    /// Returns invalid TOML, unknown settings, or invalid option errors.
    pub fn parse(source: &str) -> io::Result<Self> {
        let configuration: Self = toml::from_str(source).map_err(invalid)?;
        configuration.validate()?;

        Ok(configuration)
    }

    /// Checks patterns, rule options, and local file references.
    ///
    /// # Errors
    /// Returns an error for invalid configuration values.
    pub fn validate(&self) -> io::Result<()> {
        self.format.validate()?;
        self.lint.validate()?;
        self.luau.native()?;
        let mut namespaces = BTreeSet::new();

        for environment in &self.environment {
            instar_bridge::frontend::validate_namespace(&environment.namespace)?;

            if matches!(environment.namespace.as_str(), "@luau" | "@roblox")
                || !namespaces.insert(&environment.namespace)
            {
                return Err(invalid(
                    "environment namespaces must be unique and cannot use @luau or @roblox",
                ));
            }
        }

        for patterns in [
            &self.include,
            &self.exclude,
            &self.check.include,
            &self.check.exclude,
            &self.editor.index.include,
            &self.editor.index.exclude,
            &self.editor.imports.include,
            &self.editor.imports.exclude,
        ]
        .into_iter()
        .flatten()
        {
            instar_analysis::selection::validate(Some(patterns), "selection")?;
        }

        for path in self
            .environment
            .iter()
            .flat_map(|environment| {
                environment
                    .definitions
                    .iter()
                    .chain(&environment.documentation)
            })
            .chain(self.roblox.sourcemaps.iter().flatten())
        {
            let text = path
                .to_str()
                .ok_or_else(|| invalid("configuration paths must be UTF-8"))?;

            if text.is_empty() || text.contains('\0') || text.contains("://") {
                return Err(invalid(
                    "configuration paths must name nonempty local files",
                ));
            }
        }

        Ok(())
    }

    pub(crate) fn inherit_selection(&mut self) {
        for (include, exclude) in [
            (&mut self.check.include, &mut self.check.exclude),
            (&mut self.format.include, &mut self.format.exclude),
            (&mut self.lint.include, &mut self.lint.exclude),
            (
                &mut self.editor.index.include,
                &mut self.editor.index.exclude,
            ),
            (
                &mut self.editor.imports.include,
                &mut self.editor.imports.exclude,
            ),
        ] {
            if include.is_none() {
                include.clone_from(&self.include);
            }

            if exclude.is_none() {
                exclude.clone_from(&self.exclude);
            }
        }
    }
}

/// Generates the schema directly from the configuration types.
#[must_use]
pub fn schema() -> schemars::Schema {
    schemars::schema_for!(Configuration)
}

/// Optional file-selection overrides.
#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct Selection {
    /// Include patterns; omission falls back to global selection.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Vec<String>")]
    pub include: Option<Vec<String>>,

    /// Exclude patterns; omission falls back to global selection.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Vec<String>")]
    pub exclude: Option<Vec<String>>,
}

/// User preferences for the selected editor features.
#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct Editor {
    /// Diagnostic publication scope.
    pub diagnostics: Diagnostics,

    /// Workspace indexing selection.
    pub index: Selection,

    /// Completion preferences.
    pub completion: Completion,

    /// Automatic import preferences and independent candidate selection.
    pub imports: Imports,

    /// Inlay hint preferences.
    pub hints: Hints,
}

/// Diagnostic publication preferences.
#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct Diagnostics {
    /// Files for which diagnostics should be published.
    pub scope: Scope,
}

/// Diagnostic publication scope; dependencies may still be analyzed.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Scope {
    /// Publish diagnostics for open files.
    #[default]
    OpenFiles,

    /// Publish diagnostics across selected workspace files.
    Workspace,
}

/// Completion preferences.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct Completion {
    /// Offer snippets when the client supports them.
    pub snippets: bool,
}

impl Default for Completion {
    fn default() -> Self {
        Self { snippets: true }
    }
}

/// Import-completion preferences.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct Imports {
    /// Offer import completions.
    pub enabled: bool,

    /// Independently selected import candidates.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Vec<String>")]
    pub include: Option<Vec<String>>,

    /// Excluded import candidates.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Vec<String>")]
    pub exclude: Option<Vec<String>>,

    /// Preferred require argument style.
    pub require: Require,

    /// Binding keyword for generated imports.
    pub binding: Binding,
}

impl Default for Imports {
    fn default() -> Self {
        Self {
            enabled: true,
            include: None,
            exclude: None,
            require: Require::default(),
            binding: Binding::default(),
        }
    }
}

/// Preferred require argument style.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Require {
    /// Generate string paths.
    #[default]
    String,

    /// Generate Roblox instance paths when available.
    Instance,
}

/// Generated import binding keyword.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Binding {
    /// Generate constant bindings.
    #[default]
    Const,

    /// Generate local bindings.
    Local,
}

/// Inlay hint preferences.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct Hints {
    /// Show inferred variable types.
    pub variable_types: Visibility,

    /// Show argument parameter names.
    pub parameter_names: Visibility,

    /// Show inferred return types.
    pub return_types: Visibility,

    /// Omit obvious or redundant hints.
    pub hide_obvious: HintFilter,
}

impl Default for Hints {
    fn default() -> Self {
        Self {
            variable_types: Visibility::Hidden,
            parameter_names: Visibility::Hidden,
            return_types: Visibility::Hidden,
            hide_obvious: HintFilter::ExcludeObvious,
        }
    }
}

/// Hint visibility, represented as a boolean in configuration.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(from = "bool", into = "bool")]
#[schemars(with = "bool", inline)]
pub enum Visibility {
    /// Hide the hint.
    #[default]
    Hidden,

    /// Show the hint.
    Visible,
}

impl From<bool> for Visibility {
    fn from(visible: bool) -> Self {
        if visible { Self::Visible } else { Self::Hidden }
    }
}

impl From<Visibility> for bool {
    fn from(visibility: Visibility) -> Self {
        matches!(visibility, Visibility::Visible)
    }
}

/// Hint filtering, represented by the `hide_obvious` boolean in configuration.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(from = "bool", into = "bool")]
#[schemars(with = "bool", inline)]
pub enum HintFilter {
    /// Include all hints.
    All,

    /// Exclude obvious or redundant hints.
    #[default]
    ExcludeObvious,
}

impl From<bool> for HintFilter {
    fn from(hide_obvious: bool) -> Self {
        if hide_obvious {
            Self::ExcludeObvious
        } else {
            Self::All
        }
    }
}

impl From<HintFilter> for bool {
    fn from(filter: HintFilter) -> Self {
        matches!(filter, HintFilter::ExcludeObvious)
    }
}

/// One ordered namespace of local declaration and documentation files.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Environment {
    /// Documentation namespace, including its leading at sign.
    pub namespace: String,

    /// Declaration paths relative to the defining configuration, loaded in order.
    #[serde(default)]
    pub definitions: Vec<PathBuf>,

    /// Documentation paths relative to the defining configuration.
    #[serde(default)]
    pub documentation: Vec<PathBuf>,
}

/// Native Luau settings independent of upstream source configuration.
#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct Luau {
    /// Registered boolean flags and integer limits.
    pub flags: BTreeMap<String, Flag>,
}

/// A typed native Luau flag override.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(untagged)]
pub enum Flag {
    /// Boolean fast flag.
    Boolean(bool),

    /// Integer fast limit.
    Integer(i32),
}

impl Luau {
    pub(crate) fn native(&self) -> io::Result<BTreeMap<String, instar_bridge::flags::Value>> {
        let flags = self
            .flags
            .iter()
            .map(|(name, value)| {
                let value = match value {
                    Flag::Boolean(value) => instar_bridge::flags::Value::Boolean(*value),
                    Flag::Integer(value) => instar_bridge::flags::Value::Integer(*value),
                };

                (name.clone(), value)
            })
            .collect();

        instar_bridge::flags::validate(&flags)?;

        Ok(flags)
    }
}

/// Roblox integration settings.
#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct Roblox {
    /// Enable Roblox explicitly, or infer it from effective sourcemaps when omitted.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(with = "bool")]
    pub enabled: Option<bool>,

    /// Sourcemap paths; omission discovers the nearest ancestor map, and an empty list disables discovery.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Vec<PathBuf>")]
    pub sourcemaps: Option<Vec<PathBuf>>,

    /// Roblox API security level.
    pub security: Security,
}

/// Roblox API security level.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Security {
    /// Ordinary game scripts.
    #[default]
    None,

    /// Local-user privileged APIs.
    Local,

    /// Studio plugin APIs.
    Plugin,

    /// Roblox-internal APIs.
    Roblox,
}
