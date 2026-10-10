use instar_shared::Options;

use instar_syntax::{
    literal,
    tree::{NodeIndex, NodeKind},
};

use super::expressions::{arguments_values, number, unwrap};
use crate::{RuleIdentity, RuleInput, RuleResult, RuleSettings, request::Context};

impl Context<'_, '_> {
    fn color_bounds(&mut self, node: NodeIndex) {
        if let Some(values) = self.constructor(node, b"Color3")
            && values.iter().any(|value| {
                number(self.tree, *value).is_some_and(|number| !(0.0..=1.0).contains(&number))
            })
        {
            self.finding(
                node,
                RuleIdentity::RobloxIncorrectColor3NewBounds,
                "Color3.new channels use a 0–1 scale",
            );
        }
    }

    fn manual_dimensions(&mut self, node: NodeIndex) {
        if let Some(values) = self.constructor(node, b"UDim2")
            && values.len() == 4
        {
            let scale = number(self.tree, values[1]) == Some(0.0)
                && number(self.tree, values[3]) == Some(0.0);

            let offset = number(self.tree, values[0]) == Some(0.0)
                && number(self.tree, values[2]) == Some(0.0);

            if scale || offset {
                self.finding(
                    node,
                    RuleIdentity::RobloxManualFromscaleOrFromoffset,
                    if scale {
                        "use UDim2.fromScale when both offsets are zero"
                    } else {
                        "use UDim2.fromOffset when both scales are zero"
                    },
                );
            }
        }
    }

    fn suspicious_dimensions(&mut self, node: NodeIndex) {
        if let Some(values) = self.constructor(node, b"UDim2")
            && values.len() == 2
        {
            self.finding(
                node,
                RuleIdentity::RobloxSuspiciousUdim2New,
                "UDim2.new needs scale and offset components for both axes",
            );
        }
    }

    fn player_enumeration(&mut self, node: NodeIndex) {
        if self.policy.roblox
            && let NodeKind::MethodCall {
                receiver, method, ..
            } = &self.tree.node(node).kind
            && self.tree.text(*method) == b"GetChildren"
            && self.players(*receiver)
        {
            self.finding(
                node,
                RuleIdentity::RobloxPreferGetPlayers,
                "use Players:GetPlayers() to select players",
            );
        }
    }

    fn constructor(&self, node: NodeIndex, name: &[u8]) -> Option<Vec<NodeIndex>> {
        if self.policy.roblox
            && let NodeKind::Call { callee, arguments } = &self.tree.node(node).kind
            && let NodeKind::Field {
                receiver,
                name: member,
                ..
            } = &self.tree.node(*callee).kind
            && self.tree.text(*member) == b"new"
            && self.global(*receiver, name)
        {
            Some(arguments_values(self.tree, *arguments))
        } else {
            None
        }
    }

    fn players(&self, node: NodeIndex) -> bool {
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
            && literal::string(self.tree, values[0]).as_deref() == Some("Players")
    }
}

pub(crate) fn evaluate(
    input: &RuleInput<'_, '_>,
    settings: &RuleSettings,
    options: &Options,
) -> RuleResult {
    let mut context = Context::new(input, settings);

    let check: fn(&mut Context<'_, '_>, NodeIndex, &[NodeIndex]) = match context.rule {
        RuleIdentity::RobloxIncorrectColor3NewBounds => |context, node, _| {
            context.require_number(node);
            context.color_bounds(node);
        },

        RuleIdentity::RobloxManualFromscaleOrFromoffset => |context, node, _| {
            context.require_number(node);
            context.manual_dimensions(node);
        },

        RuleIdentity::RobloxSuspiciousUdim2New => {
            |context, node, _| context.suspicious_dimensions(node)
        }

        RuleIdentity::RobloxPreferGetPlayers => |context, node, _| context.player_enumeration(node),
        _ => unreachable!("Roblox producer received another rule"),
    };

    context.walk(options, check);

    context.finish(options)
}
