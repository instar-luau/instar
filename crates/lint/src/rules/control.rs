use instar_shared::Options;

use instar_syntax::{
    token::{Keyword, Span, Symbol, TokenKind},
    tree::{NodeIndex, NodeKind},
};

use super::expressions::{assignment, body, empty, number, same, single, truth, unwrap};
use crate::{RuleIdentity, RuleInput, RuleResult, RuleSettings, request::Context};

impl Context<'_, '_> {
    fn parenthesized_condition(&mut self, node: NodeIndex) {
        if let Some(condition) = self.condition(node)
            && matches!(self.tree.node(condition).kind, NodeKind::Group { .. })
        {
            self.finding(
                condition,
                RuleIdentity::ParenthesizedConditions,
                "unnecessary parentheses around condition",
            );
        }
    }

    fn constant_condition(&mut self, node: NodeIndex) {
        if let Some(condition) = self.condition(node) {
            let condition = unwrap(self.tree, condition);

            if truth(self.tree, condition).is_some() {
                self.finding(
                    condition,
                    RuleIdentity::ConstantCondition,
                    "condition has a fixed truth value",
                );
            }
        }
    }

    fn length_condition(&mut self, node: NodeIndex) {
        if let Some(condition) = self.condition(node) {
            let condition = unwrap(self.tree, condition);

            if let NodeKind::Unary { operator, .. } = &self.tree.node(condition).kind
                && self.tree.token(*operator).kind == TokenKind::Symbol(Symbol::Length)
            {
                self.finding(
                    condition,
                    RuleIdentity::LengthAsCondition,
                    "zero is truthy in Luau; compare the length explicitly",
                );
            }
        }
    }

    fn zero_step(&mut self, node: NodeIndex) {
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
    }

    fn empty_loop(&mut self, node: NodeIndex) {
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

    fn empty_conditional(&mut self, node: NodeIndex) {
        let tree = self.tree;

        let NodeKind::If {
            branches,
            otherwise,
            ..
        } = &tree.node(node).kind
        else {
            return;
        };

        for branch in tree.list(branches) {
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

        if let Some(otherwise) = otherwise
            && empty(tree, body(tree, *otherwise))
            && let NodeKind::Else { keyword, .. } = &tree.node(*otherwise).kind
        {
            self.emit(
                tree.token(*keyword).span,
                RuleIdentity::EmptyIf,
                "empty conditional branch",
            );
        }
    }

    fn identical_branches(&mut self, node: NodeIndex) {
        let tree = self.tree;

        let NodeKind::If {
            branches,
            otherwise: Some(otherwise),
            ..
        } = &tree.node(node).kind
        else {
            return;
        };

        let bodies = tree
            .list(branches)
            .iter()
            .filter_map(|branch| match &tree.node(branch.node).kind {
                NodeKind::Branch { body, .. } => Some(*body),
                _ => None,
            })
            .collect::<Vec<_>>();

        if let Some(first) = bodies.first()
            && bodies.iter().all(|other| same(tree, *first, *other))
            && same(tree, *first, body(tree, *otherwise))
        {
            self.finding(
                node,
                RuleIdentity::IfSameThenElse,
                "conditional branches have identical bodies",
            );
        }
    }

    fn negated_condition(&mut self, node: NodeIndex) {
        let tree = self.tree;

        let NodeKind::If {
            branches,
            otherwise: Some(_),
            ..
        } = &tree.node(node).kind
        else {
            return;
        };

        if let [branch] = tree.list(branches)
            && let NodeKind::Branch { condition, .. } = &tree.node(branch.node).kind
            && let NodeKind::Unary { operator, .. } = &tree.node(unwrap(tree, *condition)).kind
            && tree.token(*operator).kind == TokenKind::Keyword(Keyword::Not)
        {
            self.finding(
                *condition,
                RuleIdentity::NegatedCondition,
                "negated condition can be expressed by swapping branches",
            );
        }
    }

    fn conditional_assignment(&mut self, node: NodeIndex) {
        let tree = self.tree;

        let NodeKind::If {
            branches,
            otherwise: Some(otherwise),
            ..
        } = &tree.node(node).kind
        else {
            return;
        };

        if let [branch] = tree.list(branches)
            && let NodeKind::Branch { body: first, .. } = &tree.node(branch.node).kind
            && let (Some(left), Some(right)) =
                (single(tree, *first), single(tree, body(tree, *otherwise)))
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

    fn collapsible_condition(&mut self, node: NodeIndex) {
        let tree = self.tree;

        let NodeKind::If {
            branches,
            otherwise: None,
            ..
        } = &tree.node(node).kind
        else {
            return;
        };

        if let [branch] = tree.list(branches)
            && let NodeKind::Branch { body, .. } = &tree.node(branch.node).kind
            && let Some(inner) = single(tree, *body)
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

    fn else_branch(&mut self, node: NodeIndex, ancestors: &[NodeIndex]) {
        if !matches!(self.tree.node(node).kind, NodeKind::Else { .. }) {
            return;
        }

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

    fn condition(&self, node: NodeIndex) -> Option<NodeIndex> {
        match &self.tree.node(node).kind {
            NodeKind::Branch { condition, .. }
            | NodeKind::While { condition, .. }
            | NodeKind::Repeat { condition, .. }
            | NodeKind::Conditional { condition, .. } => Some(*condition),

            _ => None,
        }
    }
}

pub(crate) fn evaluate(
    input: &RuleInput<'_, '_>,
    settings: &RuleSettings,
    options: &Options,
) -> RuleResult {
    let mut context = Context::new(input, settings);

    let check: fn(&mut Context<'_, '_>, NodeIndex, &[NodeIndex]) = match context.rule {
        RuleIdentity::ConstantCondition => |context, node, _| {
            context.require_number(node);
            context.constant_condition(node);
        },

        RuleIdentity::ZeroStepLoop => |context, node, _| {
            context.require_number(node);
            context.zero_step(node);
        },

        RuleIdentity::LengthAsCondition => |context, node, _| context.length_condition(node),

        RuleIdentity::ParenthesizedConditions => {
            |context, node, _| context.parenthesized_condition(node)
        }

        RuleIdentity::EmptyLoop => |context, node, _| context.empty_loop(node),
        RuleIdentity::EmptyIf => |context, node, _| context.empty_conditional(node),
        RuleIdentity::IfSameThenElse => |context, node, _| context.identical_branches(node),
        RuleIdentity::NegatedCondition => |context, node, _| context.negated_condition(node),

        RuleIdentity::IfExpressionAssignment => {
            |context, node, _| context.conditional_assignment(node)
        }

        RuleIdentity::CollapsibleIf => |context, node, _| context.collapsible_condition(node),

        RuleIdentity::ElseAfterReturn => {
            |context, node, ancestors| context.else_branch(node, ancestors)
        }

        _ => unreachable!("control producer received another rule"),
    };

    context.walk(options, check);

    context.finish(options)
}
