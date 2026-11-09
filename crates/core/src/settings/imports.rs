//! Automatic import discovery boundaries and filters.

use std::path::{Path, PathBuf};

use figment::Error;
use globset::{GlobBuilder, GlobMatcher};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::source::Source;

/// Project-owned automatic import discovery policy.
#[derive(Clone, Debug, Default, Deserialize, JsonSchema, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Configuration {
    /// Dependency directory globs relative to their defining configuration file.
    pub roots: Vec<String>,

    /// Allowed module globs; an empty list allows all modules.
    pub include: Vec<String>,

    /// Excluded module globs, taking precedence over inclusions.
    pub exclude: Vec<String>,

    #[serde(skip)]
    boundaries: Vec<(PathBuf, GlobMatcher)>,

    #[serde(skip)]
    inclusion: Vec<(PathBuf, GlobMatcher)>,

    #[serde(skip)]
    exclusion: Vec<(PathBuf, GlobMatcher)>,
}

impl Configuration {
    pub(super) fn resolve(&mut self, source: &Source<'_>, base: &Path) -> Result<(), Box<Error>> {
        self.boundaries = patterns(&self.roots, &source.field("roots"), base, true)?;
        self.inclusion = patterns(&self.include, &source.field("include"), base, false)?;
        self.exclusion = patterns(&self.exclude, &source.field("exclude"), base, false)?;

        Ok(())
    }

    /// Return whether the explicit filters allow a module.
    #[must_use]
    pub fn can_import(&self, path: &Path) -> bool {
        (self.inclusion.is_empty() || matches(&self.inclusion, path))
            && !matches(&self.exclusion, path)
    }

    /// Return the outermost configured dependency root containing a module.
    #[must_use]
    pub fn dependency_root<'path>(&self, path: &'path Path) -> Option<&'path Path> {
        path.parent()?
            .ancestors()
            .filter(|directory| {
                self.boundaries.iter().any(|(base, pattern)| {
                    directory.strip_prefix(base).is_ok_and(|relative| {
                        let relative = if relative.as_os_str().is_empty() {
                            Path::new(".")
                        } else {
                            relative
                        };

                        pattern.is_match(relative)
                    })
                })
            })
            .last()
    }
}

fn matches(patterns: &[(PathBuf, GlobMatcher)], path: &Path) -> bool {
    patterns.iter().any(|(directory, pattern)| {
        let relative = path.strip_prefix(directory).unwrap_or(path);

        let relative = if relative.as_os_str().is_empty() {
            Path::new(".")
        } else {
            relative
        };

        pattern.is_match(relative)
    })
}

fn patterns(
    values: &[String],
    source: &Source<'_>,
    base: &Path,
    literal_separator: bool,
) -> Result<Vec<(PathBuf, GlobMatcher)>, Box<Error>> {
    values
        .iter()
        .enumerate()
        .map(|(index, value)| {
            let field = source.field(&index.to_string());

            if value.is_empty() || value.contains('\0') {
                return Err(
                    field.error("Import patterns must be nonempty and contain no null characters.")
                );
            }

            let pattern = GlobBuilder::new(value)
                .literal_separator(literal_separator)
                .build()
                .map_err(|error| field.error(error.to_string()))?;

            Ok((field.base(base).to_owned(), pattern.compile_matcher()))
        })
        .collect()
}
