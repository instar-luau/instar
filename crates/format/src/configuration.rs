//! Formatter configuration and require grouping policies.
use std::{collections::BTreeSet, num::NonZeroUsize};

use instar_analysis::error::invalid;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Effective formatter settings after configuration inheritance.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct Configuration {
    /// Included file patterns, inheriting global selection when omitted.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Vec<String>")]
    pub include: Option<Vec<String>>,

    /// Excluded file patterns, inheriting global selection when omitted.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Vec<String>")]
    pub exclude: Option<Vec<String>>,

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
}

impl Default for Configuration {
    fn default() -> Self {
        Self {
            include: None,
            exclude: None,
            width: NonZeroUsize::new(120).expect("120 is nonzero"),
            indent_style: IndentStyle::default(),
            indent_width: NonZeroUsize::new(4).expect("4 is nonzero"),
            line_ending: LineEnding::default(),
            quote_style: QuoteStyle::default(),
            requires: Requires::default(),
        }
    }
}

impl Configuration {
    /// Validates file patterns and require groups.
    ///
    /// # Errors
    /// Returns an error for invalid globs or invalid require group names or patterns.
    pub fn validate(&self) -> std::io::Result<()> {
        instar_analysis::selection::validate(self.include.as_deref(), "format.include")?;
        instar_analysis::selection::validate(self.exclude.as_deref(), "format.exclude")?;

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
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
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
    pub fn validate(&self) -> std::io::Result<()> {
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

                        glob::Pattern::new(pattern).map_err(|error| {
                            invalid(format!(
                                "format.requires.groups: group {:?} has invalid glob {pattern:?}: {error}",
                                group.name
                            ))
                        })?;
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
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
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
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CustomGroup {
    /// Unique nonempty name, distinct from built-in names.
    #[schemars(length(min = 1))]
    pub name: String,

    /// Nonempty list of nonempty glob patterns.
    #[schemars(length(min = 1))]
    pub patterns: Vec<String>,
}
