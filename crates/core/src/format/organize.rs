use vermis::tree::{ListEntry, NodeKind, Tree};

use crate::{
    format::{
        Edit, Error, Prepared,
        directives::{self, Directives},
        engine,
        imports::{self, Imports, Require},
        statements,
    },
    settings::format::{BlankLines, Order},
};

/// Reorder recognized imports without formatting other source text.
///
/// # Errors
/// Returns malformed syntax or directives, or a failed preservation check.
pub fn organize(source: &str, configuration: &Prepared) -> Result<Vec<Edit>, Error> {
    let prefix = usize::from(source.starts_with('\u{feff}')) * 3;
    let result = organized(&source[prefix..], configuration);

    match result {
        Ok(mut edits) => {
            for edit in &mut edits {
                edit.range.start += prefix;
                edit.range.end += prefix;
            }

            Ok(edits)
        }

        Err(mut error) => {
            error.range.start += prefix;
            error.range.end += prefix;

            Err(error)
        }
    }
}

fn organized(source: &str, configuration: &Prepared) -> Result<Vec<Edit>, Error> {
    let tree = engine::parse(source.as_bytes())?;
    let directives = directives::collect(&tree)?;
    let mut edits = Vec::new();

    if directives
        .regions()?
        .iter()
        .any(|directive| directive.scope == (0..source.len()))
        || configuration.settings().require.order == Order::Preserve
    {
        return Ok(edits);
    }

    let imports = Imports::instances(&tree);

    for node in &tree.nodes {
        if let NodeKind::Block {
            statements: entries,
        } = &node.kind
        {
            collect(
                &tree,
                tree.list(entries),
                configuration,
                &imports,
                &directives,
                &mut edits,
            );
        }
    }

    edits.sort_by_key(|edit| edit.range.start);

    if edits
        .windows(2)
        .any(|pair| pair[1].range.start < pair[0].range.end)
    {
        return Err(Error::internal(0..0, "overlapping import edits"));
    }

    let mut output = source.to_owned();

    for edit in edits.iter().rev() {
        output.replace_range(edit.range.clone(), &edit.replacement);
    }

    engine::validate(&tree, &output, configuration, Some(&imports), &directives)?;

    Ok(edits)
}

fn collect(
    tree: &Tree<'_>,
    entries: &[ListEntry],
    configuration: &Prepared,
    imports: &Imports,
    directives: &Directives,
    edits: &mut Vec<Edit>,
) {
    let requires: Vec<_> = entries
        .iter()
        .map(|entry| imports.require(tree, entry.node, directives))
        .collect();

    let ranges = statements::ranges(tree, entries);

    for run in imports::runs(tree, requires.iter().map(Option::as_ref)) {
        let boundary = ranges[run.start].start..ranges[run.end - 1].end;

        if ranges[run.clone()]
            .windows(2)
            .any(|pair| pair[1].start < pair[0].end)
            || run.start > 0 && ranges[run.start].start < ranges[run.start - 1].end
            || run.end < ranges.len() && ranges[run.end].start < boundary.end
            || !statements::isolated(tree, entries, &run, &boundary)
        {
            continue;
        }

        let mut order: Vec<_> = run.clone().collect();

        order.sort_by(|left, right| {
            imports::compare(
                configuration,
                &required(&requires, *left).path,
                &required(&requires, *right).path,
            )
        });

        let expected = String::from_utf8_lossy(&tree.source[boundary.clone()]);

        let ending = if expected.contains("\r\n") {
            "\r\n"
        } else {
            "\n"
        };

        let mut replacement = String::new();
        let mut previous = None;

        for index in order {
            if let Some(previous) = previous {
                let blank = index > run.start
                    && tree.source[ranges[index - 1].end..ranges[index].start].contains(&b'\n');

                let grouped = configuration.settings().require.order == Order::Grouped
                    && configuration.settings().require.blank_lines == BlankLines::BetweenGroups
                    && configuration.group(&required(&requires, previous).path)
                        != configuration.group(&required(&requires, index).path);

                replacement.push_str(ending);

                if blank || grouped {
                    replacement.push_str(ending);
                }
            }

            let content = String::from_utf8_lossy(&tree.source[ranges[index].clone()]);
            replacement.push_str(without_ending(&content));
            previous = Some(index);
        }

        if expected.ends_with('\n') {
            replacement.push_str(if expected.ends_with("\r\n") {
                "\r\n"
            } else {
                "\n"
            });
        }

        if expected != replacement {
            edits.push(Edit {
                range: boundary,
                expected: expected.into_owned(),
                replacement,
            });
        }
    }
}

fn required(requires: &[Option<Require>], index: usize) -> &Require {
    requires[index]
        .as_ref()
        .expect("internal error: missing require in import run")
}

fn without_ending(source: &str) -> &str {
    source
        .strip_suffix("\r\n")
        .or_else(|| source.strip_suffix('\n'))
        .unwrap_or(source)
}
