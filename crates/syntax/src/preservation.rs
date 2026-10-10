use std::{collections::BTreeSet, mem};

use instar_shared::{Completion, Options, Reason, Span};
use serde::{Deserialize, Serialize};

use vermis::{
    token::{Symbol, TokenKind},
    tree::{NodeIndex, NodeKind, Tree as SyntaxTree},
};

use crate::{BindingResult, DirectiveKind, Tree, bindings, directives, imports::require, literal};

/// Outcome of one required syntax guarantee.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub enum Comparison {
    /// The complete comparison agreed.
    Preserved,

    /// A semantic or byte-preservation difference was found.
    Changed,

    /// Required comparison coverage was unavailable.
    Unavailable(Reason),
}

/// Complete preservation evidence for an exact proposed snapshot.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct PreservationResult {
    /// Node structure and operator binding.
    pub structure: Comparison,

    /// Decoded strings, number spellings and interpolation boundaries.
    pub literals: Comparison,

    /// Comment contents and statement/token attachment.
    pub comments: Comparison,

    /// Scoped source policies.
    pub directives: Comparison,

    /// Parentheses, calls and expression-list shape.
    pub multiple_results: Comparison,

    /// Exact original bytes outside selected spans.
    pub outside: Comparison,

    /// Original selected spans.
    pub selections: Vec<Span>,

    /// Corresponding output spans.
    pub output_ranges: Vec<Span>,

    /// Whether requested require runs were normalized.
    pub import_ordering: bool,

    /// Whether all comparisons completed.
    pub completion: Completion,
}

impl PreservationResult {
    /// Whether every requested guarantee was proved.
    #[must_use]
    pub fn preserved(&self) -> bool {
        self.completion == Completion::Complete
            && [
                &self.structure,
                &self.literals,
                &self.comments,
                &self.directives,
                &self.multiple_results,
                &self.outside,
            ]
            .into_iter()
            .all(|comparison| comparison == &Comparison::Preserved)
    }
}

/// Compares syntax, literals, directives, comments and unselected bytes.
#[must_use]
pub fn preserve(
    original: &Tree<'_>,
    proposed: &Tree<'_>,
    selections: &[Span],
    options: &Options,
) -> PreservationResult {
    let output_ranges = map_ranges(
        original.snapshot().text.as_ref(),
        proposed.snapshot().text.as_ref(),
        selections,
    );

    match output_ranges {
        Some(output_ranges) => preserve_mapped(
            original,
            proposed,
            selections,
            &output_ranges,
            false,
            options,
        ),

        None => unavailable(
            selections,
            false,
            Completion::Incomplete(vec![Reason::FailedPrerequisite]),
        ),
    }
}

/// Compares an explicit range mapping, optionally normalizing recognized require runs.
#[must_use]
pub fn preserve_mapped(
    original: &Tree<'_>,
    proposed: &Tree<'_>,
    selections: &[Span],
    output_ranges: &[Span],
    import_ordering: bool,
    options: &Options,
) -> PreservationResult {
    if let Some(completion) = options.interrupted() {
        return unavailable(selections, import_ordering, completion);
    }

    if original.coverage() != &Completion::Complete
        || proposed.coverage() != &Completion::Complete
        || original.settings() != proposed.settings()
    {
        return unavailable(
            selections,
            import_ordering,
            Completion::Incomplete(vec![Reason::UnsupportedFeature]),
        );
    }

    let original_bindings = bindings(original, options);
    let proposed_bindings = bindings(proposed, options);

    if original_bindings.completion != Completion::Complete
        || proposed_bindings.completion != Completion::Complete
    {
        return unavailable(
            selections,
            import_ordering,
            options
                .interrupted()
                .unwrap_or(Completion::Incomplete(vec![Reason::FailedPrerequisite])),
        );
    }

    let before = fingerprint(
        original.syntax(),
        original.syntax().root,
        &original_bindings,
        import_ordering,
        options,
    );

    let after = fingerprint(
        proposed.syntax(),
        proposed.syntax().root,
        &proposed_bindings,
        import_ordering,
        options,
    );

    if let Some(completion) = options.interrupted() {
        return unavailable(selections, import_ordering, completion);
    }

    let structure = match (&before, &after) {
        (Some(before), Some(after)) => comparison(before == after),
        _ => Comparison::Unavailable(Reason::UnsupportedSyntax),
    };

    let literals = compare_literals(original.syntax(), proposed.syntax(), import_ordering);
    let original_directives = directives(original, options);
    let proposed_directives = directives(proposed, options);

    let policy = comparison(
        original_directives.diagnostics.is_empty()
            && proposed_directives.diagnostics.is_empty()
            && policies(original, &original_directives) == policies(proposed, &proposed_directives),
    );

    let comments = match (&before, &after) {
        (Some(before), Some(after)) => comparison(
            before == after
                && attached_comments(
                    original.syntax(),
                    &original_bindings,
                    import_ordering,
                    options,
                ) == attached_comments(
                    proposed.syntax(),
                    &proposed_bindings,
                    import_ordering,
                    options,
                ),
        ),

        _ => Comparison::Unavailable(Reason::UnsupportedSyntax),
    };

    let outside = comparison(outside(
        original.snapshot().text.as_ref(),
        proposed.snapshot().text.as_ref(),
        selections,
        output_ranges,
    ));

    PreservationResult {
        structure: structure.clone(),
        literals,
        comments,
        directives: policy,
        multiple_results: structure,
        outside,
        selections: selections.to_vec(),
        output_ranges: output_ranges.to_vec(),
        import_ordering,
        completion: options.interrupted().unwrap_or(Completion::Complete),
    }
}

