mod layout;
pub use layout::*;

use std::{
    fs, io,
    path::Path,
};

use schemars::JsonSchema;
use serde::Deserialize;

use super::selection::Selection;

#[derive(Debug, Clone, Copy, Default, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Indentation {
    #[default]
    Tabs,

    Spaces,
}

#[derive(Debug, Clone, Copy, Default, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Endings {
    #[default]
    Unix,

    Windows,
}

impl Endings {
    pub(crate) fn text(self) -> &'static str {
        match self {
            Self::Unix => "\n",
            Self::Windows => "\r\n",
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum Quotes {
    #[default]
    AutoPreferDouble,

    AutoPreferSingle,
    ForceDouble,
    ForceSingle,
    Preserve,
}

#[derive(Debug, Clone, Copy, Default, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum Zero {
    #[default]
    Add,

    Strip,
    Preserve,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum Parentheses {
    #[default]
    Always,

    NoSingleString,
    NoSingleTable,

    None,

    Input,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum Spacing {
    #[default]
    Never,

    Definitions,
    Calls,
    Always,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum Semicolons {
    #[default]
    Never,

    Always,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum Expansion {
    #[default]
    WhenNeeded,

    Always,
    Never,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum Separator {
    #[default]
    Comma,

    Semicolon,
}

impl Separator {
    pub(super) fn text(self) -> &'static str {
        match self {
            Self::Comma => ",",
            Self::Semicolon => ";",
        }
    }
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct Tables {
    pub enabled: bool,
    pub width: usize,
    pub separator: Separator,
}

impl Default for Tables {
    fn default() -> Self {
        Self {
            enabled: true,
            width: 60,
            separator: Separator::default(),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum ConditionalExpansion {
    #[default]
    Never,

    Always,
    WhenLarge,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum ConditionalStyle {
    #[default]
    Block,

    Leading,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum Placement {
    #[default]
    SameLine,

    NextLine,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct Conditional {
    pub expand: ConditionalExpansion,
    pub width: usize,
    pub style: ConditionalStyle,
    pub placement: Placement,
    pub indent: usize,
}

impl Default for Conditional {
    fn default() -> Self {
        Self {
            expand: ConditionalExpansion::default(),
            width: 60,
            style: ConditionalStyle::default(),
            placement: Placement::default(),
            indent: 1,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum CallStyle {
    #[default]
    OnePerLine,

    HugLast,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct Calls {
    pub expand: Expansion,
    pub style: CallStyle,
    pub indent: usize,
}

impl Default for Calls {
    fn default() -> Self {
        Self {
            expand: Expansion::default(),
            style: CallStyle::default(),
            indent: 1,
        }
    }
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct Parameters {
    pub expand: Expansion,
    pub indent: usize,
}

impl Default for Parameters {
    fn default() -> Self {
        Self {
            expand: Expansion::default(),
            indent: 1,
        }
    }
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "Independent formatter switches match the reference configuration"
)]
pub struct Options {
    pub collapse_simple_statement: Collapse,
    pub block_newline_gaps: Gaps,
    pub sort_requires: Requires,
    pub require_binding: Binding,
    pub prefer_const: Constants,
    pub unused_imports: Imports,
    pub sort_table_types: Properties,
    pub sort_tables: Sorting,
    pub function_style: Functions,
    pub call_chains: Chains,
    pub type_operators: Operators,
    pub table_types: Tables,
    pub if_expression: Conditional,
    pub function_call: Calls,
    pub enabled: bool,
    pub include: Vec<String>,
    pub exclude: Vec<String>,
    pub recommended: Option<bool>,
    pub call_parentheses: Parentheses,
    pub space_after_function_names: Spacing,
    pub semicolons: Semicolons,
    pub function_declaration: Parameters,
    pub quote_style: Quotes,
    pub leading_zero: Zero,
    pub column_width: usize,
    pub indent_type: Indentation,
    pub indent_width: usize,
    pub line_endings: Endings,

    pub final_newline: bool,

    pub magic_trailing_comma: bool,
    pub space_inside_parens: bool,
    pub space_inside_brackets: bool,
    pub space_inside_braces: bool,
    pub trailing_comma: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            function_call: Calls::default(),
            if_expression: Conditional::default(),
            table_types: Tables::default(),
            collapse_simple_statement: Collapse::default(),
            block_newline_gaps: Gaps::default(),
            sort_requires: Requires::default(),
            require_binding: Binding::default(),
            prefer_const: Constants::default(),
            unused_imports: Imports::default(),
            sort_table_types: Properties::default(),
            sort_tables: Sorting::default(),
            function_style: Functions::default(),
            call_chains: Chains::default(),
            type_operators: Operators::default(),
            enabled: true,
            include: Vec::new(),
            exclude: Vec::new(),
            recommended: None,
            call_parentheses: Parentheses::default(),
            space_after_function_names: Spacing::default(),
            semicolons: Semicolons::default(),
            function_declaration: Parameters::default(),
            quote_style: Quotes::default(),
            leading_zero: Zero::default(),
            column_width: 120,
            indent_type: Indentation::default(),
            indent_width: 4,
            line_endings: Endings::default(),
            final_newline: true,
            magic_trailing_comma: true,
            space_inside_parens: false,
            space_inside_brackets: false,
            space_inside_braces: true,
            trailing_comma: true,
        }
    }
}

impl Options {
    /// # Errors
    /// Returns filesystem, encoding, or configuration failures.
    pub fn discover(path: &Path, explicit: Option<&Path>) -> io::Result<Self> {
        Ok(Configuration::discover(path, explicit)?.options)
    }
}

pub struct Configuration {
    pub options: Options,
    pub selection: Selection,
}

impl Configuration {
    /// # Errors
    /// Returns formatting failures or invalid output.
    pub fn format(&self, source: &[u8]) -> io::Result<Vec<u8>> {
        super::format(source, &self.options)
    }

    /// # Errors
    /// Returns filesystem, encoding, configuration, or glob failures.
    pub fn discover(path: &Path, explicit: Option<&Path>) -> io::Result<Self> {
        let path = crate::source::absolute(path).map_err(io::Error::other)?;
        let mut merged = serde_json::Map::new();
        let mut selection = Selection::default();

        let directory = path
            .parent()
            .ok_or_else(|| io::Error::other("source has no parent"))?;

        if explicit.is_none() {
            for ancestor in directory.ancestors().collect::<Vec<_>>().into_iter().rev() {
                let configuration = ancestor.join("instar.toml");

                match fs::read_to_string(&configuration) {
                    Ok(text) => {
                        merge(
                            &mut merged,
                            &text,
                            &mut selection,
                            &configuration,
                        )
                        .map_err(|error| {
                            io::Error::other(format!("{}: {error}", configuration.display()))
                        })?;
                    }

                    Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                    Err(error) => return Err(error),
                }
            }
        }

        if let Some(explicit) = explicit {
            let explicit = crate::source::absolute(explicit).map_err(io::Error::other)?;

            merge(
                &mut merged,
                &fs::read_to_string(&explicit)?,
                &mut selection,
                &explicit,
            )?;
        }

        if merged.get("recommended") == Some(&serde_json::Value::Bool(false)) {
            for key in [
                "magic_trailing_comma",
                "space_inside_braces",
                "trailing_comma",
            ] {
                merged.entry(key).or_insert(serde_json::Value::Bool(false));
            }
        }

        Ok(Self {
            options: serde_json::from_value(merged.into()).map_err(io::Error::other)?,
            selection,
        })
    }
}

fn overlay(
    under: &mut serde_json::Map<String, serde_json::Value>,
    over: serde_json::Map<String, serde_json::Value>,
) {
    for (key, value) in over {
        match (under.get_mut(&key), value) {
            (Some(serde_json::Value::Object(under)), serde_json::Value::Object(over)) => {
                overlay(under, over);
            }

            (_, value) => {
                under.insert(key, value);
            }
        }
    }
}

fn merge(
    merged: &mut serde_json::Map<String, serde_json::Value>,
    text: &str,
    selection: &mut Selection,
    configuration: &Path,
) -> io::Result<()> {
    crate::project::InstarConfig::parse(text).map_err(io::Error::other)?;
    let mut value: serde_json::Value = toml_edit::de::from_str(text).map_err(io::Error::other)?;
    selection.merge(&value, configuration)?;

    value = value
        .get_mut("format")
        .map_or(serde_json::Value::Null, serde_json::Value::take);

    if let serde_json::Value::Object(fields) = value {
        overlay(merged, fields);
    }

    Ok(())
}
