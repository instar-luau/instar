//! Shared file-selection validation and matching.

use std::{io, path::Path};

use crate::error::invalid;

fn compile(pattern: &str, field: &str) -> io::Result<glob::Pattern> {
    if pattern.is_empty() || pattern.contains('\0') {
        return Err(invalid(format!(
            "{field}: file patterns must be nonempty and contain no NUL"
        )));
    }

    glob::Pattern::new(pattern)
        .map_err(|error| invalid(format!("{field}: invalid glob {pattern:?}: {error}")))
}

/// Validates an optional list of file patterns.
///
/// # Errors
/// Returns invalid data for empty, NUL-containing, or malformed patterns.
pub fn validate(patterns: Option<&[String]>, field: &str) -> io::Result<()> {
    for pattern in patterns.into_iter().flatten() {
        compile(pattern, field)?;
    }

    Ok(())
}

/// Matches an absolute source against optional file patterns.
///
/// # Errors
/// Returns invalid data for invalid patterns.
pub fn matches(source: &Path, patterns: Option<&[String]>, empty: bool) -> io::Result<bool> {
    let Some(patterns) = patterns.filter(|patterns| !patterns.is_empty()) else {
        return Ok(empty);
    };

    patterns.iter().try_fold(false, |matched, pattern| {
        Ok(compile(pattern, "selection")?.matches_path(source) || matched)
    })
}
