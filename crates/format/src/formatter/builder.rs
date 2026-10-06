mod layout;
mod requires;
mod trivia;

use super::{document::Document, quotes::quote};
use crate::Configuration;
use instar_analysis::{Options, Reason};
use instar_syntax::bindings::Bindings;
use std::{cell::Cell, time::Instant};

use vermis::{
    token::TokenKind,
    tree::{NodeIndex, Tree},
};

pub(super) struct Plan {
    pub(super) document: Vec<Document>,
    pub(super) quotes: Vec<Option<Vec<u8>>>,
}

struct Statement {
    content: Vec<Document>,
    blank: bool,
    require: Option<Require>,
}

struct Require {
    path: String,
    binding: NodeIndex,
}

pub(super) struct Builder<'tree, 'source> {
    tree: &'tree Tree<'source>,
    configuration: &'tree Configuration,
    spacing: Vec<Option<usize>>,
    bindings: Bindings,
    options: &'tree Options,
    started: Instant,
    interruption: Cell<Option<Reason>>,
    quotes: Vec<Option<Vec<u8>>>,
}

impl<'tree, 'source> Builder<'tree, 'source> {
    pub(super) fn build(
        tree: &'tree Tree<'source>,
        configuration: &'tree Configuration,
        options: &'tree Options,
        started: Instant,
    ) -> Result<Plan, Reason> {
        let bindings = Bindings::analyze(tree, options, started);

        if let Some(reason) = bindings.interruption {
            return Err(reason);
        }

        let mut builder = Self {
            tree,
            configuration,
            bindings,
            options,
            started,
            interruption: Cell::new(None),
            spacing: vec![None; tree.tokens.len()],
            quotes: tree
                .tokens
                .iter()
                .map(|token| {
                    if token.kind == TokenKind::QuotedString {
                        quote(
                            token.bytes(tree.source),
                            configuration.quote_style,
                            options,
                            started,
                        )
                        .map(Some)
                    } else {
                        Ok(None)
                    }
                })
                .collect::<Result<_, _>>()?,
        };

        builder.prepare_spacing()?;

        let document = builder.node(tree.root);

        if let Some(reason) = builder
            .interruption
            .get()
            .or_else(|| options.interrupted(started))
        {
            return Err(reason);
        }

        Ok(Plan {
            document,
            quotes: builder.quotes,
        })
    }

    fn interrupted(&self) -> bool {
        let reason = self
            .interruption
            .get()
            .or_else(|| self.options.interrupted(self.started));

        self.interruption.set(reason);

        reason.is_some()
    }
}
