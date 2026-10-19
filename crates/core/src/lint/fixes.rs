use crate::lint::{Edit, Error, Finding, syntax};

/// Apply nonoverlapping correction groups and reparse the resulting source.
///
/// # Errors
/// Rejects malformed input, stale or invalid ranges, and corrections producing invalid syntax.
pub fn apply(source: &str, findings: &[Finding]) -> Result<String, Error> {
    syntax::parse(source)?;

    let mut proposals: Vec<_> = findings
        .iter()
        .filter(|finding| !finding.edits.is_empty())
        .collect();

    proposals.sort_by(|left, right| {
        left.range
            .start
            .cmp(&right.range.start)
            .then_with(|| left.rule.cmp(&right.rule))
    });

    let mut accepted: Vec<&Edit> = Vec::new();

    for finding in proposals {
        let mut edits: Vec<_> = finding.edits.iter().collect();
        edits.sort_by_key(|edit| (edit.range.start, edit.range.end));

        for edit in &edits {
            if source.get(edit.range.clone()) != Some(edit.expected.as_str()) {
                return Err(Error {
                    range: edit.range.clone(),
                    message: "correction does not match the source snapshot".to_owned(),
                });
            }
        }

        if edits.windows(2).any(|pair| overlap(pair[0], pair[1])) {
            return Err(Error {
                range: finding.range.clone(),
                message: "correction contains overlapping edits".to_owned(),
            });
        }

        if edits
            .iter()
            .any(|edit| accepted.iter().any(|previous| overlap(edit, previous)))
        {
            continue;
        }

        accepted.extend(edits);
    }

    accepted.sort_by_key(|edit| (edit.range.start, edit.range.end));
    let mut corrected = source.to_owned();

    for edit in accepted.into_iter().rev() {
        corrected.replace_range(edit.range.clone(), &edit.replacement);
    }

    syntax::parse(&corrected).map_err(|error| Error {
        range: error.range,
        message: format!("correction produced invalid syntax: {}", error.message),
    })?;

    Ok(corrected)
}

fn overlap(left: &Edit, right: &Edit) -> bool {
    left.range.start <= right.range.end
        && right.range.start <= left.range.end
        && (left.range.start < right.range.end && right.range.start < left.range.end
            || left.range.is_empty()
            || right.range.is_empty())
}
