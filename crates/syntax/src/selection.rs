use std::ops::Range;

use instar_shared::{Completion, Options, Reason, Span};

use vermis::{
    token::TokenKind,
    tree::{ListEntry, NodeIndex, NodeKind, NodeList},
};

use crate::{DirectiveKind, Tree, directives};

/// One safely expanded request and its enclosing context.
#[derive(Clone, Debug)]
pub struct SyntaxSelection {
    /// Original caller selection.
    pub requested: Span,

    /// Expanded complete statements and lines.
    pub expanded: Vec<Span>,

    /// Selected statement identities.
    pub statements: Vec<ListEntry>,

    /// Enclosing indentation bytes, retaining tabs for the layout settings.
    pub indentation: Vec<u8>,

    /// Enclosing block adds one indentation level.
    pub nested: bool,

    /// Explicit reason a malformed selection cannot be formatted.
    pub unavailable: Option<Reason>,
}

/// Expanded selections and the protected boundaries they intersect.
#[derive(Clone, Debug)]
pub struct SelectionResult {
    /// One outcome per requested span.
    pub selections: Vec<SyntaxSelection>,

    /// Merged expanded spans.
    pub expanded: Vec<Span>,

    /// Scoped formatting suppression boundaries.
    pub suppressions: Vec<Span>,

    /// Whether all requested selections were examined.
    pub completion: Completion,
}

/// Expands ranges against one immutable snapshot and merges overlaps.
#[must_use]
pub fn selections(tree: &Tree<'_>, ranges: &[Span], options: &Options) -> SelectionResult {
    let directives = directives(tree, options);

    let mut result = SelectionResult {
        selections: Vec::new(),
        expanded: Vec::new(),
        suppressions: directives
            .directives
            .iter()
            .filter(|directive| matches!(directive.kind, DirectiveKind::Format))
            .map(|directive| directive.scope)
            .collect(),
        completion: tree.coverage().clone(),
    };

    for requested in ranges {
        if let Some(completion) = options.interrupted() {
            result.completion = completion;
            break;
        }

        let selected = expand(tree, *requested, options);
        result.expanded.extend(selected.expanded.iter().copied());
        result.selections.push(selected);
    }

    result.expanded.sort_by_key(|span| (span.start, span.end));
    let mut merged: Vec<Span> = Vec::new();

    for span in &result.expanded {
        if let Some(previous) = merged.last_mut()
            && span.start <= previous.end
        {
            previous.end = previous.end.max(span.end);
        } else {
            merged.push(*span);
        }
    }

    result.expanded = merged;

    result
}

/// Expands one range to syntax and line boundaries.
#[must_use]
pub fn expand(tree: &Tree<'_>, requested: Span, options: &Options) -> SyntaxSelection {
    let mut selected = SyntaxSelection {
        requested,
        expanded: Vec::new(),
        statements: Vec::new(),
        indentation: Vec::new(),
        nested: false,
        unavailable: None,
    };

    if requested.validate(tree.snapshot()).is_err() {
        selected.unavailable = Some(Reason::FailedPrerequisite);

        return selected;
    }

    let syntax = tree.syntax();
    let mut range = lines(syntax.source, requested.start..requested.end);
    let prefix = usize::from(tree.snapshot().text.starts_with('\u{feff}')) * 3;

    if trivia(tree, range.clone()) {
        return selected;
    }

    range.start = range.start.max(prefix);

    loop {
        if options.interrupted().is_some() {
            selected.unavailable = Some(Reason::FailedPrerequisite);

            return selected;
        }

        let mut tokens = syntax.tokens.iter().filter(|token| {
            token.span.start < range.end
                && range.start < token.span.end
                && !matches!(token.kind, TokenKind::Whitespace | TokenKind::EndOfFile)
        });

        let Some(first) = tokens.next() else {
            return selected;
        };

        let last = tokens.next_back().unwrap_or(first);
        let mut candidate = None;
        let mut size = usize::MAX;
        let mut pending = vec![(syntax.root, None)];

        while let Some((index, parent)) = pending.pop() {
            if options.interrupted().is_some() {
                selected.unavailable = Some(Reason::FailedPrerequisite);

                return selected;
            }

            let node = syntax.node(index);

            if let NodeKind::Block { statements } = &node.kind {
                let root = parent.is_some_and(|parent| {
                    matches!(syntax.node(parent).kind, NodeKind::Root { .. })
                });

                if (root || node.span.start <= first.span.start && last.span.end <= node.span.end)
                    && node.span.end - node.span.start <= size
                    && let Some(found) = select(tree, statements, parent, range.clone())
                {
                    candidate = Some(found);
                    size = node.span.end - node.span.start;
                }
            }

            pending.extend(
                syntax
                    .children(index)
                    .into_iter()
                    .map(|child| (child, Some(index))),
            );
        }

        let Some((statements, span, indentation, nested)) = candidate else {
            selected.unavailable = Some(Reason::UnsupportedSyntax);

            return selected;
        };

        let mut expanded = lines(
            syntax.source,
            span.start.min(first.span.start).min(range.start)
                ..span.end.max(last.span.end).max(range.end),
        );

        expanded.start = expanded.start.max(prefix);

        if expanded != range {
            range = expanded;
            continue;
        }

        selected.expanded.push(Span {
            start: range.start,
            end: range.end,
        });

        selected.statements = statements;
        selected.indentation = indentation;
        selected.nested = nested;

        let malformed = malformed(tree, &selected.statements, options);

        if options.interrupted().is_some() {
            selected.unavailable = Some(Reason::FailedPrerequisite);
        } else if malformed
            || syntax.diagnostics.iter().any(|diagnostic| {
                diagnostic.span.start < range.end && range.start <= diagnostic.span.end
            })
        {
            selected.unavailable = Some(Reason::UnsupportedSyntax);
        }

        return selected;
    }
}

