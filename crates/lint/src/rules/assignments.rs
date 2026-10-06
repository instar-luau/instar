use vermis::token::{Symbol, TokenKind};
use vermis::tree::{NodeIndex, NodeKind};

use super::Context;
use super::syntax::{assignment, in_loop, same, stable};
use crate::Rule;

pub(super) fn check(context: &mut Context<'_, '_>, node: NodeIndex, ancestors: &[NodeIndex]) {
    match &context.tree.node(node).kind {
        NodeKind::Block { .. } => context.swapped(node),

        NodeKind::Assignment { .. } | NodeKind::CompoundAssignment { .. } => {
            context.assignments(node, ancestors);
        }

        _ => {}
    }
}

impl Context<'_, '_> {
    pub(super) fn swapped(&mut self, node: NodeIndex) {
        let tree = self.tree;

        let NodeKind::Block { statements } = &tree.node(node).kind else {
            return;
        };

        for pair in tree.list(statements).windows(2) {
            if let (Some((left, right)), Some((other_left, other_right))) = (
                assignment(tree, pair[0].node),
                assignment(tree, pair[1].node),
            ) && matches!(tree.node(left).kind, NodeKind::Name { .. })
                && matches!(tree.node(right).kind, NodeKind::Name { .. })
                && same(tree, left, other_right)
                && same(tree, right, other_left)
                && !same(tree, left, right)
            {
                self.finding(
                    pair[1].node,
                    Rule::AlmostSwapped,
                    "sequential assignments overwrite a value before swapping it",
                );
            }
        }
    }

    pub(super) fn assignments(&mut self, node: NodeIndex, ancestors: &[NodeIndex]) {
        let tree = self.tree;

        match &tree.node(node).kind {
            NodeKind::Assignment {
                targets, values, ..
            } => {
                for (target, value) in tree.list(targets).iter().zip(tree.list(values)) {
                    if stable(tree, target.node) && same(tree, target.node, value.node) {
                        self.finding(
                            target.node,
                            Rule::SelfAssignment,
                            "assignment leaves a value unchanged",
                        );
                    }
                }

                for target in tree.list(targets) {
                    self.unscoped(target.node);
                }
            }

            NodeKind::CompoundAssignment {
                target, operator, ..
            } => {
                self.unscoped(*target);

                if tree.token(*operator).kind == TokenKind::Symbol(Symbol::ConcatenateAssignment)
                    && in_loop(tree, node, ancestors)
                {
                    self.finding(
                        node,
                        Rule::StringConcatInLoop,
                        "repeated concatenation grows a string quadratically",
                    );
                }
            }

            _ => {}
        }
    }
}
