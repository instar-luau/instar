use instar_shared::Options;
use instar_syntax::tree::{NodeIndex, NodeKind};

use super::expressions::{assignment, same};
use crate::{RuleIdentity, RuleInput, RuleResult, RuleSettings, request::Context};

impl Context<'_, '_> {
    fn swapped(&mut self, node: NodeIndex) {
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
                && matches!(tree.node(other_left).kind, NodeKind::Name { .. })
                && matches!(tree.node(other_right).kind, NodeKind::Name { .. })
                && same(tree, left, other_right)
                && same(tree, right, other_left)
                && !same(tree, left, right)
            {
                self.finding(
                    pair[1].node,
                    RuleIdentity::AlmostSwapped,
                    "sequential assignments overwrite a value before swapping it",
                );
            }
        }
    }

    fn assignments(&mut self, node: NodeIndex) {
        let tree = self.tree;

        let NodeKind::Assignment {
            targets, values, ..
        } = &tree.node(node).kind
        else {
            return;
        };

        for (target, value) in tree.list(targets).iter().zip(tree.list(values)) {
            if matches!(tree.node(target.node).kind, NodeKind::Name { .. })
                && matches!(tree.node(value.node).kind, NodeKind::Name { .. })
                && same(tree, target.node, value.node)
            {
                self.finding(
                    target.node,
                    RuleIdentity::SelfAssignment,
                    "assignment leaves a value unchanged",
                );
            }
        }
    }
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

fn check(context: &mut Context<'_, '_>, node: NodeIndex, _: &[NodeIndex]) {
    match &context.tree.node(node).kind {
        NodeKind::Block { .. } => context.swapped(node),
        NodeKind::Assignment { .. } => context.assignments(node),
        _ => {}
    }
}
