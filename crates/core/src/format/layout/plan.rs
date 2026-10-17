use vermis::{
    token::TokenKind,
    tree::{NodeIndex, NodeKind, Tree},
};

use super::{document::Document, protection, quotes::quote};

use crate::{
    format::{
        Error, Prepared,
        directives::{Directives, Kind},
        imports::Imports,
    },
    settings::format::Configuration,
};

pub(crate) struct Plan {
    pub(super) document: Vec<Document>,
    pub(super) quotes: Vec<Option<Vec<u8>>>,
    pub(super) final_newline: bool,
}

pub(crate) struct Builder<'tree, 'source> {
    pub(super) tree: &'tree Tree<'source>,
    pub(super) configuration: &'tree Configuration,
    pub(super) spacing: Vec<Option<usize>>,
    pub(super) imports: Option<&'tree Imports>,
    pub(super) directives: &'tree Directives,
    pub(super) prepared: &'tree Prepared,
}

impl<'tree, 'source> Builder<'tree, 'source> {
    pub(crate) fn build(
        tree: &'tree Tree<'source>,
        prepared: &'tree Prepared,
        imports: Option<&'tree Imports>,
        directives: &'tree Directives,
    ) -> Result<Plan, Error> {
        let configuration = prepared.settings();
        let regions = directives.regions()?;

        let quotes = tree
            .tokens
            .iter()
            .map(|token| {
                if token.kind == TokenKind::QuotedString
                    && !regions
                        .iter()
                        .any(|directive| directive.scope.contains(&token.span.start))
                {
                    quote(token.bytes(tree.source), configuration.quote_style)
                } else {
                    None
                }
            })
            .collect();

        let mut builder = Self {
            tree,
            configuration,
            spacing: vec![None; tree.tokens.len()],
            imports,
            directives,
            prepared,
        };

        builder.prepare_spacing();

        Ok(Plan {
            document: protection::apply(tree, builder.node(tree.root), &regions)?,
            quotes,
            final_newline: configuration.final_newline
                && !regions
                    .iter()
                    .any(|directive| directive.kind == Kind::Remainder),
        })
    }
}

impl Builder<'_, '_> {
    pub(super) fn node(&self, index: NodeIndex) -> Vec<Document> {
        let node = self.tree.node(index);
        let start = node.tokens.start.get();

        match &node.kind {
            NodeKind::Root { block, end_of_file } => {
                if let NodeKind::Block { statements } = &self.tree.node(*block).kind {
                    return self.block(self.tree.list(statements), 0..end_of_file.get(), true);
                }
            }

            NodeKind::Table {
                opening,
                fields,
                closing: Some(closing),
            }
            | NodeKind::TypeTable {
                opening,
                fields,
                closing: Some(closing),
                element: None,
                ..
            } => {
                return self.table(*opening, *closing, self.tree.list(fields), None);
            }

            NodeKind::TypeTable {
                opening,
                closing: Some(closing),
                element: Some(element),
                ..
            } => {
                return self.table(*opening, *closing, &[], Some(*element));
            }

            NodeKind::Arguments {
                opening: Some(opening),
                values,
                closing: Some(closing),
            } => {
                return self.arguments(*opening, *closing, self.tree.list(values));
            }

            NodeKind::If {
                branches,
                otherwise,
                end: Some(closing),
            } => {
                return self.conditional(self.tree.list(branches), *otherwise, *closing);
            }

            NodeKind::Class {
                name,
                extends,
                with,
                members,
                end: Some(closing),
                ..
            } => {
                let entries = self.tree.list(members);

                let header_end = with.map_or_else(
                    || self.tree.node(extends.unwrap_or(*name)).tokens.end.get(),
                    |keyword| keyword.get() + 1,
                );

                let mut result = self.sequence(start..header_end, &self.tree.children(index));
                result.push(Document::Line(1));

                result.push(Document::Indent {
                    conditional: false,
                    content: self.block(entries, header_end..closing.get(), false),
                });

                result.push(Document::Line(1));
                result.push(Document::Token(*closing));

                return result;
            }

            NodeKind::Attributes { attributes } => {
                let mut result = Vec::new();

                for entry in self.tree.list(attributes) {
                    result.extend(self.node(entry.node));
                    result.push(Document::Line(1));
                }

                return result;
            }

            _ => {}
        }

        self.scope(index)
    }
}
