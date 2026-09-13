use std::{
    io,
    path::{Path, PathBuf},
};

use globset::{Glob, GlobSet, GlobSetBuilder};
use serde_json::Value;

#[derive(Clone, Copy)]
/// Command-specific source selection table.
pub enum Scope {
    /// The `[analyze]` table.
    Analyze,

    /// The `[build]` table.
    Build,

    /// The `[format]` table.
    Format,

    /// The `[lint]` table.
    Lint,
}

impl Scope {
    const fn name(self) -> &'static str {
        match self {
            Self::Analyze => "analyze",
            Self::Build => "build",
            Self::Format => "format",
            Self::Lint => "lint",
        }
    }
}

#[derive(Default)]
/// Inherited source patterns with their declaring configuration directories.
pub struct Selection {
    include: Option<Patterns>,
    exclude: Option<Patterns>,
    root_include: Option<Patterns>,
    root_exclude: Option<Patterns>,
}

struct Patterns {
    root: PathBuf,
    patterns: GlobSet,
}

impl Selection {
    /// # Errors
    /// Returns source-path normalization failures.
    pub fn includes(&self, path: &Path) -> io::Result<bool> {
        let path = crate::source::absolute(path).map_err(io::Error::other)?;

        for (patterns, included) in [
            (&self.include, true),
            (&self.exclude, false),
            (&self.root_include, true),
            (&self.root_exclude, false),
        ] {
            if patterns
                .as_ref()
                .is_some_and(|patterns| patterns.matches(&path))
            {
                return Ok(included);
            }
        }

        Ok(true)
    }

    pub(super) fn merge_root(&mut self, value: &Value, configuration: &Path) -> io::Result<()> {
        replace(&mut self.root_include, value.get("include"), configuration)?;

        replace(&mut self.root_exclude, value.get("exclude"), configuration)
    }

    pub(super) fn merge(
        &mut self,
        value: &Value,
        configuration: &Path,
        scope: Scope,
    ) -> io::Result<()> {
        self.merge_root(value, configuration)?;

        if let Some(settings) = value.get(scope.name()) {
            replace(&mut self.include, settings.get("include"), configuration)?;
            replace(&mut self.exclude, settings.get("exclude"), configuration)?;
        }

        Ok(())
    }
}

impl Patterns {
    fn matches(&self, path: &Path) -> bool {
        path.strip_prefix(&self.root)
            .unwrap_or(path)
            .ancestors()
            .filter(|ancestor| !ancestor.as_os_str().is_empty())
            .any(|ancestor| self.patterns.is_match(ancestor))
    }
}

fn replace(
    target: &mut Option<Patterns>,
    value: Option<&Value>,
    configuration: &Path,
) -> io::Result<()> {
    let Some(value) = value else {
        return Ok(());
    };

    let patterns: Vec<String> = serde_json::from_value(value.clone()).map_err(io::Error::other)?;

    if patterns.is_empty() {
        *target = None;

        return Ok(());
    }

    let mut builder = GlobSetBuilder::new();

    for pattern in patterns {
        builder.add(
            Glob::new(&pattern)
                .map_err(|error| io::Error::other(format!("invalid glob {pattern:?}: {error}")))?,
        );
    }

    let root = configuration
        .parent()
        .ok_or_else(|| io::Error::other("configuration has no parent"))?
        .to_owned();

    *target = Some(Patterns {
        root,
        patterns: builder.build().map_err(io::Error::other)?,
    });

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_patterns_override_global_patterns_without_crossing_scopes() -> io::Result<()> {
        let directory = tempfile::tempdir()?;
        let configuration = directory.path().join("instar.toml");

        let value = toml_edit::de::from_str(
            "exclude = ['global']\n[analyze]\ninclude = ['global/keep.luau']\nexclude = ['analyze']\n[build]\ninclude = ['global/keep.luau']\nexclude = ['build']\n[format]\ninclude = ['global/keep.luau']\nexclude = ['format']\n[lint]\ninclude = ['global/keep.luau']\nexclude = ['lint']",
        )
        .map_err(io::Error::other)?;

        for (scope, name, other) in [
            (Scope::Analyze, "analyze", "build"),
            (Scope::Build, "build", "format"),
            (Scope::Format, "format", "lint"),
            (Scope::Lint, "lint", "analyze"),
        ] {
            let mut selection = Selection::default();
            selection.merge(&value, &configuration, scope)?;

            assert!(!selection.includes(&directory.path().join("global/file.luau"))?);
            assert!(selection.includes(&directory.path().join("global/keep.luau"))?);
            assert!(!selection.includes(&directory.path().join(name).join("file.luau"))?);
            assert!(selection.includes(&directory.path().join(other).join("file.luau"))?);
        }

        Ok(())
    }
}
