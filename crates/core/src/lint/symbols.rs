use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use vermis::tree::NodeKind;

use crate::lint::{Error, syntax::parse};

/// Value and type names supplied by the selected declaration environment.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Symbols {
    /// Declared global values.
    pub globals: BTreeSet<String>,

    /// Declared type names.
    pub types: BTreeSet<String>,
}

impl Symbols {
    /// Collect names from a declaration source.
    ///
    /// # Errors
    /// Returns malformed declaration syntax.
    pub fn declare(&mut self, source: &str) -> Result<(), Error> {
        let tree = parse(source)?;

        let name = |node| String::from_utf8_lossy(tree.text(node)).into_owned();

        for node in &tree.nodes {
            match &node.kind {
                NodeKind::Declaration { declaration, .. } => {
                    if let NodeKind::Binding { name: binding, .. } = tree.node(*declaration).kind {
                        self.globals.insert(name(binding));
                    }
                }

                NodeKind::Function {
                    name: Some(binding),
                    prefix: Some(prefix),
                    body: None,
                    ..
                } if tree.token(*prefix).bytes(tree.source) == b"declare" => {
                    self.globals.insert(name(*binding));
                }

                NodeKind::Class { name: binding, .. }
                | NodeKind::TypeAlias { name: binding, .. } => {
                    self.types.insert(name(*binding));
                }

                _ => {}
            }
        }

        Ok(())
    }
}
