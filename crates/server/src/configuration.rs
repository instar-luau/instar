use std::{collections::BTreeMap, path::Path};

use globset::{Glob, GlobMatcher};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tower_lsp_server::jsonrpc::{Error, Result};

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub(super) struct Configuration {
    pub(super) diagnostic_scope: Scope,
    pub(super) completion_snippets: bool,

    #[serde(rename = "import")]
    pub(super) imports: Imports,

    hints: BTreeMap<Hint, bool>,
}

impl Configuration {
    pub(super) fn load(value: Value) -> Result<Self> {
        let mut configuration: Self = if value.is_null() {
            Self::default()
        } else {
            serde_json::from_value(value)
                .map_err(|error| Error::invalid_params(error.to_string()))?
        };

        configuration.hints.retain(|_hint, enabled| !*enabled);
        configuration.imports.inclusion = patterns(&configuration.imports.include)?;
        configuration.imports.exclusion = patterns(&configuration.imports.exclude)?;

        Ok(configuration)
    }

    pub(super) fn hint(&self, setting: Hint) -> bool {
        self.hints.get(&setting).copied().unwrap_or(true)
    }
}

impl Default for Configuration {
    fn default() -> Self {
        Self {
            diagnostic_scope: Scope::OpenFiles,
            completion_snippets: true,
            imports: Imports::default(),
            hints: BTreeMap::new(),
        }
    }
}

#[derive(Copy, Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Scope {
    OpenFiles,
    Workspace,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub(super) struct Imports {
    pub(super) enabled: bool,
    pub(super) require: Require,
    pub(super) binding: Binding,
    pub(super) include: Vec<String>,
    pub(super) exclude: Vec<String>,

    #[serde(skip)]
    inclusion: Vec<GlobMatcher>,

    #[serde(skip)]
    exclusion: Vec<GlobMatcher>,
}

impl Imports {
    pub(super) fn accepts(&self, root: &Path, path: &Path) -> bool {
        let path = path.strip_prefix(root).unwrap_or(path);

        self.enabled
            && (self.inclusion.is_empty()
                || self.inclusion.iter().any(|pattern| pattern.is_match(path)))
            && !self.exclusion.iter().any(|pattern| pattern.is_match(path))
    }
}

impl Default for Imports {
    fn default() -> Self {
        Self {
            enabled: true,
            require: Require::String,
            binding: Binding::Local,
            include: Vec::new(),
            exclude: Vec::new(),
            inclusion: Vec::new(),
            exclusion: Vec::new(),
        }
    }
}

#[derive(Copy, Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Require {
    String,
    Instance,
}

#[derive(Copy, Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Binding {
    Local,

    #[serde(rename = "const")]
    Constant,
}

impl Binding {
    pub(super) fn keyword(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::Constant => "const",
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Hint {
    VariableTypes,
    ParameterNames,
    ReturnTypes,
    HideObvious,
}

fn patterns(values: &[String]) -> Result<Vec<GlobMatcher>> {
    values
        .iter()
        .map(|value| {
            if value.is_empty() || value.contains('\0') {
                return Err(Error::invalid_params(
                    "Import patterns must be nonempty and contain no null characters.",
                ));
            }

            Glob::new(value)
                .map(|pattern| pattern.compile_matcher())
                .map_err(|error| Error::invalid_params(error.to_string()))
        })
        .collect()
}
