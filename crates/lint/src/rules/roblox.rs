use vermis::tree::{NodeIndex, NodeKind};

use super::Context;
use super::syntax::{arguments_values, number, unwrap};
use crate::Rule;

pub(super) fn check(context: &mut Context<'_, '_>, node: NodeIndex) {
    if !context.source.roblox {
        return;
    }

    let tree = context.tree;

    if let NodeKind::Call { callee, arguments } = &tree.node(node).kind
        && let NodeKind::Field { receiver, name, .. } = &tree.node(*callee).kind
        && tree.text(*name) == b"new"
    {
        let values = arguments_values(tree, *arguments);

        if context.global(*receiver, b"Color3")
            && values.iter().any(|value| {
                number(tree, *value).is_some_and(|number| !(0.0..=1.0).contains(&number))
            })
        {
            context.finding(
                node,
                Rule::RobloxIncorrectColor3NewBounds,
                "Color3.new channels use a 0â€“1 scale",
            );
        }

        if context.global(*receiver, b"UDim2") {
            if values.len() == 2 {
                context.finding(
                    node,
                    Rule::RobloxSuspiciousUdim2New,
                    "UDim2.new needs scale and offset components for both axes",
                );
            }

            if values.len() == 4 {
                let scale =
                    number(tree, values[1]) == Some(0.0) && number(tree, values[3]) == Some(0.0);

                let offset =
                    number(tree, values[0]) == Some(0.0) && number(tree, values[2]) == Some(0.0);

                if scale || offset {
                    context.finding(
                        node,
                        Rule::RobloxManualFromscaleOrFromoffset,
                        if scale {
                            "use UDim2.fromScale when both offsets are zero"
                        } else {
                            "use UDim2.fromOffset when both scales are zero"
                        },
                    );
                }
            }
        }
    }

    if let NodeKind::MethodCall {
        receiver, method, ..
    } = &tree.node(node).kind
        && tree.text(*method) == b"GetChildren"
        && context.players(*receiver)
    {
        context.finding(
            node,
            Rule::RobloxPreferGetPlayers,
            "use Players:GetPlayers() to select players",
        );
    }
}

impl Context<'_, '_> {
    pub(super) fn players(&self, node: NodeIndex) -> bool {
        let node = unwrap(self.tree, node);

        let value = if matches!(self.tree.node(node).kind, NodeKind::Name { .. }) {
            let Some(binding) = self.bindings.declaration(node) else {
                return false;
            };

            if binding.assigned {
                return false;
            }

            let Some(value) = binding.value else {
                return false;
            };

            value
        } else {
            node
        };

        let NodeKind::MethodCall {
            receiver,
            method,
            arguments,
            ..
        } = &self.tree.node(value).kind
        else {
            return false;
        };

        let values = arguments_values(self.tree, *arguments);

        self.global(*receiver, b"game")
            && self.tree.text(*method) == b"GetService"
            && values.len() == 1
            && matches!(self.tree.node(values[0]).kind, NodeKind::String { .. })
            && instar_syntax::literal::string(self.tree, values[0]).as_deref() == Some("Players")
    }
}
