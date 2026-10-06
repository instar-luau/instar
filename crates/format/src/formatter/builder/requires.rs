use super::{Builder, Require, Statement};
use crate::configuration::{BuiltinGroup, Group, Order};
use std::collections::BTreeSet;
use vermis::tree::{NodeIndex, NodeKind};

impl Builder<'_, '_> {
    pub(super) fn require(&self, index: NodeIndex) -> Option<Require> {
        let NodeKind::Local {
            bindings, values, ..
        } = &self.tree.node(index).kind
        else {
            return None;
        };

        if self.tree.list(bindings).len() != 1 || self.tree.list(values).len() != 1 {
            return None;
        }

        let NodeKind::Binding {
            name,
            annotation: None,
            ..
        } = &self.tree.node(self.tree.list(bindings)[0].node).kind
        else {
            return None;
        };

        if self.tree.text(*name) == b"require" {
            return None;
        }

        let NodeKind::Call { callee, arguments } =
            &self.tree.node(self.tree.list(values)[0].node).kind
        else {
            return None;
        };

        if !self.bindings.global(self.tree, *callee)
            || !matches!(self.tree.node(*callee).kind, NodeKind::Name { .. })
            || self.tree.text(*callee) != b"require"
        {
            return None;
        }

        let NodeKind::Arguments { values, .. } = &self.tree.node(*arguments).kind else {
            return None;
        };

        if self.tree.list(values).len() != 1 {
            return None;
        }

        Some(Require {
            path: instar_syntax::literal::string(self.tree, self.tree.list(values)[0].node)?,
            binding: *name,
        })
    }

    pub(super) fn order(&self, statements: &mut [Statement]) {
        if self.configuration.requires.order == Order::Preserve {
            return;
        }

        let mut start = 0;

        while start < statements.len() {
            if self.interrupted() {
                return;
            }

            if statements[start].require.is_none() {
                start += 1;
                continue;
            }

            let mut end = start + 1;

            while end < statements.len() && statements[end].require.is_some() {
                end += 1;
            }

            let mut bindings = BTreeSet::new();

            if statements[start..end].iter().all(|statement| {
                bindings.insert(
                    self.tree
                        .text(statement.require.as_ref().expect("require run").binding),
                )
            }) {
                statements[start..end].sort_by(|left, right| {
                    let left = &left.require.as_ref().expect("require run").path;
                    let right = &right.require.as_ref().expect("require run").path;

                    if self.configuration.requires.order == Order::Grouped {
                        self.group(left)
                            .cmp(&self.group(right))
                            .then_with(|| left.cmp(right))
                    } else {
                        left.cmp(right)
                    }
                });
            }

            start = end;
        }
    }

    pub(super) fn group(&self, path: &str) -> usize {
        self.configuration
            .requires
            .groups
            .iter()
            .position(|group| match group {
                Group::Builtin(BuiltinGroup::Alias) => path.starts_with('@'),

                Group::Builtin(BuiltinGroup::Relative) => {
                    path.starts_with("./") || path.starts_with("../")
                }

                Group::Builtin(BuiltinGroup::Other) => true,

                Group::Custom(group) => group.patterns.iter().any(|pattern| {
                    glob::Pattern::new(pattern).is_ok_and(|pattern| pattern.matches(path))
                }),
            })
            .unwrap_or(self.configuration.requires.groups.len())
    }
}
