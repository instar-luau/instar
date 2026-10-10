use std::{
    collections::BTreeSet,
    io,
    sync::atomic::{AtomicU64, Ordering},
};

use instar_shared::{
    Diagnostic, EditIdentity, EditIntent, EditProposal, Location, Options, SourceSnapshot, Span,
    TextEdit, error::invalid,
};

use instar_syntax::{
    BindingResult,
    token::TokenKind,
    tree::{NodeIndex, NodeKind, Tree},
};

use serde::Serialize;

use crate::RuleIdentity;

static NEXT_PROPOSAL: AtomicU64 = AtomicU64::new(1);

/// Validation already established by lint, not a claim of project semantic validation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum FixValidation {
    /// The replacement preserves the recognized syntax operation.
    Syntax,

    /// The replacement preserves established lexical binding identity.
    Bindings,
}

/// A revision-anchored proposal; project validates semantics and applies it separately.
#[derive(Clone, Debug, Serialize)]
pub struct RuleFix {
    /// Catalog rule proposing the correction.
    pub rule: RuleIdentity,

    /// Shared guarded edits, without write authority.
    pub proposal: EditProposal,

    /// Guarantees established while constructing the proposal.
    pub validation: FixValidation,
}

/// Collects only selected rules' valid, nonconflicting proposals from supplied snapshots.
///
/// # Errors
/// Rejects stale guards, missing snapshots, invalid ranges, conflicting selected edits,
/// inconsistent proposal identities, and interruption. No source is changed.
pub fn collect_fixes(
    fixes: &[RuleFix],
    selected: &[RuleIdentity],
    sources: &[SourceSnapshot],
    options: &Options,
) -> io::Result<Vec<RuleFix>> {
    let mut accepted = Vec::new();
    let mut identities = BTreeSet::new();
    let mut edits: Vec<&TextEdit> = Vec::new();
    let mut source_identities = BTreeSet::new();

    for source in sources {
        options.check()?;
        source.validate()?;

        if !source_identities.insert(source.version.identity) {
            return Err(invalid("duplicate fix snapshot identity"));
        }
    }

    for fix in fixes.iter().filter(|fix| selected.contains(&fix.rule)) {
        options.check()?;

        if !identities.insert(fix.proposal.identity) {
            if accepted.iter().any(|accepted: &RuleFix| {
                accepted.proposal.identity == fix.proposal.identity
                    && accepted.proposal == fix.proposal
                    && accepted.rule == fix.rule
                    && accepted.validation == fix.validation
            }) {
                continue;
            }

            return Err(invalid("conflicting fix proposal identity"));
        }

        if fix.proposal.intent != EditIntent::Fix || fix.proposal.edits.is_empty() {
            return Err(invalid("invalid lint fix proposal intent or empty edits"));
        }

        fix.proposal.validate(sources)?;

        for edit in &fix.proposal.edits {
            options.check()?;

            if edits.iter().any(|existing| conflicts(existing, edit)) {
                return Err(invalid("selected lint fixes conflict"));
            }

            edits.push(edit);
        }

        accepted.push(fix.clone());
    }

    options.check()?;

    Ok(accepted)
}

pub(crate) fn propose(
    rule: RuleIdentity,
    finding: &Diagnostic,
    tree: &Tree<'_>,
    bindings: &BindingResult,
    options: &Options,
) -> Option<RuleFix> {
    if !tree.diagnostics.is_empty()
        || !matches!(
            rule,
            RuleIdentity::ParenthesizedConditions | RuleIdentity::SelfAssignment
        )
    {
        return None;
    }

    let range = [finding.primary.span.start, finding.primary.span.end];
    let mut pending = vec![tree.root];

    while let Some(node) = pending.pop() {
        if options.interrupted().is_some() {
            return None;
        }

        let replacements = match rule {
            RuleIdentity::ParenthesizedConditions => parentheses(tree, node, range),
            RuleIdentity::SelfAssignment => assignment(tree, node, range, bindings),
            _ => Vec::new(),
        };

        if !replacements.is_empty() {
            return Some(RuleFix {
                rule,
                proposal: EditProposal {
                    identity: EditIdentity::new(NEXT_PROPOSAL.fetch_add(1, Ordering::Relaxed)),
                    edits: replacements
                        .into_iter()
                        .map(|(range, replacement)| TextEdit {
                            location: Location {
                                source: finding.primary.source,
                                span: Span {
                                    start: range[0],
                                    end: range[1],
                                },
                            },
                            replacement,
                        })
                        .collect(),
                    guards: vec![finding.primary.source],
                    intent: EditIntent::Fix,
                },
                validation: if rule == RuleIdentity::SelfAssignment {
                    FixValidation::Bindings
                } else {
                    FixValidation::Syntax
                },
            });
        }

        pending.extend(tree.children(node));
    }

    None
}

fn conflicts(left: &TextEdit, right: &TextEdit) -> bool {
    if left.location.source.identity != right.location.source.identity {
        return false;
    }

    let left = left.location.span;
    let right = right.location.span;

    left.start < right.end && right.start < left.end || left.start == right.start
}

fn parentheses(tree: &Tree<'_>, node: NodeIndex, range: [usize; 2]) -> Vec<([usize; 2], String)> {
    let (NodeKind::Branch { condition, .. }
    | NodeKind::While { condition, .. }
    | NodeKind::Repeat { condition, .. }
    | NodeKind::Conditional { condition, .. }) = &tree.node(node).kind
    else {
        return Vec::new();
    };

    let condition = tree.node(*condition);

    if !matches!(condition.kind, NodeKind::Group { .. })
        || [condition.span.start, condition.span.end] != range
        || tree.source.get(range[0]) != Some(&b'(')
        || tree.source.get(range[1].saturating_sub(1)) != Some(&b')')
    {
        return Vec::new();
    }

    vec![
        ([range[0], range[0] + 1], " ".to_owned()),
        ([range[1] - 1, range[1]], " ".to_owned()),
    ]
}

fn assignment(
    tree: &Tree<'_>,
    node: NodeIndex,
    range: [usize; 2],
    bindings: &BindingResult,
) -> Vec<([usize; 2], String)> {
    let NodeKind::Assignment {
        targets, values, ..
    } = &tree.node(node).kind
    else {
        return Vec::new();
    };

    let ([target], [value]) = (tree.list(targets), tree.list(values)) else {
        return Vec::new();
    };

    let target_span = tree.node(target.node).span;

    if [target_span.start, target_span.end] != range
        || bindings.declaration(target.node).is_none()
        || !matches!(tree.node(target.node).kind, NodeKind::Name { .. })
        || !matches!(tree.node(value.node).kind, NodeKind::Name { .. })
        || bindings.references.get(&target.node.get()) != bindings.references.get(&value.node.get())
    {
        return Vec::new();
    }

    let span = tree.node(node).span;

    if tree.tokens.iter().any(|token| {
        matches!(token.kind, TokenKind::Comment | TokenKind::BlockComment)
            && token.span.start < span.end
            && span.start < token.span.end
    }) {
        return Vec::new();
    }

    vec![([span.start, span.end], String::new())]
}