fn compare_literals(
    original: &SyntaxTree<'_>,
    proposed: &SyntaxTree<'_>,
    ordering: bool,
) -> Comparison {
    match (literal_values(original), literal_values(proposed)) {
        (Some(mut before), Some(mut after)) => {
            if ordering {
                before.sort();
                after.sort();
            }

            comparison(before == after)
        }

        _ => Comparison::Unavailable(Reason::UnsupportedSyntax),
    }
}

fn policies(
    tree: &Tree<'_>,
    directives: &crate::DirectiveResult,
) -> Option<Vec<(DirectiveKind, Vec<u8>)>> {
    let mut policies = Vec::new();

    for directive in &directives.directives {
        let scope = directive.scope;

        let value = if matches!(directive.kind, DirectiveKind::Format) {
            tree.snapshot()
                .text
                .as_bytes()
                .get(scope.start..scope.end)?
                .to_vec()
        } else if scope.start == 0 && scope.end == tree.snapshot().text.len() {
            b"source".to_vec()
        } else {
            let syntax = vermis::parse(&tree.snapshot().text.as_bytes()[scope.start..scope.end]);

            normalized_tokens(&syntax, 0, syntax.tokens.len())?
        };

        policies.push((directive.kind.clone(), value));
    }

    Some(policies)
}

fn fingerprint(
    tree: &SyntaxTree<'_>,
    index: NodeIndex,
    bindings: &BindingResult,
    ordering: bool,
    options: &Options,
) -> Option<Vec<u8>> {
    if options.interrupted().is_some() {
        return None;
    }

    let node = tree.node(index);
    let mut result = format!("{:?}", mem::discriminant(&node.kind)).into_bytes();
    let children = tree.children(index);

    if let NodeKind::Block { statements } = &node.kind {
        let entries = tree.list(statements);
        let mut units = Vec::new();

        for entry in entries {
            let value = fingerprint(tree, entry.node, bindings, ordering, options)?;
            let import = sortable(tree, entry.node, bindings);
            units.push((value, import));
        }

        let mut start = 0;

        while start < units.len() {
            if !ordering || units[start].1.is_none() {
                start += 1;
                continue;
            }

            let mut end = start + 1;

            while end < units.len() && units[end].1.is_some() {
                end += 1;
            }

            let mut names = BTreeSet::new();

            if units[start..end]
                .iter()
                .all(|unit| names.insert(unit.1.as_ref().expect("recognized import").clone()))
            {
                units[start..end].sort_by(|left, right| left.0.cmp(&right.0));
            }

            start = end;
        }

        for (value, _) in units {
            append(&mut result, &value);
        }

        return Some(result);
    }

    let mut position = node.tokens.start.get();

    for child in children {
        let child_node = tree.node(child);

        if child_node.tokens.start.get() < position {
            return None;
        }

        append(
            &mut result,
            &normalized_tokens(tree, position, child_node.tokens.start.get())?,
        );

        append(
            &mut result,
            &fingerprint(tree, child, bindings, ordering, options)?,
        );

        position = child_node.tokens.end.get();
    }

    append(
        &mut result,
        &normalized_tokens(tree, position, node.tokens.end.get())?,
    );

    Some(result)
}

fn normalized_tokens(tree: &SyntaxTree<'_>, start: usize, end: usize) -> Option<Vec<u8>> {
    let mut result = Vec::new();

    for token in &tree.tokens[start..end] {
        if matches!(
            token.kind,
            TokenKind::Whitespace
                | TokenKind::Comment
                | TokenKind::BlockComment
                | TokenKind::EndOfFile
                | TokenKind::Symbol(Symbol::Comma | Symbol::Semicolon)
        ) {
            continue;
        }

        let bytes = token.bytes(tree.source);

        let value = match token.kind {
            TokenKind::QuotedString | TokenKind::RawString => literal::decode(bytes)?,
            _ => bytes.to_vec(),
        };

        let kind = if matches!(token.kind, TokenKind::QuotedString | TokenKind::RawString) {
            "string".to_owned()
        } else {
            format!("{:?}", token.kind)
        };

        append(&mut result, kind.as_bytes());
        append(&mut result, &value);
    }

    Some(result)
}

