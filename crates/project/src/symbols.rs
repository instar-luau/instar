//! Hierarchical source declarations independent of semantic environments.

use std::{
    io,
    path::{Path, PathBuf},
    time::Instant,
};

use instar_analysis::{Location, Options};
use instar_syntax::literal;
use vermis::tree::{NodeIndex, NodeKind, NodeList, Tree};

use crate::project::Project;

/// Syntactic declaration category.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Mutable binding.
    Variable,

    /// Constant binding.
    Constant,

    /// Named function.
    Function,

    /// Method declaration.
    Method,

    /// Table binding.
    Object,

    /// Type alias.
    Type,

    /// Class declaration.
    Class,

    /// Named member.
    Field,
}

/// One declaration and its syntactically contained declarations.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Symbol {
    /// Display name.
    pub name: String,

    /// Declaration category.
    pub kind: Kind,

    /// Complete declaration byte range.
    pub range: [usize; 2],

    /// Name byte range within the declaration.
    pub selection: [usize; 2],

    /// Nested declarations in source order.
    pub children: Vec<Self>,
}

/// A named declaration found within the editor indexing selection.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Match {
    /// Revision-anchored declaration name in its source file.
    pub location: Location<PathBuf>,

    /// Declaration display name.
    pub name: String,

    /// Declaration category.
    pub kind: Kind,

    /// Immediately containing declaration when present.
    pub container: Option<String>,
}

impl Project {
    /// Returns hierarchical declarations without preparing semantic environments.
    ///
    /// # Errors
    /// Returns source access or interruption failures.
    pub fn symbols(&mut self, path: &Path, options: &Options) -> io::Result<Vec<Symbol>> {
        let started = Instant::now();

        self.during(
            |project| {
                options.check(started)?;
                let source = project.source(path)?;
                let tree = vermis::parse(source.text.as_bytes());

                collect(&tree, tree.root, options, started)
            },
            options,
        )
    }

    /// Finds declarations by case-insensitive substring across selected sources.
    ///
    /// # Errors
    /// Returns indexing, configuration, source access, or interruption failures.
    pub fn search_symbols(
        &mut self,
        roots: &[PathBuf],
        query: &str,
        options: &Options,
    ) -> io::Result<Vec<Match>> {
        let started = Instant::now();

        self.during(
            |project| {
                let query = query.to_lowercase();
                let mut matches = Vec::new();

                for path in project.index_sources(roots, &options.remaining(started))? {
                    options.check(started)?;
                    let source = project.source(&path)?;
                    let symbols = project.symbols(&path, &options.remaining(started))?;

                    let mut pending = symbols
                        .into_iter()
                        .rev()
                        .map(|symbol| (symbol, None))
                        .collect::<Vec<_>>();

                    while let Some((symbol, container)) = pending.pop() {
                        options.check(started)?;

                        pending.extend(
                            symbol
                                .children
                                .into_iter()
                                .rev()
                                .map(|child| (child, Some(symbol.name.clone()))),
                        );

                        if symbol.name.to_lowercase().contains(&query) {
                            matches.push(Match {
                                location: Location {
                                    module: path.clone(),
                                    revision: source.revision,
                                    range: symbol.selection,
                                },
                                name: symbol.name,
                                kind: symbol.kind,
                                container,
                            });
                        }
                    }
                }

                options.check(started)?;

                Ok(matches)
            },
            options,
        )
    }
}

