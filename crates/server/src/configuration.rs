use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tower_lsp_server::jsonrpc::{Error, Result};

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub(super) struct Configuration {
    pub(super) diagnostic_scope: Scope,

    #[serde(rename = "completion_snippets")]
    pub(super) should_use_completion_snippets: bool,

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

        configuration.hints.retain(|_hint, is_enabled| !*is_enabled);

        Ok(configuration)
    }

    pub(super) fn is_hint_enabled(&self, setting: Hint) -> bool {
        self.hints.get(&setting).copied().unwrap_or(true)
    }
}

impl Default for Configuration {
    fn default() -> Self {
        Self {
            diagnostic_scope: Scope::OpenFiles,
            should_use_completion_snippets: true,
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
    #[serde(rename = "enabled")]
    pub(super) is_enabled: bool,

    pub(super) require: Require,
    pub(super) binding: Binding,
    pub(super) members: Members,
}

impl Default for Imports {
    fn default() -> Self {
        Self {
            is_enabled: true,
            require: Require::String,
            binding: Binding::Local,
            members: Members::All,
        }
    }
}

#[derive(Copy, Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Members {
    All,
    Project,
    Imported,
    None,
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
