use std::{collections::BTreeSet, str::from_utf8};

use vermis::{
    emitter::{Emitter, LineEnding},
    token::{Keyword, Symbol, TokenKind},
    tree::{NodeIndex, NodeKind, TokenIndex, Tree},
};

use crate::syntax::{self, is_identifier, literal::quoted};

use super::{
    Error,
    mapping::{invalid, is_primitive},
};

#[derive(Default)]
pub(super) struct Output {
    atoms: Vec<Atom>,
}

enum Atom {
    Keyword(Keyword),
    Symbol(Symbol),
    Source(String),
    Newline,
}

pub(super) struct Block {
    pub(super) name: String,
    pub(super) is_type: bool,
    pub(super) dependencies: Vec<String>,
    pub(super) source: String,
}

pub(super) struct Replacement {
    pub(super) name: String,
    pub(super) source: String,
}

impl Output {
    pub(super) fn keyword(&mut self, keyword: Keyword) {
        self.atoms.push(Atom::Keyword(keyword));
    }

    pub(super) fn symbol(&mut self, symbol: Symbol) {
        self.atoms.push(Atom::Symbol(symbol));
    }

    pub(super) fn source(&mut self, source: impl Into<String>) {
        self.atoms.push(Atom::Source(source.into()));
    }

    pub(super) fn property(&mut self, name: &str) {
        if is_identifier(name) {
            self.source(name);
        } else {
            self.symbol(Symbol::LeftBracket);
            self.source(quoted(name));
            self.symbol(Symbol::RightBracket);
        }
    }

    pub(super) fn newline(&mut self) {
        self.atoms.push(Atom::Newline);
    }

    pub(super) fn append(&mut self, output: Self) {
        self.atoms.extend(output.atoms);
    }

    pub(super) fn finish(&self, is_cancelled: &dyn Fn() -> bool) -> Result<String, Error> {
        let mut emitter = Emitter::new(LineEnding::LineFeed);

        for atom in &self.atoms {
            if is_cancelled() {
                return Err(Error::Cancelled);
            }

            match atom {
                Atom::Keyword(keyword) => emitter.keyword(*keyword),
                Atom::Symbol(symbol) => emitter.symbol(*symbol),
                Atom::Newline => emitter.newline(),

                Atom::Source(source) => {
                    let tree = vermis::parse(source.as_bytes());

                    for (index, token) in tree.tokens.iter().enumerate() {
                        if is_cancelled() {
                            return Err(Error::Cancelled);
                        }

                        match token.kind {
                            TokenKind::Keyword(keyword) => emitter.keyword(keyword),
                            TokenKind::Symbol(symbol) => emitter.symbol(symbol),
                            _ => emitter.token(&tree, TokenIndex::new(index)),
                        }
                    }
                }
            }
        }

        String::from_utf8(emitter.finish().into_owned())
            .map_err(|error| invalid(format!("Emitted declarations are not valid UTF-8. {error}")))
    }
}

impl Block {
    pub(super) fn key(&self) -> (bool, &str) {
        (self.is_type, &self.name)
    }
}

pub(super) fn parse(source: &str) -> Result<Tree<'_>, Error> {
    syntax::parse(source.as_bytes(), |range, message| match message {
        Some(message) => invalid(format!(
            "Invalid declaration at byte {}. {message}",
            range.start
        )),

        None => invalid("Incomplete declaration syntax."),
    })
}

pub(super) fn blocks(source: &str, is_cancelled: &dyn Fn() -> bool) -> Result<Vec<Block>, Error> {
    let tree = parse(source)?;
    let mut result = Vec::new();

    for statement in statements(&tree)? {
        if is_cancelled() {
            return Err(Error::Cancelled);
        }

        let declaration = declaration(&tree, statement);

        let (name, is_type, generics, extends) = match &tree.node(declaration).kind {
            NodeKind::TypeAlias { name, generics, .. } => (*name, true, *generics, None),
            NodeKind::Class { name, extends, .. } => (*name, true, None, *extends),

            NodeKind::Function {
                name: Some(name),
                generics,
                ..
            } => (*name, false, *generics, None),

            NodeKind::Binding { name, .. } => (*name, false, None, None),
            _ => return Err(invalid("Unsupported top-level declaration.")),
        };

        let name = text(&tree, name)?.to_owned();
        let mut dependencies = Vec::new();

        if let Some(generics) = generics
            && let NodeKind::Generics { parameters, .. } = &tree.node(generics).kind
        {
            for parameter in tree.list(parameters) {
                if let NodeKind::Generic {
                    default: Some(default),
                    ..
                } = tree.node(parameter.node).kind
                {
                    let default = text(&tree, default)?;

                    if is_identifier(default) && !is_primitive(default) {
                        dependencies.push(default.to_owned());
                    }
                }
            }
        }

        if let Some(extends) = extends
            && let NodeKind::Extends { superclass, .. } = tree.node(extends).kind
        {
            dependencies.push(text(&tree, superclass)?.to_owned());
        }

        result.push(Block {
            name,
            is_type,
            dependencies,
            source: text(&tree, statement)?.to_owned(),
        });
    }

    Ok(result)
}