fn collect(
    tree: &Tree<'_>,
    node: NodeIndex,
    options: &Options,
    started: Instant,
) -> io::Result<Vec<Symbol>> {
    options.check(started)?;
    let mut symbols = Vec::new();

    match &tree.node(node).kind {
        NodeKind::Local {
            bindings, values, ..
        }
        | NodeKind::Constant {
            bindings, values, ..
        } => locals(tree, node, bindings, values, &mut symbols, options, started)?,

        NodeKind::Function { name, body, .. } => {
            let children = body
                .map(|body| collect(tree, body, options, started))
                .transpose()?
                .unwrap_or_default();

            if let Some(name) = name {
                let kind = if matches!(
                    tree.node(*name).kind,
                    NodeKind::FunctionName {
                        method: Some(_),
                        ..
                    }
                ) {
                    Kind::Method
                } else {
                    Kind::Function
                };

                append(tree, node, *name, kind, children, &mut symbols);
            } else {
                symbols.extend(children);
            }
        }

        NodeKind::TypeAlias {
            name, annotation, ..
        } => {
            let children = collect(tree, *annotation, options, started)?;
            append(tree, node, *name, Kind::Type, children, &mut symbols);
        }

        NodeKind::Class { name, members, .. } => {
            let mut children = Vec::new();

            for member in tree.list(members) {
                children.extend(collect(tree, member.node, options, started)?);
            }

            for child in &mut children {
                child.kind = match child.kind {
                    Kind::Function => Kind::Method,
                    Kind::Variable => Kind::Field,
                    kind => kind,
                };
            }

            append(tree, node, *name, Kind::Class, children, &mut symbols);
        }

        NodeKind::TableField {
            key: Some(key),
            value,
            ..
        } if matches!(
            tree.node(*key).kind,
            NodeKind::Name { .. } | NodeKind::String { .. } | NodeKind::Number { .. }
        ) =>
        {
            let children = collect(tree, *value, options, started)?;

            append(
                tree,
                node,
                *key,
                value_kind(tree, *value, Kind::Field),
                children,
                &mut symbols,
            );
        }

        NodeKind::TypeField {
            key, annotation, ..
        } => {
            let children = collect(tree, *annotation, options, started)?;
            append(tree, node, *key, Kind::Field, children, &mut symbols);
        }

        NodeKind::Binding { name, .. } => {
            append(tree, node, *name, Kind::Variable, Vec::new(), &mut symbols);
        }

        _ => {
            for child in tree.children(node) {
                symbols.extend(collect(tree, child, options, started)?);
            }
        }
    }

    options.check(started)?;

    Ok(symbols)
}

fn locals(
    tree: &Tree<'_>,
    node: NodeIndex,
    bindings: &NodeList,
    values: &NodeList,
    symbols: &mut Vec<Symbol>,
    options: &Options,
    started: Instant,
) -> io::Result<()> {
    for (index, binding) in tree.list(bindings).iter().enumerate() {
        options.check(started)?;

        let name = match tree.node(binding.node).kind {
            NodeKind::Binding { name, .. } => name,
            _ => binding.node,
        };

        let value = tree.list(values).get(index).map(|value| value.node);

        let kind = if matches!(tree.node(node).kind, NodeKind::Constant { .. }) {
            Kind::Constant
        } else {
            Kind::Variable
        };

        let kind = value.map_or(kind, |value| value_kind(tree, value, kind));

        let children = value
            .map(|value| collect(tree, value, options, started))
            .transpose()?
            .unwrap_or_default();

        append(tree, node, name, kind, children, symbols);
    }

    Ok(())
}

fn append(
    tree: &Tree<'_>,
    node: NodeIndex,
    name: NodeIndex,
    kind: Kind,
    children: Vec<Symbol>,
    symbols: &mut Vec<Symbol>,
) {
    let selection = tree.node(name).span;

    if selection.start == selection.end {
        return;
    }

    let range = tree.node(node).span;

    let name = literal::string(tree, name)
        .unwrap_or_else(|| String::from_utf8_lossy(tree.text(name)).into_owned());

    symbols.push(Symbol {
        name,
        kind,
        range: [range.start, range.end],
        selection: [selection.start, selection.end],
        children,
    });
}

fn value_kind(tree: &Tree<'_>, node: NodeIndex, fallback: Kind) -> Kind {
    match tree.node(node).kind {
        NodeKind::Function { .. } => Kind::Function,
        NodeKind::Table { .. } if fallback == Kind::Variable => Kind::Object,
        _ => fallback,
    }
}
