use std::collections::BTreeSet;

use figment::Error;
use globset::{Glob, GlobSet, GlobSetBuilder};

use crate::settings::format::{Configuration, Group, StandardGroup};

/// Immutable formatting settings with validated module-path matchers.
pub struct Prepared {
    configuration: Configuration,
    groups: Vec<Matcher>,
}

impl Prepared {
    /// Validate settings and compile module-path groups for reuse.
    ///
    /// # Errors
    /// Returns invalid group names, duplicate names, or malformed patterns.
    pub fn new(configuration: &Configuration) -> Result<Self, Box<Error>> {
        let mut names = BTreeSet::new();
        let mut groups = Vec::with_capacity(configuration.require.groups.len());

        for (index, group) in configuration.require.groups.iter().enumerate() {
            let (name, matcher) = match group {
                Group::Standard(StandardGroup::Alias) => ("alias", Matcher::Alias),
                Group::Standard(StandardGroup::Relative) => ("relative", Matcher::Relative),
                Group::Standard(StandardGroup::Other) => ("other", Matcher::Other),

                Group::Custom(group) => {
                    if group.name.trim().is_empty()
                        || matches!(group.name.as_str(), "alias" | "relative" | "other")
                        || group.patterns.is_empty()
                    {
                        return Err(invalid(
                            index,
                            "custom require groups need a nonreserved name and at least one pattern",
                        ));
                    }

                    let mut patterns = GlobSetBuilder::new();

                    for pattern in &group.patterns {
                        if pattern.trim().is_empty() {
                            return Err(invalid(index, "require group patterns must not be empty"));
                        }

                        patterns.add(
                            Glob::new(pattern)
                                .map_err(|error| invalid(index, error.to_string()))?,
                        );
                    }

                    let patterns = patterns
                        .build()
                        .map_err(|error| invalid(index, error.to_string()))?;

                    (group.name.as_str(), Matcher::Custom(patterns))
                }
            };

            if !names.insert(name) {
                return Err(invalid(index, "require group names must be unique"));
            }

            groups.push(matcher);
        }

        Ok(Self {
            configuration: configuration.clone(),
            groups,
        })
    }

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

fn invalid(index: usize, reason: impl Into<String>) -> Box<Error> {
    Box::new(
        Error::from(reason.into())
            .with_path("require")
            .with_path("groups")
            .with_path(&index.to_string()),
    )
}
