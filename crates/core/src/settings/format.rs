//! Formatting and require grouping configuration.

use std::num::NonZeroUsize;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{Patterns, schema};

/// Formatting settings and file selection.
#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Configuration {
    /// Replacement inclusion patterns.
    pub include: Option<Patterns>,

    /// Replacement exclusion patterns.
    pub exclude: Option<Patterns>,

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
    #[serde(rename = "final_newline")]
    pub should_emit_final_newline: bool,

    /// Require ordering and grouping.
    pub require: Requires,
}

impl Default for Configuration {
    fn default() -> Self {
        Self {
            include: None,
            exclude: None,
            width: NonZeroUsize::new(120).expect("Internal error: default line width is zero."),
            indent_style: Indentation::default(),
            indent_width: NonZeroUsize::new(4)
                .expect("Internal error: default indentation width is zero."),
            line_ending: LineEnding::default(),
            quote_style: Quotes::default(),
            should_emit_final_newline: true,
            require: Requires::default(),
        }
    }
}

/// Indentation characters.
#[derive(Copy, Clone, Debug, Default, Eq, PartialEq, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Indentation {
    /// Indent with tabs.
    #[default]
    Tabs,

    /// Indent with spaces.
    Spaces,
}

/// Output line ending convention.
#[derive(Copy, Clone, Debug, Default, Eq, PartialEq, Deserialize, JsonSchema, Serialize)]
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
#[derive(Copy, Clone, Debug, Default, Eq, PartialEq, Deserialize, JsonSchema, Serialize)]
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
#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
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
#[derive(Copy, Clone, Debug, Default, Eq, PartialEq, Deserialize, JsonSchema, Serialize)]
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
#[derive(Copy, Clone, Debug, Default, Eq, PartialEq, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BlankLines {
    /// Separate groups with a blank line.
    #[default]
    BetweenGroups,

    /// Do not insert blank lines between groups.
    None,
}

/// A standard require classification or a custom named pattern group.
#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(untagged)]
pub enum Group {
    /// A standard classification.
    Standard(StandardGroup),

    /// A named module-path pattern group.
    Custom(CustomGroup),
}

/// Standard require classifications.
#[derive(Copy, Clone, Debug, Eq, PartialEq, Deserialize, JsonSchema, Serialize)]
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
#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CustomGroup {
    /// Nonempty custom group name.
    #[schemars(extend("allOf" = [schema::pattern()], "not" = { "enum": ["alias", "relative", "other"] }))]
    pub name: String,

    /// Nonempty module-path pattern list.
    #[schemars(length(min = 1), extend("items" = schema::pattern()))]
    pub patterns: Vec<String>,
}
