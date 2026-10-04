use std::{borrow::Cow, io};

use glob::{MatchOptions, Pattern};

use vermis::{
    token::{Keyword, Span, TokenKind},
    tree::{ListEntry, NodeIndex, NodeKind, NodeList, Tree},
};

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

pub(super) fn sort<'source>(
    source: &'source str,
    options: &RequiresOptions,
    tree: &Tree<'source>,
) -> io::Result<Cow<'source, str>> {
    if options.order == RequireOrder::Preserve {
        return Ok(Cow::Borrowed(source));
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

    collect(
        tree, tree.root, source, options, &patterns, &mut edits, false,
    );

    if edits
        .iter()
        .all(|edit| source[edit.start..edit.end] == edit.text)
    {
        return Ok(Cow::Borrowed(source));
    }

    let mut output = source.to_owned();
    edits.sort_unstable_by_key(|edit| std::cmp::Reverse(edit.start));

    for edit in edits {
        output.replace_range(edit.start..edit.end, &edit.text);
    }

    Ok(Cow::Owned(output))
}

fn collect(
    tree: &Tree<'_>,
    index: NodeIndex,
    source: &str,
    options: &RequiresOptions,
    patterns: &[(&str, Pattern)],
    edits: &mut Vec<Edit>,
    mut shadowed: bool,
) -> bool {
    if let NodeKind::Block { statements } = &tree.node(index).kind {
        return collect_block(
            tree,
            tree.list(statements),
            source,
            options,
            patterns,
            edits,
            shadowed,
        );
    }

    let mut visit =
        |child, shadowed| collect(tree, child, source, options, patterns, edits, shadowed);

    match &tree.node(index).kind {
        NodeKind::Local {
            bindings, values, ..
        }
        | NodeKind::Constant {
            bindings, values, ..
        } => {
            for value in tree.list(values) {
                visit(value.node, shadowed);
            }

            collect_bindings(tree, bindings, shadowed, &mut visit)
        }

        NodeKind::Binding { annotation, .. } => {
            if let Some(annotation) = annotation {
                visit(*annotation, shadowed);
            }

            shadowed
        }

        NodeKind::Parameters { parameters, .. } => {
            collect_bindings(tree, parameters, shadowed, &mut visit)
        }

        NodeKind::Function { prefix, name, .. } => {
            shadowed |= prefix.is_some_and(|prefix| {
                tree.token(prefix).kind == TokenKind::Keyword(Keyword::Local)
                    || tree.token(prefix).bytes(tree.source) == b"const"
            }) && name.is_some_and(|name| tree.text(name) == b"require");

            let mut inner = shadowed;

            for child in tree.children(index) {
                inner = visit(child, inner);
            }

            shadowed
        }

        NodeKind::If { .. }
        | NodeKind::Conditional { .. }
        | NodeKind::While { .. }
        | NodeKind::Repeat { .. }
        | NodeKind::NumericFor { .. }
        | NodeKind::GenericFor { .. }
        | NodeKind::Do { .. } => collect_scoped_control(tree, index, shadowed, &mut visit),

        _ => {
            for child in tree.children(index) {
                shadowed = visit(child, shadowed);
            }

            shadowed
        }
    }
}

