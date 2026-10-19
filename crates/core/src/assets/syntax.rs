use std::{collections::BTreeSet, str::from_utf8};

use vermis::{
    emitter::{Emitter, LineEnding},
    lexer::Lexer,
    token::{Keyword, Symbol, TokenKind},
    tree::{NodeIndex, NodeKind, TokenIndex, Tree},
};

use crate::syntax;

use super::{
    Error,
    mapping::{invalid, primitive},
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

    pub(super) fn name(&mut self, name: &str) -> Result<(), Error> {
        if !identifier(name) {
            return Err(invalid(format!("invalid declaration identifier `{name}`")));
        }

        self.source(name);

        Ok(())
    }

    pub(super) fn property(&mut self, name: &str) -> Result<(), Error> {
        if identifier(name) {
            self.source(name);
        } else {
            self.symbol(Symbol::LeftBracket);
            self.source(quoted(name)?);
            self.symbol(Symbol::RightBracket);
        }

        Ok(())
    }

    pub(super) fn newline(&mut self) {
        self.atoms.push(Atom::Newline);
    }

    pub(super) fn append(&mut self, output: Self) {
        self.atoms.extend(output.atoms);
    }

    pub(super) fn finish(&self, cancelled: fn() -> bool) -> Result<String, Error> {
        let mut emitter = Emitter::new(LineEnding::LineFeed);

        for atom in &self.atoms {
            if cancelled() {
                return Err(Error::Cancelled);
            }

            match atom {
                Atom::Keyword(keyword) => emitter.keyword(*keyword),
                Atom::Symbol(symbol) => emitter.symbol(*symbol),
                Atom::Newline => emitter.newline(),

                Atom::Source(source) => {
                    let tree = vermis::parse(source.as_bytes());

                    for (index, token) in tree.tokens.iter().enumerate() {
                        if cancelled() {
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
            .map_err(|error| invalid(format!("invalid emitted UTF-8: {error}")))
    }
}

impl Block {
    pub(super) fn key(&self) -> (bool, &str) {
        (self.is_type, &self.name)
    }
}

pub(super) fn identifier(name: &str) -> bool {
    let mut tokens = Lexer::new(name.as_bytes());

    matches!(tokens.next(), Some(token) if token.kind == TokenKind::Name && token.span.start == 0 && token.span.end == name.len())
        && matches!(tokens.next(), Some(token) if token.kind == TokenKind::EndOfFile)
}

pub(super) fn parse(source: &str) -> Result<Tree<'_>, Error> {
    syntax::parse(source.as_bytes(), |range, message| match message {
        Some(message) => invalid(format!(
            "invalid declaration at byte {}: {message}",
            range.start
        )),

        None => invalid("incomplete declaration syntax"),
    })
}

pub(super) fn blocks(source: &str, cancelled: fn() -> bool) -> Result<Vec<Block>, Error> {
    let tree = parse(source)?;
    let mut result = Vec::new();

    for statement in statements(&tree)? {
        if cancelled() {
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
            _ => return Err(invalid("unsupported top-level declaration")),
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

                    if identifier(default) && !primitive(default) {
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
    cancelled: fn() -> bool,
) -> Result<Vec<(String, Vec<Replacement>)>, Error> {
    let tree = parse(source)?;
    let mut result = Vec::new();

    for statement in statements(&tree)? {
        if cancelled() {
            return Err(Error::Cancelled);
        }

        if !matches!(
            tree.node(statement).kind,
            NodeKind::Declaration {
                external: Some(_),
                ..
            }
        ) {
            return Err(invalid("invalid member declaration"));
        }

        let declaration = declaration(&tree, statement);

        let NodeKind::Class {
            name,
            members,
            end: Some(_),
            ..
        } = &tree.node(declaration).kind
        else {
            return Err(invalid("invalid member declaration"));
        };

        let mut signatures = Vec::new();

        for member in tree.list(members) {
            let name = match tree.node(member.node).kind {
                NodeKind::Function {
                    name: Some(name), ..
                } => name,

                NodeKind::TypeField { key, .. } => key,
                _ => return Err(invalid("invalid replacement member")),
            };

            let name = text(&tree, name)?.to_owned();

            if !identifier(&name) {
                return Err(invalid("invalid replacement member name"));
            }

            signatures.push(Replacement {
                name,
                source: text(&tree, member.node)?.to_owned(),
            });
        }

        result.push((text(&tree, *name)?.to_owned(), signatures));
    }

    Ok(result)
}

pub(super) fn ordered(source: &str, cancelled: fn() -> bool) -> Result<String, Error> {
    let mut pending = blocks(source, cancelled)?;
    let mut emitted = BTreeSet::new();
    let mut output = Output::default();

    while !pending.is_empty() {
        let count = pending.len();
        let mut remaining = Vec::new();

        for block in pending {
            if cancelled() {
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
                "unresolved class inheritance: {}",
                remaining
                    .iter()
                    .map(|block| block.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )));
        }

        pending = remaining;
    }

    output.finish(cancelled)
}

pub(super) fn injected(
    source: &str,
    exclusions: &[String],
    globals: &[String],
    exports: &[String],
    declarations: &[Block],
    cancelled: fn() -> bool,
) -> Result<String, Error> {
    let mut sections = String::new();
    let mut remaining = source;

    while let Some((_, section)) = remaining.split_once("-- SECTION BEGIN: ") {
        if cancelled() {
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

    for block in blocks(&sections, cancelled)? {
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

    output.finish(cancelled)
}

fn statements(tree: &Tree<'_>) -> Result<Vec<NodeIndex>, Error> {
    let NodeKind::Root { block, .. } = tree.node(tree.root).kind else {
        return Err(invalid("missing declaration root"));
    };

    let NodeKind::Block { statements } = &tree.node(block).kind else {
        return Err(invalid("missing declaration block"));
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
        .map_err(|error| invalid(format!("invalid declaration UTF-8: {error}")))
}

fn quoted(value: &str) -> Result<String, Error> {
    let quoted = serde_json::to_string(value).map_err(|source| Error::Data {
        input: "property name".to_owned(),
        source,
    })?;

    let mut result = String::with_capacity(quoted.len());
    let mut characters = quoted.chars();

    while let Some(character) = characters.next() {
        result.push(character);

        if character == '\\'
            && let Some(escaped) = characters.next()
        {
            match escaped {
                'u' => {
                    result.push_str("u{");

                    for _ in 0..4 {
                        if let Some(digit) = characters.next() {
                            result.push(digit);
                        }
                    }

                    result.push('}');
                }

                'b' => result.push_str("x08"),
                'f' => result.push_str("x0c"),
                escaped => result.push(escaped),
            }
        }
    }

    Ok(result)
}
