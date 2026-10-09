//! Shared file-selection validation and matching.
use std::{io, path::Path};

use glob::{MatchOptions, Pattern};

use crate::error::invalid;

/// Validates an optional list of patterns.
///
/// # Errors
/// Returns invalid data for empty, negated, NUL-containing, or malformed patterns.
pub fn validate(patterns: Option<&[String]>, field: &str) -> io::Result<()> {
    for pattern in patterns.into_iter().flatten() {
        compile(pattern, field)?;
    }

    Ok(())
}

/// Anchors a file pattern to its declaring directory, retaining bare-name recursion.
///
/// # Errors
/// Returns invalid data for invalid patterns or non-UTF-8 directories.
pub fn anchor(base: &Path, pattern: &str) -> io::Result<String> {
    compile(pattern, "selection")?;
    let pattern = pattern.replace('\\', "/");
    let anchored = pattern.starts_with('/') || pattern.trim_end_matches('/').contains('/');
    let pattern = pattern.trim_start_matches('/');

    let base = base
        .to_str()
        .ok_or_else(|| invalid("selection base must be UTF-8"))?;

    let prefix = Pattern::escape(base.replace('\\', "/").trim_end_matches('/'));

    Ok(format!(
        "{prefix}/{}{pattern}",
        if anchored { "" } else { "**/" }
    ))
}

/// Selects candidates by include and then exclude lists.
///
/// # Errors
/// Returns invalid data for invalid patterns.
pub fn selected(
    source: &Path,
    include: Option<&[String]>,
    exclude: Option<&[String]>,
) -> io::Result<bool> {
    Ok(matches(source, include, true)? && !matches(source, exclude, false)?)
}

/// Matches a source or one of its parent directories against anchored file patterns.
///
/// # Errors
/// Returns invalid data for invalid patterns or non-UTF-8 sources.
pub fn matches(source: &Path, patterns: Option<&[String]>, empty: bool) -> io::Result<bool> {
    let Some(patterns) = patterns.filter(|patterns| !patterns.is_empty()) else {
        return Ok(empty);
    };

    let source = source
        .to_str()
        .ok_or_else(|| invalid("selection source must be UTF-8"))?
        .replace('\\', "/");

    for text in patterns {
        let pattern = compile(text.trim_end_matches(['/', '\\']), "selection")?;
        let directory = text.ends_with(['/', '\\']);
        let mut candidate = source.as_str();

        if !directory && wildcard(&pattern, candidate) {
            return Ok(true);
        }

        while let Some((parent, _)) = candidate.rsplit_once('/') {
            if wildcard(&pattern, parent) {
                return Ok(true);
            }

            candidate = parent;
        }
    }

    Ok(false)
}

/// Matches a require string using the shared wildcard syntax without filesystem anchoring.
///
/// # Errors
/// Returns invalid data for invalid patterns.
pub fn string(source: &str, pattern: &str) -> io::Result<bool> {
    Ok(wildcard(
        &compile(pattern, "pattern")?,
        &source.replace('\\', "/"),
    ))
}

fn wildcard(pattern: &Pattern, source: &str) -> bool {
    pattern.matches_with(
        source,
        MatchOptions {
            case_sensitive: true,
            require_literal_separator: true,
            require_literal_leading_dot: false,
        },
    )
}

fn compile(pattern: &str, field: &str) -> io::Result<Pattern> {
    if pattern.is_empty() || pattern.contains('\0') || pattern.starts_with('!') {
        return Err(invalid(format!(
            "{field}: patterns must be nonempty, contain no NUL, and cannot use ! negation"
        )));
    }

    let pattern = pattern.replace('\\', "/");

    Pattern::new(&pattern)
        .map_err(|error| invalid(format!("{field}: invalid glob {pattern:?}: {error}")))
}