fn collect_block(
    tree: &Tree<'_>,
    statements: &[ListEntry],
    source: &str,
    options: &RequiresOptions,
    patterns: &[(&str, Pattern)],
    edits: &mut Vec<Edit>,
    mut shadowed: bool,
) -> bool {
    let statements = statements
        .iter()
        .map(|entry| entry.node)
        .collect::<Vec<_>>();

    let mut start = 0;

    while start < statements.len() {
        if shadowed
            || require_path(tree, statements[start]).is_none()
            || !standalone(source, tree.node(statements[start]).span)
        {
            shadowed = collect(
                tree,
                statements[start],
                source,
                options,
                patterns,
                edits,
                shadowed,
            );

            start += 1;
            continue;
        }

        let mut end = start + 1;

        while end < statements.len()
            && require_path(tree, statements[end]).is_some()
            && standalone(source, tree.node(statements[end]).span)
            && trivia(
                source,
                line_end(source, tree.node(statements[end - 1]).span.end),
                line_begin(source, tree.node(statements[end]).span.start),
            )
        {
            end += 1;
        }

        if end - start > 1 {
            reorder(
                tree,
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

        for &statement in &statements[start..end] {
            shadowed = collect(tree, statement, source, options, patterns, edits, shadowed);
        }

        start = end;
    }

    shadowed
}

fn collect_bindings(
    tree: &Tree<'_>,
    bindings: &NodeList,
    mut shadowed: bool,
    visit: &mut impl FnMut(NodeIndex, bool) -> bool,
) -> bool {
    for binding in tree.list(bindings) {
        visit(binding.node, shadowed);
        shadowed |= binds_require(tree, binding.node);
    }

    shadowed
}

fn collect_scoped_control(
    tree: &Tree<'_>,
    index: NodeIndex,
    shadowed: bool,
    visit: &mut impl FnMut(NodeIndex, bool) -> bool,
) -> bool {
    match &tree.node(index).kind {
        NodeKind::If {
            branches,
            otherwise,
            ..
        } => {
            for branch in tree.list(branches) {
                visit(branch.node, shadowed);
            }

            if let Some(otherwise) = otherwise {
                visit(*otherwise, shadowed);
            }
        }

        NodeKind::Conditional {
            condition,
            truthy,
            falsy,
            ..
        } => {
            let inner = visit(*condition, shadowed);
            visit(*truthy, inner);
            visit(*falsy, shadowed);
        }

        NodeKind::While {
            condition, body, ..
        } => {
            let inner = visit(*condition, shadowed);
            visit(*body, inner);
        }

        NodeKind::Repeat {
            body, condition, ..
        } => {
            let inner = visit(*body, shadowed);
            visit(*condition, inner);
        }

        NodeKind::NumericFor {
            binding,
            start,
            end,
            step,
            body,
            ..
        } => {
            visit(*start, shadowed);
            visit(*end, shadowed);

            if let Some(step) = step {
                visit(*step, shadowed);
            }

            visit(*binding, shadowed);
            let inner = shadowed || binds_require(tree, *binding);
            visit(*body, inner);
        }

        NodeKind::GenericFor {
            bindings,
            values,
            body,
            ..
        } => {
            for value in tree.list(values) {
                visit(value.node, shadowed);
            }

            let inner = collect_bindings(tree, bindings, shadowed, visit);
            visit(*body, inner);
        }

        NodeKind::Do { body, .. } => {
            visit(*body, shadowed);
        }

        _ => {}
    }

    shadowed
}

fn binds_require(tree: &Tree<'_>, index: NodeIndex) -> bool {
    matches!(&tree.node(index).kind, NodeKind::Binding { name, .. }
        if tree.text(*name) == b"require")
}

fn reorder(
    tree: &Tree<'_>,
    statements: &[NodeIndex],
    previous: Option<NodeIndex>,
    source: &str,
    options: &RequiresOptions,
    patterns: &[(&str, Pattern)],
    edits: &mut Vec<Edit>,
) {
    let first = line_begin(source, tree.node(statements[0]).span.start);
    let last = line_end(source, tree.node(statements[statements.len() - 1]).span.end);
    let before = previous.map(|statement| line_end(source, tree.node(statement).span.end));

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
        let begin = line_begin(source, tree.node(statement).span.start);

        let comments_begin = if index == 0 {
            start
        } else {
            line_end(source, tree.node(statements[index - 1]).span.end)
        };

        let mut text = String::new();

        for line in source[comments_begin..begin].split_inclusive('\n') {
            if !line.trim().is_empty() {
                text.push_str(line);
            }
        }

        text.push_str(&source[begin..line_end(source, tree.node(statement).span.end)]);
        let path = require_path(tree, statement).expect("sortable require");

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

fn require_path(tree: &Tree<'_>, statement: NodeIndex) -> Option<String> {
    let call = match &tree.node(statement).kind {
        NodeKind::CallStatement { call } => *call,

        NodeKind::Local {
            bindings, values, ..
        } => {
            if tree.list(bindings).len() != 1 || tree.list(values).len() != 1 {
                return None;
            }

            if binds_require(tree, tree.list(bindings)[0].node) {
                return None;
            }

            tree.list(values)[0].node
        }

        _ => return None,
    };

    let NodeKind::Call { callee, arguments } = &tree.node(call).kind else {
        return None;
    };

    if !matches!(tree.node(*callee).kind, NodeKind::Name { .. }) || tree.text(*callee) != b"require"
    {
        return None;
    }

    let NodeKind::Arguments { values, .. } = &tree.node(*arguments).kind else {
        return None;
    };

    let [argument] = tree.list(values) else {
        return None;
    };

    crate::string_value(tree.text(argument.node)).ok()
}

fn line_begin(source: &str, at: usize) -> usize {
    source[..at].rfind('\n').map_or(0, |position| position + 1)
}

fn line_end(source: &str, at: usize) -> usize {
    source[at..]
        .find('\n')
        .map_or(source.len(), |offset| at + offset + 1)
}

fn standalone(source: &str, span: Span) -> bool {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_shadowed_require_order() {
        let cases = [
            "local require=print\nrequire('z')\nrequire('a')\n",
            "local function require(value) print(value) end\nrequire('z')\nrequire('a')\n",
            "local function f(require)\nrequire('z')\nrequire('a')\nend\n",
            "for _,require in {} do\nrequire('z')\nrequire('a')\nend\n",
            "for require=1,2 do\nrequire('z')\nrequire('a')\nend\n",
            "local require=print\ndo\nrequire('z')\nrequire('a')\nend\n",
            "local require=print\nlocal function f()\nrequire('z')\nrequire('a')\nend\n",
        ];

        for order in [RequireOrder::Grouped, RequireOrder::Alphabetical] {
            let options = RequiresOptions {
                order,
                ..RequiresOptions::default()
            };

            for source in cases {
                let tree = vermis::parse(source.as_bytes());
                assert!(tree.diagnostics.is_empty(), "{:?}", tree.diagnostics);
                assert_eq!(sort(source, &options, &tree).unwrap(), source, "{source}");
            }
        }
    }

    #[test]
    fn require_declarations_are_sorting_barriers() {
        let cases = [
            "local require=require('./z')\nlocal other=require('./a')\n",
            "local z=require('./z')\nlocal require=require('./a')\n",
            "local z=require('./z')\nlocal require=require('./a')\nlocal other=require('./b')\n",
        ];

        for order in [RequireOrder::Grouped, RequireOrder::Alphabetical] {
            let options = RequiresOptions {
                order,
                ..RequiresOptions::default()
            };

            for source in cases {
                let tree = vermis::parse(source.as_bytes());
                assert!(tree.diagnostics.is_empty(), "{:?}", tree.diagnostics);
                assert_eq!(sort(source, &options, &tree).unwrap(), source, "{source}");
            }
        }
    }

    #[test]
    fn restores_builtin_require_sorting_after_scopes() {
        let scopes = [
            "do\nlocal require=print\nrequire('z')\nrequire('a')\nend\n",
            "do\nlocal function require(value) print(value) end\nrequire('z')\nrequire('a')\nend\n",
            "local function f(require)\nrequire('z')\nrequire('a')\nend\n",
            "for _,require in {} do\nrequire('z')\nrequire('a')\nend\n",
            "for require=1,2 do\nrequire('z')\nrequire('a')\nend\n",
        ];

        for order in [RequireOrder::Grouped, RequireOrder::Alphabetical] {
            let options = RequiresOptions {
                order,
                ..RequiresOptions::default()
            };

            for scope in scopes {
                let source = format!("{scope}require('z')\nrequire('a')\n");
                let expected = format!("{scope}require('a')\nrequire('z')\n");
                let tree = vermis::parse(source.as_bytes());
                assert!(tree.diagnostics.is_empty(), "{:?}", tree.diagnostics);

                assert_eq!(
                    sort(&source, &options, &tree).unwrap(),
                    expected,
                    "{source}"
                );
            }
        }
    }
}
