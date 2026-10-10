use instar_shared::Options;

use instar_syntax::{
    token::{Keyword, Span, Symbol, TokenKind},
    tree::{NodeIndex, NodeKind, NodeList},
};

use super::expressions::{assignment, body, empty, number, same, single, truth, unwrap};
use crate::{RuleIdentity, RuleInput, RuleResult, RuleSettings, request::Context};

impl Context<'_, '_> {
    pub(super) fn condition(&mut self, condition: NodeIndex) {
        if matches!(self.tree.node(condition).kind, NodeKind::Group { .. }) {
            self.finding(
                condition,
                RuleIdentity::ParenthesizedConditions,
                "unnecessary parentheses around condition",
            );
        }

        let node = unwrap(self.tree, condition);

        if truth(self.tree, node).is_some() {
            self.finding(
                node,
                RuleIdentity::ConstantCondition,
                "condition has a fixed truth value",
            );
        }

        if let NodeKind::Unary { operator, .. } = &self.tree.node(node).kind
            && self.tree.token(*operator).kind == TokenKind::Symbol(Symbol::Length)
        {
            self.finding(
                node,
                RuleIdentity::LengthAsCondition,
                "zero is truthy in Luau; compare the length explicitly",
            );
        }
    }

    pub(super) fn loops(&mut self, node: NodeIndex) {
        if let NodeKind::NumericFor {
            step: Some(step), ..
        } = &self.tree.node(node).kind
            && number(self.tree, *step) == Some(0.0)
        {
            self.finding(
                *step,
                RuleIdentity::ZeroStepLoop,
                "numeric loop step is zero",
            );
        }

        match &self.tree.node(node).kind {
            NodeKind::While {
                keyword,
                do_keyword,
                body,
                ..
            }
            | NodeKind::NumericFor {
                keyword,
                do_keyword,
                body,
                ..
            }
            | NodeKind::GenericFor {
                keyword,
                do_keyword,
                body,
                ..
            } if empty(self.tree, *body) => {
                self.emit(
                    Span {
                        start: self.tree.token(*keyword).span.start,
                        end: self.tree.token(do_keyword.unwrap_or(*keyword)).span.end,
                    },
                    RuleIdentity::EmptyLoop,
                    "empty loop body",
                );
            }

            NodeKind::Repeat { keyword, body, .. } if empty(self.tree, *body) => {
                self.emit(
                    self.tree.token(*keyword).span,
                    RuleIdentity::EmptyLoop,
                    "empty loop body",
                );
            }

            _ => {}
        }
    }

    pub(super) fn conditional(
        &mut self,
        node: NodeIndex,
        branches: &NodeList,
        otherwise: Option<NodeIndex>,
    ) {
        let tree = self.tree;
        let branches = tree.list(branches);

        let bodies = branches
            .iter()
            .filter_map(|branch| match &tree.node(branch.node).kind {
                NodeKind::Branch { body, .. } => Some(*body),
                _ => None,
            })
            .collect::<Vec<_>>();

        for branch in branches {
            if let NodeKind::Branch {
                keyword,
                then,
                body,
                ..
            } = &tree.node(branch.node).kind
                && empty(tree, *body)
            {
                self.emit(
                    Span {
                        start: tree.token(*keyword).span.start,
                        end: tree.token(then.unwrap_or(*keyword)).span.end,
                    },
                    RuleIdentity::EmptyIf,
                    "empty conditional branch",
                );
            }
        }

        if let Some(otherwise) = otherwise {
            let body = body(tree, otherwise);

            if empty(tree, body)
                && let NodeKind::Else { keyword, .. } = &tree.node(otherwise).kind
            {
                self.emit(
                    tree.token(*keyword).span,
                    RuleIdentity::EmptyIf,
                    "empty conditional branch",
                );
            }

            if let Some(first) = bodies.first()
                && bodies.iter().all(|other| same(tree, *first, *other))
                && same(tree, *first, body)
            {
                self.finding(
                    node,
                    RuleIdentity::IfSameThenElse,
                    "conditional branches have identical bodies",
                );
            }

            if branches.len() == 1 {
                if let NodeKind::Branch { condition, .. } = &tree.node(branches[0].node).kind
                    && let NodeKind::Unary { operator, .. } =
                        &tree.node(unwrap(tree, *condition)).kind
                    && tree.token(*operator).kind == TokenKind::Keyword(Keyword::Not)
                {
                    self.finding(
                        *condition,
                        RuleIdentity::NegatedCondition,
                        "negated condition can be expressed by swapping branches",
                    );
                }

                if let Some(first) = bodies.first()
                    && let (Some(left), Some(right)) = (single(tree, *first), single(tree, body))
                    && let (Some((left, _)), Some((right, _))) =
                        (assignment(tree, left), assignment(tree, right))
                    && matches!(tree.node(left).kind, NodeKind::Name { .. })
                    && same(tree, left, right)
                {
                    self.finding(
                        node,
                        RuleIdentity::IfExpressionAssignment,
                        "matching branch assignments can use an if expression",
                    );
                }
            }
        } else if branches.len() == 1
            && let Some(first) = bodies.first()
            && let Some(inner) = single(tree, *first)
            && let NodeKind::If {
                branches,
                otherwise: None,
                ..
            } = &tree.node(inner).kind
            && tree.list(branches).len() == 1
        {
            self.finding(
                node,
                RuleIdentity::CollapsibleIf,
                "nested single-branch conditionals can be combined",
            );
        }
    }

    pub(super) fn else_branch(&mut self, node: NodeIndex, ancestors: &[NodeIndex]) {
        let Some(parent) = ancestors.last() else {
            return;
        };

        let NodeKind::If { branches, .. } = &self.tree.node(*parent).kind else {
            return;
        };

        if self.tree.list(branches).iter().all(|branch| {
            let NodeKind::Branch { body, .. } = &self.tree.node(branch.node).kind else {
                return false;
            };

            let NodeKind::Block { statements } = &self.tree.node(*body).kind else {
                return false;
            };

            self.tree.list(statements).last().is_some_and(|statement| {
                matches!(self.tree.node(statement.node).kind, NodeKind::Return { .. })
            })
        }) {
            self.finding(
                node,
                RuleIdentity::ElseAfterReturn,
                "else follows branches that return",
            );
        }
    }
}

pub(super) fn check(context: &mut Context<'_, '_>, node: NodeIndex, ancestors: &[NodeIndex]) {
    match &context.tree.node(node).kind {
        NodeKind::If {
            branches,
            otherwise,
            ..
        } => context.conditional(node, branches, *otherwise),

        NodeKind::Else { .. } => context.else_branch(node, ancestors),

        NodeKind::Branch { condition, .. }
        | NodeKind::While { condition, .. }
        | NodeKind::Repeat { condition, .. }
        | NodeKind::Conditional { condition, .. } => context.condition(*condition),

        _ => {}
    }

    context.loops(node);
}

pub(crate) fn evaluate(
    input: &RuleInput<'_, '_>,
    settings: &RuleSettings,
    options: &Options,
) -> RuleResult {
    let mut context = Context::new(input, settings);
    context.walk(options, check);

    context.finish(options)
}