pub(super) fn replacements(
    source: &str,
    is_cancelled: &dyn Fn() -> bool,
) -> Result<Vec<(String, Vec<Replacement>)>, Error> {
    let tree = parse(source)?;
    let mut result = Vec::new();

    for (name, members) in tree.nodes.iter().filter_map(|node| match &node.kind {
        NodeKind::Class { name, members, .. } => Some((name, members)),
        _ => None,
    }) {
        if is_cancelled() {
            return Err(Error::Cancelled);
        }

        let mut signatures = Vec::new();

        for member in tree.list(members) {
            let name = match tree.node(member.node).kind {
                NodeKind::Function {
                    name: Some(name), ..
                } => name,

                NodeKind::TypeField { key, .. } => key,
                _ => continue,
            };

            signatures.push(Replacement {
                name: text(&tree, name)?.to_owned(),
                source: text(&tree, member.node)?.to_owned(),
            });
        }

        result.push((text(&tree, *name)?.to_owned(), signatures));
    }

    Ok(result)
}

pub(super) fn ordered(source: &str, is_cancelled: &dyn Fn() -> bool) -> Result<String, Error> {
    let mut pending = blocks(source, is_cancelled)?;
    let mut emitted = BTreeSet::new();
    let mut output = Output::default();

    while !pending.is_empty() {
        let count = pending.len();
        let mut remaining = Vec::new();

        for block in pending {
            if is_cancelled() {
                return Err(Error::Cancelled);
            }

            if block
                .dependencies
                .iter()
                .any(|name| !emitted.contains(name))
            {
                remaining.push(block);
            } else {
                output.source(block.source);
                output.newline();
                output.newline();

                if block.is_type {
                    emitted.insert(block.name);
                }
            }
        }

        if remaining.len() == count {
            return Err(invalid(format!(
                "Unresolved declaration dependencies: {}.",
                remaining
                    .iter()
                    .map(|block| format!("`{}`", block.name))
                    .collect::<Vec<_>>()
                    .join(", ")
            )));
        }

        pending = remaining;
    }

    output.finish(is_cancelled)
}

pub(super) fn injected(
    source: &str,
    exclusions: &[String],
    globals: &[String],
    exports: &[String],
    declarations: &[Block],
    is_cancelled: &dyn Fn() -> bool,
) -> Result<String, Error> {
    let mut sections = String::new();
    let mut remaining = source;

    while let Some((_, section)) = remaining.split_once("-- SECTION BEGIN: ") {
        if is_cancelled() {
            return Err(Error::Cancelled);
        }

        let Some((name, body)) = section.split_once('\n') else {
            break;
        };

        let Some((body, rest)) = body.split_once("-- SECTION END: ") else {
            break;
        };

        remaining = rest;
        let name = name.trim_end_matches('\r');

        if !name.is_empty() && !exclusions.iter().any(|excluded| excluded == name) {
            sections.push_str(body);
            sections.push('\n');
        }
    }

    let mut output = Output::default();

    for block in blocks(&sections, is_cancelled)? {
        if declarations
            .iter()
            .any(|declaration| declaration.key() == block.key())
            || (!block.is_type && globals.contains(&block.name))
        {
            continue;
        }

        if exports.contains(&block.name) && block.source.starts_with("type ") {
            output.source("export");
        }

        output.source(block.source);
        output.newline();
        output.newline();
    }

    output.finish(is_cancelled)
}

fn statements(tree: &Tree<'_>) -> Result<Vec<NodeIndex>, Error> {
    let NodeKind::Root { block, .. } = tree.node(tree.root).kind else {
        return Err(invalid("Missing declaration root."));
    };

    let NodeKind::Block { statements } = &tree.node(block).kind else {
        return Err(invalid("Missing declaration block."));
    };

    Ok(tree
        .list(statements)
        .iter()
        .map(|entry| entry.node)
        .collect())
}

fn declaration(tree: &Tree<'_>, mut index: NodeIndex) -> NodeIndex {
    while let NodeKind::Export { declaration, .. } | NodeKind::Declaration { declaration, .. } =
        tree.node(index).kind
    {
        index = declaration;
    }

    index
}

fn text<'source>(tree: &Tree<'source>, index: NodeIndex) -> Result<&'source str, Error> {
    from_utf8(tree.text(index))
        .map_err(|error| invalid(format!("Declaration text is not valid UTF-8. {error}")))
}
