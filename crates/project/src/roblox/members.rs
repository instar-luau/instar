use std::{
    collections::{BTreeMap, BTreeSet},
    io,
};

use instar_shared::{Completion, Options, SourceSnapshot, error::invalid};
use instar_syntax::{SyntaxSettings, literal, parse, tree::NodeKind};

pub(crate) struct Members(BTreeMap<String, Class>);

struct Class {
    parent: Option<String>,
    names: BTreeSet<String>,
    indexed: bool,
}

impl Members {
    pub(crate) fn parse(source: &SourceSnapshot, options: &Options) -> io::Result<Self> {
        options.check()?;
        let syntax = parse(source, &SyntaxSettings::default(), options);
        options.check()?;

        if syntax.completion != Completion::Complete {
            return Err(invalid("unsupported Roblox declaration syntax"));
        }

        let tree = syntax.tree.syntax();

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
                    literal::string(tree, key)
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

        loop {
            let declaration = self.0.get(class).ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::Unsupported,
                    format!("Roblox member precedence is unavailable for class {class}"),
                )
            })?;

            if !visited.insert(class) {
                return Err(invalid("cyclic Roblox class inheritance"));
            }

            if declaration.indexed || declaration.names.contains(name) {
                return Ok(true);
            }

            let Some(parent) = &declaration.parent else {
                return Ok(false);
            };

            class = parent;
        }
    }
}
