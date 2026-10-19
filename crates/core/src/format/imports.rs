use std::{cmp::Ordering, collections::BTreeSet, ops::Range};

use vermis::{
    token::TokenKind,
    tree::{NodeIndex, NodeKind, Tree},
};

use crate::{
    format::{Prepared, directives::Directives},
    settings::format::Order,
    syntax::{bindings::Bindings, literal},
};

#[derive(Clone)]
pub(crate) struct Require {
    pub(crate) path: String,
    pub(crate) binding: NodeIndex,
    boundary: bool,
}

pub(crate) struct Imports {
    bindings: Bindings,
    instances: bool,
}

impl Imports {
    pub(crate) fn new(tree: &Tree<'_>) -> Self {
        Self {
            bindings: Bindings::new(tree),
            instances: false,
        }
    }

    pub(crate) fn instances(tree: &Tree<'_>) -> Self {
        Self {
            bindings: Bindings::new(tree),
            instances: true,
        }
    }

    pub(crate) fn proposed(&self, tree: &Tree<'_>) -> Self {
        if self.instances {
            Self::instances(tree)
        } else {
            Self::new(tree)
        }
    }

    pub(crate) fn require(
        &self,
        tree: &Tree<'_>,
        index: NodeIndex,
        directives: &Directives,
    ) -> Option<Require> {
        if directives.blocks(tree, index) {
            return None;
        }

        let (NodeKind::Local {
            bindings, values, ..
        }
        | NodeKind::Constant {
            bindings, values, ..
        }) = &tree.node(index).kind
        else {
            return None;
        };

        let ([binding], [value]) = (tree.list(bindings), tree.list(values)) else {
            return None;
        };

        let NodeKind::Binding {
            name,
            annotation: None,
            ..
        } = &tree.node(binding.node).kind
        else {
            return None;
        };

        if tree.text(*name) == b"require"
            || self.instances && matches!(tree.text(*name), b"script" | b"game" | b"workspace")
        {
            return None;
        }

        let NodeKind::Call { callee, arguments } = &tree.node(value.node).kind else {
            return None;
        };

        if !self.bindings.global(tree, *callee, "require") {
            return None;
        }

        let NodeKind::Arguments { values, .. } = &tree.node(*arguments).kind else {
            return None;
        };

        let [value] = tree.list(values) else {
            return None;
        };

        let path = string(tree, value.node).or_else(|| self.instance(tree, value.node))?;

        Some(Require {
            path,
            binding: *name,
            boundary: directives.boundary(tree, index).is_some(),
        })
    }

    fn instance(&self, tree: &Tree<'_>, node: NodeIndex) -> Option<String> {
        if !self.instances {
            return None;
        }

        match tree.node(node).kind {
            NodeKind::Name { .. } => {
                let name = match tree.text(node) {
                    b"script" => "script",
                    b"game" => "game",
                    b"workspace" => "workspace",
                    _ => return None,
                };

                self.bindings
                    .global(tree, node, name)
                    .then(|| name.to_owned())
            }

            NodeKind::Field { receiver, name, .. } => Some(format!(
                "{}/{}",
                self.instance(tree, receiver)?,
                String::from_utf8_lossy(tree.text(name))
            )),

            NodeKind::Index { receiver, key, .. } => Some(format!(
                "{}/{}",
                self.instance(tree, receiver)?,
                string(tree, key)?
            )),

            NodeKind::MethodCall {
                receiver,
                method,
                arguments,
                ..
            } => {
                let path = self.instance(tree, receiver)?;

                let NodeKind::Arguments { values, .. } = &tree.node(arguments).kind else {
                    return None;
                };

                let [value] = tree.list(values) else {
                    return None;
                };

                let name = string(tree, value.node)?;

                if tree.text(method) == b"WaitForChild"
                    || path == "game" && tree.text(method) == b"GetService"
                {
                    Some(format!("{path}/{name}"))
                } else {
                    None
                }
            }

            _ => None,
        }
    }
}

pub(crate) fn runs<'require>(
    tree: &Tree<'_>,
    requires: impl IntoIterator<Item = Option<&'require Require>>,
) -> Vec<Range<usize>> {
    let requires: Vec<_> = requires.into_iter().collect();
    let mut result = Vec::new();
    let mut start = 0;

    while start < requires.len() {
        if requires[start].is_none() {
            start += 1;
            continue;
        }

        let mut end = start + 1;

        while end < requires.len() && requires[end].is_some_and(|require| !require.boundary) {
            end += 1;
        }

        let mut names = BTreeSet::new();

        if requires[start..end]
            .iter()
            .all(|require| require.is_some_and(|require| names.insert(tree.text(require.binding))))
        {
            result.push(start..end);
        }

        start = end;
    }

    result
}

pub(crate) fn compare(configuration: &Prepared, left: &str, right: &str) -> Ordering {
    if configuration.settings().require.order == Order::Grouped {
        configuration
            .group(left)
            .cmp(&configuration.group(right))
            .then_with(|| left.cmp(right))
    } else {
        left.cmp(right)
    }
}

fn string(tree: &Tree<'_>, node: NodeIndex) -> Option<String> {
    let NodeKind::String { token } = tree.node(node).kind else {
        return None;
    };

    if !matches!(
        tree.token(token).kind,
        TokenKind::QuotedString | TokenKind::RawString
    ) {
        return None;
    }

    String::from_utf8(literal::decode(tree.text(node))?).ok()
}
