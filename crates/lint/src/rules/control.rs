use vermis::{
    token::{Keyword, TokenKind},
    tree::{NodeIndex, NodeKind},
};

use super::{
    Context,
    syntax::{
        arguments_values, assignment, body, empty, empty_table, in_loop, number, same, single,
        unwrap,
    },
};

use crate::Rule;

impl Context<'_, '_> {
    pub(super) fn loops(&mut self, node: NodeIndex) {
        if let NodeKind::NumericFor {
            step: Some(step), ..
        } = &self.tree.node(node).kind
            && number(self.tree, *step) == Some(0.0)
        {
            self.finding(*step, Rule::ZeroStepLoop, "numeric loop step is zero");
        }

        match &self.tree.node(node).kind {
            NodeKind::While { body, .. }
            | NodeKind::Repeat { body, .. }
            | NodeKind::NumericFor { body, .. }
            | NodeKind::GenericFor { body, .. }
                if empty(self.tree, *body) =>
            {
                self.finding(*body, Rule::EmptyLoop, "empty loop body");
            }

            _ => {}
        }
    }

    pub(super) fn conditional(
        &mut self,
        node: NodeIndex,
        branches: &vermis::tree::NodeList,
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

        for body in &bodies {
            if empty(tree, *body) {
                self.finding(*body, Rule::EmptyIf, "empty conditional branch");
            }
        }

        if let Some(otherwise) = otherwise {
            let body = body(tree, otherwise);

            if empty(tree, body) {
                self.finding(body, Rule::EmptyIf, "empty conditional branch");
            }

            if let Some(first) = bodies.first()
                && bodies.iter().all(|other| same(tree, *first, *other))
                && same(tree, *first, body)
            {
                self.finding(
                    node,
                    Rule::IfSameThenElse,
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
                        Rule::NegatedCondition,
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
                        Rule::IfExpressionAssignment,
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
                Rule::CollapsibleIf,
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
                Rule::ElseAfterReturn,
                "else follows branches that return",
            );
        }
    }

    pub(super) fn iteration(&mut self, node: NodeIndex, ancestors: &[NodeIndex]) {
        let tree = self.tree;

        if let NodeKind::MethodCall {
            receiver,
            method,
            arguments,
            ..
        } = &tree.node(node).kind
            && self.global(*receiver, b"game")
            && tree.text(*method) == b"GetService"
            && arguments_values(tree, *arguments).len() == 1
            && matches!(
                tree.node(arguments_values(tree, *arguments)[0]).kind,
                NodeKind::String { .. }
            )
            && in_loop(tree, node, ancestors)
        {
            self.finding(
                node,
                Rule::LoopInvariantCall,
                "GetService call repeated inside a loop",
            );
        }

        let NodeKind::GenericFor {
            bindings,
            values,
            body,
            ..
        } = &tree.node(node).kind
        else {
            return;
        };

        let bindings = tree.list(bindings);
        let values = tree.list(values);

        if bindings.len() != 2 || values.len() != 1 {
            return;
        }

        let (NodeKind::Binding { name: key, .. }, NodeKind::Binding { name: value, .. }) = (
            &tree.node(bindings[0].node).kind,
            &tree.node(bindings[1].node).kind,
        ) else {
            return;
        };

        let NodeKind::Call { callee, arguments } = &tree.node(values[0].node).kind else {
            return;
        };

        if !self.global(*callee, b"pairs") || arguments_values(tree, *arguments).len() != 1 {
            return;
        }

        let Some(statement) = single(tree, *body) else {
            return;
        };

        let Some((target, assigned)) = assignment(tree, statement) else {
            return;
        };

        let NodeKind::Index {
            receiver,
            key: assigned_key,
            ..
        } = &tree.node(target).kind
        else {
            return;
        };

        let Some(destination) = self.bindings.declaration(*receiver) else {
            return;
        };

        if destination.assigned
            || !destination
                .value
                .is_some_and(|value| empty_table(tree, value))
        {
            return;
        }

        if matches!(tree.node(assigned).kind, NodeKind::Name { .. })
            && same(tree, assigned, *value)
            && same(tree, *assigned_key, *key)
            && self.bindings.references.get(&assigned.get()) == Some(&value.get())
            && self.bindings.references.get(&assigned_key.get()) == Some(&key.get())
        {
            self.finding(
                node,
                Rule::ManualTableClone,
                "table-copy loop can use table.clone",
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
        _ => {}
    }

    context.loops(node);
    context.iteration(node, ancestors);
}
