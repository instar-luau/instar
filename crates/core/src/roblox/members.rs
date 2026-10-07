use std::{
    collections::{BTreeMap, BTreeSet},
    io,
};

use instar_analysis::error::invalid;
use instar_syntax::literal;
use vermis::tree::NodeKind;

pub(crate) struct Members(BTreeMap<String, Class>);

struct Class {
    parent: Option<String>,
    names: BTreeSet<String>,
    indexed: bool,
}

impl Members {
    pub(super) fn parse(source: &str) -> io::Result<Self> {
        let tree = vermis::parse(source.as_bytes());

        if let Some(diagnostic) = tree.diagnostics.first() {
            return Err(invalid(format!(
                "invalid Roblox declarations: {}",
                diagnostic.message
            )));
        }

        let mut classes = BTreeMap::new();

        for node in &tree.nodes {
            let NodeKind::Class {
                name,
                extends,
                members,
                ..
            } = &node.kind
            else {
                continue;
            };

            let name = String::from_utf8_lossy(tree.text(*name)).into_owned();

            let parent = extends.and_then(|extends| {
                if let NodeKind::Extends { superclass, .. } = tree.node(extends).kind {
                    Some(String::from_utf8_lossy(tree.text(superclass)).into_owned())
                } else {
                    None
                }
            });

            let mut names = BTreeSet::new();
            let mut indexed = false;

            for member in tree.list(members) {
                let key = match tree.node(member.node).kind {
                    NodeKind::TypeField { key, .. } => key,

                    NodeKind::TypeIndexer { .. } => {
                        indexed = true;
                        continue;
                    }

                    NodeKind::Function {
                        name: Some(name), ..
                    } => name,

                    _ => return Err(invalid("unsupported Roblox declaration member")),
                };

                let name = if matches!(tree.node(key).kind, NodeKind::String { .. }) {
                    literal::string(&tree, key)
                        .ok_or_else(|| invalid("invalid Roblox member name"))?
                } else {
                    String::from_utf8_lossy(tree.text(key)).into_owned()
                };

                names.insert(name);
            }

            if classes
                .insert(
                    name,
                    Class {
                        parent,
                        names,
                        indexed,
                    },
                )
                .is_some()
            {
                return Err(invalid("duplicate Roblox class declaration"));
            }
        }

        Ok(Self(classes))
    }

    pub(crate) fn contains<'inventory>(
        &'inventory self,
        mut class: &'inventory str,
        name: &str,
    ) -> io::Result<bool> {
        let mut visited = BTreeSet::new();

        while let Some(declaration) = self.0.get(class) {
            if !visited.insert(class) {
                return Err(invalid("cyclic Roblox class inheritance"));
            }

            if declaration.indexed || declaration.names.contains(name) {
                return Ok(true);
            }

            let Some(parent) = &declaration.parent else {
                break;
            };

            class = parent;
        }

        Ok(false)
    }
}
