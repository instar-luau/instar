use std::{
    io,
    path::{Path, PathBuf},
};

use globset::{Glob, GlobSet, GlobSetBuilder};
use serde_json::Value;

#[derive(Default)]
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

    pub(super) fn merge(&mut self, value: &Value, configuration: &Path) -> io::Result<()> {
        self.merge_root(value, configuration)?;

        if let Some(format) = value.get("format") {
            replace(&mut self.include, format.get("include"), configuration)?;
            replace(&mut self.exclude, format.get("exclude"), configuration)?;
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
