//! Formatting and require grouping configuration.

use std::{collections::BTreeSet, num::NonZeroUsize};

use figment::Error;
use globset::{Glob, GlobSet, GlobSetBuilder};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::schema;

/// Formatting settings and file selection.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct Configuration {
    /// Replacement inclusion patterns.
    #[schemars(extend("items" = schema::pattern()))]
    pub include: Option<Vec<String>>,

    /// Replacement exclusion patterns.
    #[schemars(extend("items" = schema::pattern()))]
    pub exclude: Option<Vec<String>>,

    /// Positive target line width.
    pub width: NonZeroUsize,

    /// Indentation characters.
    pub indent_style: Indentation,

    /// Positive indentation width.
    pub indent_width: NonZeroUsize,

    /// Output line endings.
    pub line_ending: LineEnding,

    /// String quoting policy.
    pub quote_style: Quotes,

    /// Emit a final line ending for whole-source formatting.
    pub final_newline: bool,

    /// Require ordering and grouping.
    pub require: Requires,
}

impl Configuration {
    /// Validate settings and compile module-path groups for reuse.
    ///
    /// # Errors
    /// Returns invalid group names, duplicate names, or malformed patterns.
    pub fn prepare(self) -> Result<Prepared, Box<Error>> {
        let mut names = BTreeSet::new();
        let mut groups = Vec::with_capacity(self.require.groups.len());

        for (index, group) in self.require.groups.iter().enumerate() {
            let (name, matcher) = match group {
                Group::Standard(StandardGroup::Alias) => ("alias", Matcher::Alias),
                Group::Standard(StandardGroup::Relative) => ("relative", Matcher::Relative),
                Group::Standard(StandardGroup::Other) => ("other", Matcher::Other),

                Group::Custom(group) => {
                    if group.name.trim().is_empty()
                        || matches!(group.name.as_str(), "alias" | "relative" | "other")
                        || group.patterns.is_empty()
                    {
                        return Err(group_error(
                            index,
                            "custom require groups need a nonreserved name and at least one pattern",
                        ));
                    }

                    let mut patterns = GlobSetBuilder::new();

                    for pattern in &group.patterns {
                        if pattern.trim().is_empty() {
                            return Err(group_error(
                                index,
                                "require group patterns must not be empty",
                            ));
                        }

                        patterns.add(
                            Glob::new(pattern)
                                .map_err(|error| group_error(index, error.to_string()))?,
                        );
                    }

                    let patterns = patterns
                        .build()
                        .map_err(|error| group_error(index, error.to_string()))?;

                    (group.name.as_str(), Matcher::Custom(patterns))
                }
            };

            if !names.insert(name) {
                return Err(group_error(index, "require group names must be unique"));
            }

            groups.push(matcher);
        }

        Ok(Prepared {
            configuration: self,
            groups,
        })
    }
}

impl Default for Configuration {
    fn default() -> Self {
        Self {
            include: None,
            exclude: None,
            width: NonZeroUsize::new(120).expect("120 is nonzero"),
            indent_style: Indentation::default(),
            indent_width: NonZeroUsize::new(4).expect("4 is nonzero"),
            line_ending: LineEnding::default(),
            quote_style: Quotes::default(),
            final_newline: true,
            require: Requires::default(),
        }
    }
}

/// Immutable formatting settings with validated module-path matchers.
pub struct Prepared {
    configuration: Configuration,
    groups: Vec<Matcher>,
}

impl Prepared {
    /// Return the settings used to prepare this configuration.
    #[must_use]
    pub fn settings(&self) -> &Configuration {
        &self.configuration
    }

    /// Return the first matching require group, or the trailing unclassified group.
    #[must_use]
    pub fn group(&self, path: &str) -> usize {
        self.groups
            .iter()
            .position(|group| match group {
                Matcher::Alias => path.starts_with('@'),
                Matcher::Relative => path.starts_with("./") || path.starts_with("../"),
                Matcher::Other => true,
                Matcher::Custom(patterns) => patterns.is_match(path),
            })
            .unwrap_or(self.groups.len())
    }
}

enum Matcher {
    Alias,
    Relative,
    Other,
    Custom(GlobSet),
}

/// Indentation characters.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Indentation {
    /// Indent with tabs.
    #[default]
    Tabs,

    /// Indent with spaces.
    Spaces,
}

/// Output line ending convention.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
pub enum LineEnding {
    /// Line feed.
    #[default]
    #[serde(rename = "lf")]
    LineFeed,

    /// Carriage return followed by line feed.
    #[serde(rename = "crlf")]
    CarriageReturnLineFeed,
}

/// Quoting policy for string literals.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Quotes {
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
    /// Require ordering policy.
    pub order: Order,

    /// Ordered groups, with the first matching group taking precedence.
    #[schemars(extend("uniqueItems" = true))]
    pub groups: Vec<Group>,

    /// Blank lines between groups.
    pub blank_lines: BlankLines,
}

impl Default for Requires {
    fn default() -> Self {
        Self {
            order: Order::default(),
            groups: vec![
                Group::Standard(StandardGroup::Alias),
                Group::Standard(StandardGroup::Relative),
                Group::Standard(StandardGroup::Other),
            ],
            blank_lines: BlankLines::default(),
        }
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

/// A standard require classification or a custom named pattern group.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(untagged)]
pub enum Group {
    /// A standard classification.
    Standard(StandardGroup),

    /// A named module-path pattern group.
    Custom(CustomGroup),
}

/// Standard require classifications.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum StandardGroup {
    /// Alias-based require paths.
    Alias,

    /// Relative require paths.
    Relative,

    /// Require paths unmatched by earlier groups.
    Other,
}

/// A named group matching module paths against glob patterns.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CustomGroup {
    /// Nonempty custom group name.
    #[schemars(extend("allOf" = [schema::pattern()], "not" = { "enum": ["alias", "relative", "other"] }))]
    pub name: String,

    /// Nonempty module-path pattern list.
    #[schemars(length(min = 1), extend("items" = schema::pattern()))]
    pub patterns: Vec<String>,
}

fn group_error(index: usize, reason: impl Into<String>) -> Box<Error> {
    Box::new(
        Error::from(reason.into())
            .with_path("require")
            .with_path("groups")
            .with_path(&index.to_string()),
    )
}