fn sortable(tree: &SyntaxTree<'_>, index: NodeIndex, bindings: &BindingResult) -> Option<Vec<u8>> {
    let NodeKind::Local {
        bindings: names,
        values,
        ..
    } = &tree.node(index).kind
    else {
        return None;
    };

    let ([name], [value]) = (tree.list(names), tree.list(values)) else {
        return None;
    };

    let NodeKind::Binding {
        name,
        annotation: None,
        ..
    } = &tree.node(name.node).kind
    else {
        return None;
    };

    if tree.text(*name) == b"require" {
        return None;
    }

    literal::string(tree, require(tree, value.node, bindings)?)?;

    Some(tree.text(*name).to_vec())
}

fn attached_comments(
    tree: &SyntaxTree<'_>,
    bindings: &BindingResult,
    ordering: bool,
    options: &Options,
) -> Option<Vec<Vec<u8>>> {
    let mut comments = Vec::new();

    for (position, token) in tree.tokens.iter().enumerate() {
        if options.interrupted().is_some() {
            return None;
        }

        if !matches!(token.kind, TokenKind::Comment | TokenKind::BlockComment) {
            continue;
        }

        let mut value = token.bytes(tree.source).to_vec();

        if token.bytes(tree.source).starts_with(b"--!")
            && tree.tokens[..position].iter().all(|token| {
                matches!(
                    token.kind,
                    TokenKind::Whitespace | TokenKind::Comment | TokenKind::BlockComment
                )
            })
        {
            append(&mut value, b"source");
            comments.push(value);
            continue;
        }

        let owner = tree
            .nodes
            .iter()
            .enumerate()
            .filter(|(_, node)| {
                node.tokens.start.get() <= position && position < node.tokens.end.get()
            })
            .min_by_key(|(_, node)| node.tokens.end.get() - node.tokens.start.get());

        if let Some((index, node)) = owner
            && !matches!(
                node.kind,
                NodeKind::Root { .. }
                    | NodeKind::Block { .. }
                    | NodeKind::Function { .. }
                    | NodeKind::If { .. }
                    | NodeKind::Branch { .. }
                    | NodeKind::Else { .. }
                    | NodeKind::While { .. }
                    | NodeKind::Repeat { .. }
                    | NodeKind::NumericFor { .. }
                    | NodeKind::GenericFor { .. }
                    | NodeKind::Do { .. }
            )
        {
            append(
                &mut value,
                &fingerprint(tree, NodeIndex::new(index), bindings, ordering, options)?,
            );

            let gap = tree
                .children(NodeIndex::new(index))
                .iter()
                .take_while(|child| tree.node(**child).tokens.end.get() <= position)
                .count();

            append(&mut value, &gap.to_le_bytes());
            comments.push(value);
            continue;
        }

        let substantive = |kind| {
            !matches!(
                kind,
                TokenKind::Whitespace
                    | TokenKind::Comment
                    | TokenKind::BlockComment
                    | TokenKind::EndOfFile
                    | TokenKind::Symbol(Symbol::Comma | Symbol::Semicolon)
            )
        };

        let previous = tree.tokens[..position]
            .iter()
            .rposition(|token| substantive(token.kind));

        let inline = previous.is_some_and(|previous| {
            !tree.source[tree.tokens[previous].span.end..token.span.start].contains(&b'\n')
        });

        let adjacent = if inline {
            previous
        } else {
            tree.tokens[position + 1..]
                .iter()
                .position(|token| substantive(token.kind))
                .map(|next| position + 1 + next)
        };

        value.push(u8::from(inline));

        if let Some(adjacent) = adjacent {
            let statement = enclosing_statement(tree, adjacent);

            if let Some(statement) = statement {
                append(
                    &mut value,
                    &fingerprint(tree, statement, bindings, ordering, options)?,
                );

                append(&mut value, &attachment(tree, statement, adjacent)?);
            }
        }

        comments.push(value);
    }

    if ordering {
        comments.sort();
    }

    Some(comments)
}

