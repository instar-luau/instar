use std::{cmp::Ordering, collections::BTreeSet, ops::Range};

use vermis::{
    token::TokenKind,
    tree::{NodeIndex, NodeKind, Tree},
};

use crate::{
    format::{Prepared, directives::Directives},
    settings::format::{BlankLines, Order},
    syntax::{bindings::Bindings, literal},
};

#[derive(Clone, Copy, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) enum Kind {
    Service,
    Module,
    Member,
    Type,
}

#[derive(Clone)]
pub(crate) struct Import {
    pub(crate) kind: Kind,
    pub(crate) path: String,
    pub(crate) binding: NodeIndex,
    is_boundary: bool,
}

pub(crate) struct Imports {
    pub(super) bindings: Bindings,
}

impl Imports {
    pub(crate) fn new(tree: &Tree<'_>) -> Self {
        Self {
            bindings: Bindings::new(tree),
        }
    }

    pub(crate) fn declaration(
        &self,
        tree: &Tree<'_>,
        index: NodeIndex,
        directives: &Directives,
    ) -> Option<Import> {
        if directives.is_blocked(tree, index) {
            return None;
        }

        if let NodeKind::TypeAlias {
            name, annotation, ..
        } = tree.node(index).kind
        {
            let NodeKind::TypeName {
                namespace: Some(namespace),
                ..
            } = tree.node(annotation).kind
            else {
                return None;
            };

            return Some(Import {
                kind: Kind::Type,
                path: self.origin(tree, namespace)?,
                binding: name,
                is_boundary: directives.boundary(tree, index).is_some(),
            });
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

        if matches!(
            tree.text(*name),
            b"require" | b"script" | b"game" | b"workspace"
        ) {
            return None;
        }

        let (kind, path) = match tree.node(value.node).kind {
            NodeKind::MethodCall {
                receiver,
                method,
                arguments,
                ..
            } if self.bindings.is_global(tree, receiver, "game")
                && tree.text(method) == b"GetService" =>
            {
                (Kind::Service, string(tree, argument(tree, arguments)?)?)
            }

            NodeKind::Call { .. } => (Kind::Module, self.module(tree, value.node)?),
            NodeKind::Field { receiver, .. } => (Kind::Member, self.origin(tree, receiver)?),

            NodeKind::Index { receiver, key, .. } => {
                string(tree, key)?;

                (Kind::Member, self.origin(tree, receiver)?)
            }

            _ => return None,
        };

        Some(Import {
            kind,
            path,
            binding: *name,
            is_boundary: directives.boundary(tree, index).is_some(),
        })
    }

    fn module(&self, tree: &Tree<'_>, node: NodeIndex) -> Option<String> {
        let NodeKind::Call { callee, arguments } = tree.node(node).kind else {
            return None;
        };

        if !self.bindings.is_global(tree, callee, "require") {
            return None;
        }

        let value = argument(tree, arguments)?;

        string(tree, value).or_else(|| self.instance(tree, value))
    }

    fn origin(&self, tree: &Tree<'_>, node: NodeIndex) -> Option<String> {
        let node = self.bindings.value(tree, node);

        match tree.node(node).kind {
            NodeKind::Field { receiver, .. } => self.origin(tree, receiver),

            NodeKind::Index { receiver, key, .. } => {
                string(tree, key)?;

                self.origin(tree, receiver)
            }

            _ => self.module(tree, node),
        }
    }

    fn instance(&self, tree: &Tree<'_>, node: NodeIndex) -> Option<String> {
        let node = self.bindings.value(tree, node);

        match tree.node(node).kind {
            NodeKind::Name { .. } => {
                let name = match tree.text(node) {
                    b"script" => "script",
                    b"game" => "game",
                    b"workspace" => "workspace",
                    _ => return None,
                };

                self.bindings
                    .is_global(tree, node, name)
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
                let name = string(tree, argument(tree, arguments)?)?;

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

pub(crate) fn runs<'import>(
    tree: &Tree<'_>,
    imports: impl IntoIterator<Item = Option<&'import Import>>,
) -> Vec<Range<usize>> {
    let imports: Vec<_> = imports.into_iter().collect();
    let mut result = Vec::new();
    let mut start = 0;

    while start < imports.len() {
        if imports[start].is_none() {
            start += 1;
            continue;
        }

        let mut end = start + 1;

        while end < imports.len() && imports[end].is_some_and(|import| !import.is_boundary) {
            end += 1;
        }

        let mut names = BTreeSet::new();

        if imports[start..end].iter().all(|import| {
            import.is_some_and(|import| {
                names.insert((import.kind == Kind::Type, tree.text(import.binding)))
            })
        }) {
            result.push(start..end);
        }

        start = end;
    }

    result
}

pub(crate) fn compare(configuration: &Prepared, left: &Import, right: &Import) -> Ordering {
    left.kind.cmp(&right.kind).then_with(|| {
        if configuration.settings().require.order == Order::Preserve {
            Ordering::Equal
        } else if configuration.settings().require.order == Order::Grouped
            && left.kind != Kind::Service
        {
            configuration
                .group(&left.path)
                .cmp(&configuration.group(&right.path))
                .then_with(|| left.path.cmp(&right.path))
        } else {
            left.path.cmp(&right.path)
        }
    })
}

pub(crate) fn separated(configuration: &Prepared, left: &Import, right: &Import) -> bool {
    left.kind != right.kind
        || left.kind != Kind::Service
            && configuration.settings().require.order == Order::Grouped
            && configuration.settings().require.blank_lines == BlankLines::BetweenGroups
            && configuration.group(&left.path) != configuration.group(&right.path)
}

fn argument(tree: &Tree<'_>, node: NodeIndex) -> Option<NodeIndex> {
    let NodeKind::Arguments { values, .. } = &tree.node(node).kind else {
        return None;
    };

    let [value] = tree.list(values) else {
        return None;
    };

    Some(value.node)
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
