use std::ops::Range;

use vermis::tree::{NodeIndex, NodeKind, Tree};

use crate::format::{
    Edit, Error, Prepared,
    directives::{self, Kind},
    engine, statements,
};

struct Selection {
    original: Range<usize>,
    proposed: Range<usize>,
}

/// Format a selected byte range, expanding it to complete syntax statements.
///
/// # Errors
/// Returns invalid ranges, malformed syntax or directives, or failed preservation checks.
pub fn range(
    source: &str,
    configuration: &Prepared,
    range: Range<usize>,
) -> Result<Option<Edit>, Error> {
    if source.get(range.clone()).is_none() {
        return Err(Error::input(range, "invalid formatting range"));
    }

    let prefix = usize::from(source.starts_with('\u{feff}')) * 3;
    let requested = range.start.saturating_sub(prefix)..range.end.saturating_sub(prefix);

    let result = selected(&source[prefix..], configuration, &requested);

    match result {
        Ok(edit) => Ok(edit.map(|mut edit| {
            edit.range.start += prefix;
            edit.range.end += prefix;

            edit
        })),

        Err(mut error) => {
            error.range.start += prefix;
            error.range.end += prefix;

            Err(error)
        }
    }
}

fn selected(
    source: &str,
    configuration: &Prepared,
    requested: &Range<usize>,
) -> Result<Option<Edit>, Error> {
    let tree = engine::parse(source.as_bytes())?;
    let directives = directives::collect(&tree)?;

    if directives.policies.iter().any(|directive| {
        matches!(directive.kind, Kind::Verbatim | Kind::Remainder)
            && directive.scope == (0..source.len())
    }) {
        return Ok(None);
    }

    let output = engine::render(&tree, configuration, None, &directives)?;
    let proposed = engine::parse(output.as_bytes())?;
    let mut selection = None;

    select(
        &tree,
        &proposed,
        tree.root,
        proposed.root,
        requested,
        &mut selection,
    );

    let Some(selection) = selection else {
        return Ok(None);
    };

    let expected = &source[selection.original.clone()];
    let replacement = &output[selection.proposed];

    if expected == replacement {
        return Ok(None);
    }

    let mut patched = source.to_owned();
    patched.replace_range(selection.original.clone(), replacement);
    engine::validate(&tree, &patched, configuration, None, &directives)?;

    Ok(Some(Edit {
        range: selection.original,
        expected: expected.to_owned(),
        replacement: replacement.to_owned(),
    }))
}

fn select(
    original: &Tree<'_>,
    proposed: &Tree<'_>,
    left: NodeIndex,
    right: NodeIndex,
    requested: &Range<usize>,
    selection: &mut Option<Selection>,
) {
    if let (NodeKind::Block { statements: before }, NodeKind::Block { statements: after }) =
        (&original.node(left).kind, &proposed.node(right).kind)
    {
        let before = original.list(before);
        let after = proposed.list(after);
        let mut ranges = statements::ranges(original, before);

        if left == block(original) {
            extend(&mut ranges, original.source.len());
        }

        let first = ranges.iter().position(|range| intersects(range, requested));

        let last = ranges
            .iter()
            .rposition(|range| intersects(range, requested));

        if let (Some(first), Some(last)) = (first, last) {
            let boundary = ranges[first].start..ranges[last].end;

            if boundary.start <= requested.start
                && requested.end <= boundary.end
                && statements::isolated(original, before, &(first..last + 1), &boundary)
                && selection.as_ref().is_none_or(|selection| {
                    boundary.end - boundary.start
                        < selection.original.end - selection.original.start
                })
            {
                let mut ranges = statements::ranges(proposed, after);

                if right == block(proposed) {
                    extend(&mut ranges, proposed.source.len());
                }

                let replacement = ranges[first].start..ranges[last].end;

                if statements::isolated(proposed, after, &(first..last + 1), &replacement) {
                    *selection = Some(Selection {
                        original: boundary,
                        proposed: replacement,
                    });
                }
            }
        } else if before.is_empty() && left == block(original) {
            *selection = Some(Selection {
                original: 0..original.source.len(),
                proposed: 0..proposed.source.len(),
            });
        }
    }

    for (left, right) in original
        .children(left)
        .into_iter()
        .zip(proposed.children(right))
    {
        select(original, proposed, left, right, requested, selection);
    }
}

fn intersects(boundary: &Range<usize>, requested: &Range<usize>) -> bool {
    if requested.is_empty() {
        boundary.start <= requested.start && requested.start < boundary.end
    } else {
        boundary.start < requested.end && requested.start < boundary.end
    }
}

fn block(tree: &Tree<'_>) -> NodeIndex {
    if let NodeKind::Root { block, .. } = tree.node(tree.root).kind {
        block
    } else {
        tree.root
    }
}

fn extend(ranges: &mut [Range<usize>], length: usize) {
    if let Some(first) = ranges.first_mut() {
        first.start = 0;
    }

    if let Some(last) = ranges.last_mut() {
        last.end = length;
    }
}
