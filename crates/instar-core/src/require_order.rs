use std::io;

use glob::{MatchOptions, Pattern};
use vermis::{Kind, Parts, View};

use crate::config::{RequireBlankLines, RequireGroup, RequireOrder, RequiresOptions};

struct Entry {
    path: String,
    group: usize,
    text: String,
}

struct Edit {
    start: usize,
    end: usize,
    text: String,
}

pub(crate) fn sort(source: &str, options: &RequiresOptions) -> io::Result<String> {
    if options.order == RequireOrder::Preserve {
        return Ok(source.to_owned());
    }

    let tree = vermis::parse(source.as_bytes());

    if !tree.diagnostics.is_empty() {
        return Ok(source.to_owned());
    }

    let patterns = options
        .groups
        .iter()
        .filter_map(|group| match group {
            RequireGroup::Custom { patterns, .. } => Some(patterns.as_slice()),
            _ => None,
        })
        .flatten()
        .map(|pattern| {
            Pattern::new(pattern)
                .map(|compiled| (pattern.as_str(), compiled))
                .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))
        })
        .collect::<io::Result<Vec<_>>>()?;

    let mut edits = Vec::new();

    if let Some(root) = tree.view(tree.root) {
        collect(root, source, options, &patterns, &mut edits);
    }

    let mut output = source.to_owned();
    edits.sort_unstable_by_key(|edit| std::cmp::Reverse(edit.start));

    for edit in edits {
        output.replace_range(edit.start..edit.end, &edit.text);
    }

    Ok(output)
}

fn collect(
    node: View<'_, '_>,
    source: &str,
    options: &RequiresOptions,
    patterns: &[(&str, Pattern)],
    edits: &mut Vec<Edit>,
) {
    if let Some(Parts::Block { statements }) = node.parts() {
        let statements = statements.collect::<Vec<_>>();
        let mut start = 0;

        while start < statements.len() {
            if require_path(statements[start]).is_none() || !standalone(source, statements[start]) {
                start += 1;
                continue;
            }

            let mut end = start + 1;

            while end < statements.len()
                && require_path(statements[end]).is_some()
                && standalone(source, statements[end])
                && trivia(
                    source,
                    line_end(source, statements[end - 1].span().end),
                    line_begin(source, statements[end].span().start),
                )
            {
                end += 1;
            }

            if end - start > 1 {
                reorder(
                    &statements[start..end],
                    statements
                        .get(start.wrapping_sub(1))
                        .filter(|_| start > 0)
                        .copied(),
                    source,
                    options,
                    patterns,
                    edits,
                );
            }

            start = end;
        }
    }

    for child in node.children() {
        collect(child, source, options, patterns, edits);
    }
}

fn reorder(
    statements: &[View<'_, '_>],
    previous: Option<View<'_, '_>>,
    source: &str,
    options: &RequiresOptions,
    patterns: &[(&str, Pattern)],
    edits: &mut Vec<Edit>,
) {
    let first = line_begin(source, statements[0].span().start);
    let last = line_end(source, statements[statements.len() - 1].span().end);
    let before = previous.map(|statement| line_end(source, statement.span().end));

    let attached = before
        .filter(|&begin| begin <= first && trivia(source, begin, first))
        .filter(|&begin| {
            source[begin..first]
                .lines()
                .any(|line| line.trim_start().starts_with("--"))
        });

    let start = attached.unwrap_or(first);

    let newline = if source[start..last].contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    };

    let mut entries = Vec::with_capacity(statements.len());

    for (index, &statement) in statements.iter().enumerate() {
        let begin = line_begin(source, statement.span().start);

        let comments_begin = if index == 0 {
            start
        } else {
            line_end(source, statements[index - 1].span().end)
        };

        let mut text = String::new();

        for line in source[comments_begin..begin].split_inclusive('\n') {
            if !line.trim().is_empty() {
                text.push_str(line);
            }
        }

        text.push_str(&source[begin..line_end(source, statement.span().end)]);
        let path = require_path(statement).expect("sortable require");

        entries.push(Entry {
            group: group_index(&path, options, patterns),
            path,
            text,
        });
    }

    if options.order == RequireOrder::Grouped {
        entries.sort_by(|left, right| {
            left.group
                .cmp(&right.group)
                .then_with(|| left.path.cmp(&right.path))
        });
    } else {
        entries.sort_by(|left, right| left.path.cmp(&right.path));
    }

    let mut text = String::new();
    let mut group = None;

    for entry in entries {
        if !text.is_empty() {
            text.push_str(newline);

            if options.order == RequireOrder::Grouped
                && options.blank_lines == RequireBlankLines::BetweenGroups
                && group.is_some_and(|previous| previous != entry.group)
            {
                text.push_str(newline);
            }
        }

        text.push_str(entry.text.trim_end_matches(['\r', '\n']));
        group = Some(entry.group);
    }

    if source[start..last].ends_with('\n') {
        text.push_str(newline);
    }

    edits.push(Edit {
        start,
        end: last,
        text,
    });
}

fn require_path(statement: View<'_, '_>) -> Option<String> {
    let call = match statement.parts()? {
        Parts::CallStatement { call } => call,

        Parts::Local {
            mut bindings,
            mut values,
        } => {
            bindings.next()?;

            if bindings.next().is_some() {
                return None;
            }

            let value = values.next()?;

            if values.next().is_some() {
                return None;
            }

            value
        }

        _ => return None,
    };

    let Parts::Call { callee, arguments } = call.parts()? else {
        return None;
    };

    if callee.kind() != Kind::Name || callee.text() != b"require" {
        return None;
    }

    let mut arguments = arguments.children();
    let argument = arguments.next()?;

    if arguments.next().is_some() {
        return None;
    }

    crate::string_value(argument.text()).ok()
}

fn line_begin(source: &str, at: usize) -> usize {
    source[..at].rfind('\n').map_or(0, |position| position + 1)
}

fn line_end(source: &str, at: usize) -> usize {
    source[at..]
        .find('\n')
        .map_or(source.len(), |offset| at + offset + 1)
}

fn standalone(source: &str, statement: View<'_, '_>) -> bool {
    let span = statement.span();

    if !source[line_begin(source, span.start)..span.start]
        .trim()
        .is_empty()
    {
        return false;
    }

    let after = source[span.end..line_end(source, span.end)].trim();
    let after = after.strip_prefix(';').unwrap_or(after).trim_start();

    after.is_empty() || after.starts_with("--")
}

fn trivia(source: &str, begin: usize, end: usize) -> bool {
    source[begin..end].lines().all(|line| {
        let line = line.trim();

        line.is_empty() || (line.starts_with("--") && !line.starts_with("--!"))
    })
}

fn group_index(path: &str, options: &RequiresOptions, patterns: &[(&str, Pattern)]) -> usize {
    for (index, group) in options.groups.iter().enumerate() {
        let matches = match group {
            RequireGroup::Alias => path.starts_with('@'),
            RequireGroup::Relative => path.starts_with("./") || path.starts_with("../"),
            RequireGroup::Other => true,

            RequireGroup::Custom {
                patterns: globs, ..
            } => globs.iter().any(|glob| {
                patterns
                    .iter()
                    .find(|(source, _)| *source == glob)
                    .is_some_and(|(_, pattern)| {
                        pattern.matches_with(
                            path,
                            MatchOptions {
                                case_sensitive: true,
                                require_literal_separator: true,
                                require_literal_leading_dot: false,
                            },
                        )
                    })
            }),
        };

        if matches {
            return index;
        }
    }

    options.groups.len()
}
