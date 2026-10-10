//! Formatter configuration and require grouping policies.
use std::{collections::BTreeSet, io, num::NonZeroUsize, slice};

use instar_shared::error::invalid;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Effective formatter settings after configuration inheritance.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct FormatSettings {
    /// Target line width.
    pub width: NonZeroUsize,

    /// Indentation characters.
    pub indent_style: IndentStyle,

    /// Width of each indentation level.
    pub indent_width: NonZeroUsize,

    /// Output line endings.
    pub line_ending: LineEnding,

    /// String quoting policy.
    pub quote_style: QuoteStyle,

    /// Require ordering and grouping settings.
    pub requires: Requires,

    /// Emit a final line ending for whole-source formatting.
    pub final_newline: bool,
}

impl Default for FormatSettings {
    fn default() -> Self {
        Self {
            width: NonZeroUsize::new(120).expect("120 is nonzero"),
            indent_style: IndentStyle::default(),
            indent_width: NonZeroUsize::new(4).expect("4 is nonzero"),
            line_ending: LineEnding::default(),
            quote_style: QuoteStyle::default(),
            requires: Requires::default(),
            final_newline: true,
        }
    }
}

impl FormatSettings {
    /// Validates require ordering and grouping settings.
    ///
    /// # Errors
    /// Returns an error for invalid globs or invalid require group names or patterns.
    pub fn validate(&self) -> io::Result<()> {
        self.requires.validate()
    }
}

/// Characters used for indentation.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum IndentStyle {
    /// Indent with tabs.
    #[default]
    Tabs,

    /// Indent with spaces.
    Spaces,
}

/// Output line ending convention.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum LineEnding {
    /// Line feed.
    #[default]
    Lf,

    /// Carriage return followed by line feed.
    Crlf,
}

/// Quoting policy for string literals.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum QuoteStyle {
    /// Prefer double quotes unless single quotes avoid escaping.
    #[default]
    PreferDouble,

    /// Prefer single quotes unless double quotes avoid escaping.
    PreferSingle,

    /// Always use double quotes.
    Double,

    /// Always use single quotes.
    Single,

    /// Preserve existing quotes.
    Preserve,
}

/// Require ordering and grouping settings.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct Requires {
    /// Ordering policy for requires.
    pub order: Order,

    /// Ordered groups, with the first matching group taking precedence.
    pub groups: Vec<Group>,

    /// Blank line policy between groups.
    pub blank_lines: BlankLines,
}

impl Default for Requires {
    fn default() -> Self {
        Self {
            order: Order::default(),
            groups: vec![
                Group::Builtin(BuiltinGroup::Alias),
                Group::Builtin(BuiltinGroup::Relative),
                Group::Builtin(BuiltinGroup::Other),
            ],
            blank_lines: BlankLines::default(),
        }
    }
}

impl Requires {
    /// Validates group names and custom glob patterns.
    ///
    /// # Errors
    /// Returns an error for empty, reserved, or duplicate names, empty custom
    /// pattern lists or patterns, or invalid custom globs.
    pub fn validate(&self) -> io::Result<()> {
        let mut names = BTreeSet::new();

        for group in &self.groups {
            let name = match group {
                Group::Builtin(BuiltinGroup::Alias) => "alias",
                Group::Builtin(BuiltinGroup::Relative) => "relative",
                Group::Builtin(BuiltinGroup::Other) => "other",

                Group::Custom(group) => {
                    if group.name.trim().is_empty() {
                        return Err(invalid(
                            "format.requires.groups: group names must not be empty",
                        ));
                    }

                    if matches!(group.name.as_str(), "alias" | "relative" | "other") {
                        return Err(invalid(format!(
                            "format.requires.groups: custom group name {:?} is reserved",
                            group.name
                        )));
                    }

                    if group.patterns.is_empty() {
                        return Err(invalid(format!(
                            "format.requires.groups: custom group {:?} needs patterns",
                            group.name
                        )));
                    }

                    for pattern in &group.patterns {
                        if pattern.trim().is_empty() {
                            return Err(invalid(format!(
                                "format.requires.groups: custom group {:?} has an empty pattern",
                                group.name
                            )));
                        }

                        validate_patterns(
                            Some(slice::from_ref(pattern)),
                            "format.requires.groups",
                        )?;
                    }

                    group.name.as_str()
                }
            };

            if !names.insert(name) {
                return Err(invalid(format!(
                    "format.requires.groups: duplicate group name {name:?}"
                )));
            }
        }

        Ok(())
    }
}

/// Require ordering policy.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Order {
    /// Preserve existing order.
    #[default]
    Preserve,

    /// Sort alphabetically.
    Alphabetical,

    /// Group requires and sort alphabetically within each group.
    Grouped,
}

/// Blank line policy between require groups.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum BlankLines {
    /// Separate groups with a blank line.
    #[default]
    BetweenGroups,

    /// Do not insert blank lines between groups.
    None,
}

/// A built-in group name or a custom named pattern group.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(untagged)]
pub enum Group {
    /// A built-in require classification.
    Builtin(BuiltinGroup),

    /// A custom group matching require paths against glob patterns.
    Custom(CustomGroup),
}

/// Built-in require classifications.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum BuiltinGroup {
    /// Alias-based require paths.
    Alias,

    /// Relative require paths.
    Relative,

    /// Require paths not matched by earlier groups.
    Other,
}

/// A named group matching require paths against glob patterns.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CustomGroup {
    /// Unique nonempty name, distinct from built-in names.
    #[schemars(length(min = 1))]
    pub name: String,

    /// Nonempty list of nonempty glob patterns.
    #[schemars(length(min = 1))]
    pub patterns: Vec<String>,
}

/// Typed command or layer overrides.
#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct FormatOverrides {
    /// Target width override.
    pub width: Option<NonZeroUsize>,

    /// Indentation style override.
    pub indent_style: Option<IndentStyle>,

    /// Indentation width override.
    pub indent_width: Option<NonZeroUsize>,

    /// Line ending override.
    pub line_ending: Option<LineEnding>,

    /// Quote policy override.
    pub quote_style: Option<QuoteStyle>,

    /// Require ordering override.
    pub requires: Option<Requires>,

    /// Final newline override.
    pub final_newline: Option<bool>,
}

impl FormatOverrides {
    /// Applies overrides only after validating the complete effective settings.
    ///
    /// # Errors
    /// Returns invalid require groups without changing the input.
    pub fn apply(&self, settings: &mut FormatSettings) -> io::Result<()> {
        let mut proposed = settings.clone();

        if let Some(value) = self.width {
            proposed.width = value;
        }

        if let Some(value) = self.indent_style {
            proposed.indent_style = value;
        }

        if let Some(value) = self.indent_width {
            proposed.indent_width = value;
        }

        if let Some(value) = self.line_ending {
            proposed.line_ending = value;
        }

        if let Some(value) = self.quote_style {
            proposed.quote_style = value;
        }

        if let Some(value) = &self.requires {
            proposed.requires = value.clone();
        }

        if let Some(value) = self.final_newline {
            proposed.final_newline = value;
        }

        proposed.validate()?;
        *settings = proposed;

        Ok(())
    }
}

fn validate_patterns(patterns: Option<&[String]>, setting: &str) -> io::Result<()> {
    for pattern in patterns.into_iter().flatten() {
        globset::Glob::new(pattern).map_err(|error| invalid(format!("{setting}: {error}")))?;
    }

    Ok(())
}
