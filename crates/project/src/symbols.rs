//! Hierarchical source declarations independent of semantic environments.

use std::io;

use instar_shared::Options;

use instar_syntax::{
    literal,
    tree::{NodeIndex, NodeKind, NodeList, Tree},
};

/// Syntactic declaration category.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SymbolKind {
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
    pub kind: SymbolKind,

    /// Complete declaration byte range.
    pub range: [usize; 2],

    /// Name byte range within the declaration.
    pub selection: [usize; 2],

    /// Nested declarations in source order.
    pub children: Vec<Self>,
}

pub(crate) fn symbols(
    source: &instar_shared::SourceSnapshot,
    settings: &instar_syntax::SyntaxSettings,
    options: &Options,
) -> io::Result<Vec<Symbol>> {
    options.check()?;
    let syntax = instar_syntax::parse(source, settings, options);

    collect(syntax.tree.syntax(), syntax.tree.syntax().root, options)
}

fn collect(tree: &Tree<'_>, node: NodeIndex, options: &Options) -> io::Result<Vec<Symbol>> {
    options.check()?;
    let mut symbols = Vec::new();

    match &tree.node(node).kind {
        NodeKind::Local {
            bindings, values, ..
        }
        | NodeKind::Constant {
            bindings, values, ..
        } => locals(tree, node, bindings, values, &mut symbols, options)?,

        NodeKind::Function { name, body, .. } => {
            let children = body
                .map(|body| collect(tree, body, options))
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
                    SymbolKind::Method
                } else {
                    SymbolKind::Function
                };

                append(tree, node, *name, kind, children, &mut symbols);
            } else {
                symbols.extend(children);
            }
        }

        NodeKind::TypeAlias {
            name, annotation, ..
        } => {
            let children = collect(tree, *annotation, options)?;
            append(tree, node, *name, SymbolKind::Type, children, &mut symbols);
        }

        NodeKind::Class { name, members, .. } => {
            let mut children = Vec::new();

            for member in tree.list(members) {
                children.extend(collect(tree, member.node, options)?);
            }

            for child in &mut children {
                child.kind = match child.kind {
                    SymbolKind::Function => SymbolKind::Method,
                    SymbolKind::Variable => SymbolKind::Field,
                    kind => kind,
                };
            }

            append(tree, node, *name, SymbolKind::Class, children, &mut symbols);
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
            let children = collect(tree, *value, options)?;

            append(
                tree,
                node,
                *key,
                value_kind(tree, *value, SymbolKind::Field),
                children,
                &mut symbols,
            );
        }

        NodeKind::TypeField {
            key, annotation, ..
        } => {
            let children = collect(tree, *annotation, options)?;
            append(tree, node, *key, SymbolKind::Field, children, &mut symbols);
        }

        NodeKind::Binding { name, .. } => {
            append(
                tree,
                node,
                *name,
                SymbolKind::Variable,
                Vec::new(),
                &mut symbols,
            );
        }

        _ => {
            for child in tree.children(node) {
                symbols.extend(collect(tree, child, options)?);
            }
        }
    }

    options.check()?;

    Ok(symbols)
}

fn locals(
    tree: &Tree<'_>,
    node: NodeIndex,
    bindings: &NodeList,
    values: &NodeList,
    symbols: &mut Vec<Symbol>,
    options: &Options,
) -> io::Result<()> {
    for (index, binding) in tree.list(bindings).iter().enumerate() {
        options.check()?;

        let name = match tree.node(binding.node).kind {
            NodeKind::Binding { name, .. } => name,
            _ => binding.node,
        };

        let value = tree.list(values).get(index).map(|value| value.node);

        let kind = if matches!(tree.node(node).kind, NodeKind::Constant { .. }) {
            SymbolKind::Constant
        } else {
            SymbolKind::Variable
        };

        let kind = value.map_or(kind, |value| value_kind(tree, value, kind));

        let children = value
            .map(|value| collect(tree, value, options))
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
    kind: SymbolKind,
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

fn value_kind(tree: &Tree<'_>, node: NodeIndex, fallback: SymbolKind) -> SymbolKind {
    match tree.node(node).kind {
        NodeKind::Function { .. } => SymbolKind::Function,
        NodeKind::Table { .. } if fallback == SymbolKind::Variable => SymbolKind::Object,
        _ => fallback,
    }
}