fn malformed(tree: &Tree<'_>, statements: &[ListEntry], options: &Options) -> bool {
    let syntax = tree.syntax();

    let mut pending = statements
        .iter()
        .map(|entry| entry.node)
        .collect::<Vec<_>>();

    while let Some(index) = pending.pop() {
        if options.interrupted().is_some() {
            break;
        }

        if matches!(
            syntax.node(index).kind,
            NodeKind::Error
                | NodeKind::Missing { .. }
                | NodeKind::Function {
                    body: Some(_),
                    end: None,
                    ..
                }
                | NodeKind::If { end: None, .. }
                | NodeKind::While { end: None, .. }
                | NodeKind::GenericFor { end: None, .. }
                | NodeKind::NumericFor {
                    end_keyword: None,
                    ..
                }
                | NodeKind::Do { end: None, .. }
                | NodeKind::Class { end: None, .. }
                | NodeKind::Repeat { until: None, .. }
                | NodeKind::Parameters { closing: None, .. }
                | NodeKind::Arguments {
                    opening: Some(_),
                    closing: None,
                    ..
                }
                | NodeKind::AttributeGroup { closing: None, .. }
                | NodeKind::Generics { closing: None, .. }
                | NodeKind::Group { closing: None, .. }
                | NodeKind::Index { closing: None, .. }
                | NodeKind::InstantiationArguments { closing: None, .. }
                | NodeKind::Table { closing: None, .. }
                | NodeKind::TableField {
                    opening: Some(_),
                    closing: None,
                    ..
                }
                | NodeKind::TypeTable { closing: None, .. }
                | NodeKind::TypeField {
                    opening: Some(_),
                    closing: None,
                    ..
                }
                | NodeKind::TypeIndexer { closing: None, .. }
                | NodeKind::TypeGroup { closing: None, .. }
                | NodeKind::TypePack {
                    opening: Some(_),
                    closing: None,
                    ..
                }
                | NodeKind::TypeArguments { closing: None, .. }
                | NodeKind::TypeOf { closing: None, .. }
        ) {
            return true;
        }

        pending.extend(syntax.children(index));
    }

    false
}

fn trivia(tree: &Tree<'_>, range: Range<usize>) -> bool {
    tree.syntax()
        .tokens
        .iter()
        .filter(|token| token.span.start < range.end && range.start < token.span.end)
        .all(|token| {
            matches!(
                token.kind,
                TokenKind::Whitespace
                    | TokenKind::Comment
                    | TokenKind::BlockComment
                    | TokenKind::EndOfFile
            )
        })
}

fn select(
    tree: &Tree<'_>,
    statements: &NodeList,
    parent: Option<NodeIndex>,
    range: Range<usize>,
) -> Option<(Vec<ListEntry>, Span, Vec<u8>, bool)> {
    let syntax = tree.syntax();
    let statements = syntax.list(statements);

    let first = statements
        .iter()
        .position(|entry| syntax.node(entry.node).span.end > range.start)?;

    let last = statements
        .iter()
        .rposition(|entry| syntax.node(entry.node).span.start < range.end)?;

    if first > last {
        return None;
    }

    let beginning = syntax.node(statements[first].node).span.start;

    let ending = statements[last].separator.map_or_else(
        || syntax.node(statements[last].node).span.end,
        |separator| syntax.token(separator).span.end,
    );

    let parent =
        parent.filter(|parent| !matches!(syntax.node(*parent).kind, NodeKind::Root { .. }));

    let indentation = parent.map_or_else(Vec::new, |parent| {
        let offset = syntax.node(parent).span.start;

        let start = syntax.source[..offset]
            .iter()
            .rposition(|byte| matches!(byte, b'\r' | b'\n'))
            .map_or(0, |index| index + 1);

        syntax.source[start..offset]
            .iter()
            .copied()
            .take_while(|byte| matches!(byte, b' ' | b'\t'))
            .collect()
    });

    Some((
        statements[first..=last].to_vec(),
        Span {
            start: beginning,
            end: ending,
        },
        indentation,
        parent.is_some(),
    ))
}

fn lines(source: &[u8], range: Range<usize>) -> Range<usize> {
    let start = source[..range.start]
        .iter()
        .rposition(|byte| matches!(byte, b'\r' | b'\n'))
        .map_or(0, |index| index + 1);

    let mut end = range.end;

    if end == range.start || end == 0 || !matches!(source[end - 1], b'\r' | b'\n') {
        while end < source.len() && !matches!(source[end], b'\r' | b'\n') {
            end += 1;
        }

        if source.get(end) == Some(&b'\r') {
            end += 1;
        }

        if source.get(end) == Some(&b'\n') {
            end += 1;
        }
    } else if source.get(end - 1) == Some(&b'\r') && source.get(end) == Some(&b'\n') {
        end += 1;
    }

    start..end
}