fn enclosing_statement(tree: &SyntaxTree<'_>, position: usize) -> Option<NodeIndex> {
    tree.nodes
        .iter()
        .filter_map(|node| {
            let NodeKind::Block { statements } = &node.kind else {
                return None;
            };

            tree.list(statements)
                .iter()
                .filter(|entry| {
                    let node = tree.node(entry.node);

                    node.tokens.start.get() <= position && position < node.tokens.end.get()
                })
                .min_by_key(|entry| {
                    let node = tree.node(entry.node);

                    node.tokens.end.get() - node.tokens.start.get()
                })
                .map(|entry| entry.node)
        })
        .min_by_key(|index| {
            let node = tree.node(*index);

            node.tokens.end.get() - node.tokens.start.get()
        })
}

fn attachment(tree: &SyntaxTree<'_>, index: NodeIndex, target: usize) -> Option<Vec<u8>> {
    let node = tree.node(index);
    let mut result = format!("{:?}", mem::discriminant(&node.kind)).into_bytes();
    let mut position = node.tokens.start.get();

    for (offset, child) in tree.children(index).into_iter().enumerate() {
        let child_node = tree.node(child);

        if child_node.tokens.start.get() <= target && target < child_node.tokens.end.get() {
            append(&mut result, &offset.to_le_bytes());
            append(&mut result, &attachment(tree, child, target)?);

            return Some(result);
        }

        if child_node.tokens.start.get() > target {
            break;
        }

        append(
            &mut result,
            &normalized_tokens(tree, position, child_node.tokens.start.get())?,
        );

        position = child_node.tokens.end.get();
    }

    append(&mut result, &normalized_tokens(tree, position, target + 1)?);

    Some(result)
}

fn literal_values(tree: &SyntaxTree<'_>) -> Option<Vec<Vec<u8>>> {
    tree.tokens
        .iter()
        .filter(|token| {
            matches!(
                token.kind,
                TokenKind::QuotedString
                    | TokenKind::RawString
                    | TokenKind::Number
                    | TokenKind::InterpolatedStringStart
                    | TokenKind::InterpolatedStringMiddle
                    | TokenKind::InterpolatedStringEnd
                    | TokenKind::InterpolatedStringSimple
            )
        })
        .map(|token| {
            let bytes = token.bytes(tree.source);

            match token.kind {
                TokenKind::QuotedString | TokenKind::RawString => literal::decode(bytes),
                _ => Some(bytes.to_vec()),
            }
        })
        .collect()
}

fn outside(before: &str, after: &str, selections: &[Span], outputs: &[Span]) -> bool {
    if selections.len() != outputs.len() {
        return false;
    }

    let mut original = 0;
    let mut proposed = 0;

    for (selection, output) in selections.iter().zip(outputs) {
        if selection.start < original
            || output.start < proposed
            || selection.end < selection.start
            || output.end < output.start
        {
            return false;
        }

        if before.get(original..selection.start) != after.get(proposed..output.start)
            || before.get(selection.start..selection.end).is_none()
            || after.get(output.start..output.end).is_none()
        {
            return false;
        }

        original = selection.end;
        proposed = output.end;
    }

    before.get(original..) == after.get(proposed..)
}

fn map_ranges(before: &str, after: &str, selections: &[Span]) -> Option<Vec<Span>> {
    let mut output = Vec::new();
    let mut cursor = 0;
    let mut original = 0;

    for (index, selection) in selections.iter().enumerate() {
        let gap = before.get(original..selection.start)?;

        if !after.get(cursor..)?.starts_with(gap) {
            return None;
        }

        cursor += gap.len();

        let next = selections
            .get(index + 1)
            .map_or(before.len(), |next| next.start);

        let gap = before.get(selection.end..next)?;

        let end = if index + 1 == selections.len() {
            after.len().checked_sub(gap.len())?
        } else if gap.is_empty() {
            return None;
        } else {
            let mut occurrences = after.get(cursor..)?.match_indices(gap);
            let (offset, _) = occurrences.next()?;

            if occurrences.next().is_some() {
                return None;
            }

            cursor + offset
        };

        output.push(Span { start: cursor, end });
        cursor = end;
        original = selection.end;
    }

    Some(output)
}

fn append(output: &mut Vec<u8>, value: &[u8]) {
    output.extend_from_slice(&value.len().to_le_bytes());
    output.extend_from_slice(value);
}

fn comparison(preserved: bool) -> Comparison {
    if preserved {
        Comparison::Preserved
    } else {
        Comparison::Changed
    }
}

fn unavailable(
    selections: &[Span],
    import_ordering: bool,
    completion: Completion,
) -> PreservationResult {
    let comparison = Comparison::Unavailable(Reason::FailedPrerequisite);

    PreservationResult {
        structure: comparison.clone(),
        literals: comparison.clone(),
        comments: comparison.clone(),
        directives: comparison.clone(),
        multiple_results: comparison.clone(),
        outside: comparison,
        selections: selections.to_vec(),
        output_ranges: Vec::new(),
        import_ordering,
        completion,
    }
}
