use std::{cell::Cell, time::Instant};

use instar_analysis::{Options, Reason};
use instar_syntax::bindings::Bindings;

use vermis::{
    token::TokenKind,
    tree::{NodeIndex, Tree},
};

use super::super::{document::Document, quotes::quote, selection::Selection};
use crate::{Configuration, configuration::Order};

pub(in crate::formatter) struct Plan {
    pub(in crate::formatter) document: Vec<Document>,
    pub(in crate::formatter) quotes: Vec<Option<Vec<u8>>>,
    pub(in crate::formatter) indentation: usize,
}

pub(super) struct Statement {
    pub(super) content: Vec<Document>,
    pub(super) blank: bool,
    pub(super) require: Option<Require>,
}

pub(super) struct Require {
    pub(super) path: String,
    pub(super) binding: NodeIndex,
}

pub(in crate::formatter) struct Builder<'tree, 'source> {
    pub(super) tree: &'tree Tree<'source>,
    pub(super) configuration: &'tree Configuration,
    pub(super) spacing: Vec<Option<usize>>,
    pub(super) bindings: Bindings,
    pub(super) options: &'tree Options,
    pub(super) started: Instant,
    interruption: Cell<Option<Reason>>,
    pub(super) quotes: Vec<Option<Vec<u8>>>,
}

impl<'tree, 'source> Builder<'tree, 'source> {
    pub(in crate::formatter) fn build(
        tree: &'tree Tree<'source>,
        selection: Option<&Selection>,
        configuration: &'tree Configuration,
        options: &'tree Options,
        started: Instant,
    ) -> Result<Plan, Reason> {
        let bindings = if configuration.requires.order == Order::Preserve {
            Bindings::default()
        } else {
            Bindings::analyze(tree, Some((options, started)))
        };

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
                    if token.kind == TokenKind::QuotedString
                        && selection
                            .is_none_or(|selection| selection.range.contains(&token.span.start))
                    {
                        quote(
                            token.bytes(tree.source),
                            configuration.quote_style,
                            options,
                            started,
                        )
                    } else {
                        Ok(None)
                    }
                })
                .collect::<Result<_, _>>()?,
        };

        builder.prepare_spacing()?;

        let document = selection.map_or_else(
            || builder.node(tree.root),
            |selection| {
                builder.block(
                    &selection.statements,
                    selection.tokens(tree),
                    selection.indentation == 0,
                )
            },
        );

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
            indentation: selection.map_or(0, |selection| selection.indentation),
        })
    }

    pub(super) fn interrupted(&self) -> bool {
        let reason = self
            .interruption
            .get()
            .or_else(|| self.options.interrupted(self.started));

        self.interruption.set(reason);

        reason.is_some()
    }
}
