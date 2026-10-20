use std::path::Path;

use figment::Error;

use ignore::{
    Match,
    gitignore::{Gitignore, GitignoreBuilder},
};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::source::Source;

/// Ordered selection rules relative to their defining configuration file.
#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(transparent)]
pub struct Patterns {
    #[schemars(extend("items" = { "type": "string", "pattern": "^[^\\u0000]*$" }))]
    rules: Vec<String>,

    #[serde(skip, default = "Gitignore::empty")]
    matcher: Gitignore,
}

impl Patterns {
    pub(super) fn resolve(&mut self, source: &Source<'_>, base: &Path) -> Result<(), Box<Error>> {
        let directory = source.base(base);

        if directory.to_str().is_none() {
            return Err(source.error("Pattern base directories must be valid UTF-8."));
        }

        let mut builder = GitignoreBuilder::new(directory);

        for (index, rule) in self.rules.iter().enumerate() {
            let field = source.field(&index.to_string());

            if rule.contains('\0') {
                return Err(field.error("Selection patterns must not contain NUL characters."));
            }

            builder
                .add_line(None, rule)
                .map_err(|error| field.error(error.to_string()))?;
        }

        self.matcher = builder
            .build()
            .map_err(|error| source.error(error.to_string()))?;

        Ok(())
    }

    pub(crate) fn is_match(&self, path: &Path, is_inclusion: bool) -> bool {
        let mut is_matched =
            is_inclusion && self.matcher.num_ignores() == 0 && self.matcher.num_whitelists() != 0;

        let ancestors: Vec<_> = path
            .ancestors()
            .take_while(|ancestor| *ancestor != self.matcher.path())
            .collect();

        for ancestor in ancestors.into_iter().rev() {
            match self.matcher.matched(ancestor, ancestor != path) {
                Match::Ignore(_) if !is_inclusion => return true,
                Match::Whitelist(_) if is_inclusion => return false,
                Match::Ignore(_) => is_matched = true,
                Match::Whitelist(_) => is_matched = false,
                Match::None => {}
            }
        }

        is_matched
    }
}
