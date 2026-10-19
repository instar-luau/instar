use vermis::{
    token::TokenKind,
    tree::{NodeIndex, NodeKind, Tree},
};

use super::{
    document::{Document, Layout, Nesting},
    protection,
    quotes::quote,
};

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
    pub(super) inline_attributes: Vec<bool>,
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

        let mut inline_attributes = vec![false; tree.nodes.len()];

        for node in &tree.nodes {
            match &node.kind {
                NodeKind::Function {
                    name: None,
                    attributes: Some(attributes),
                    ..
                }
                | NodeKind::TypeFunction {
                    attributes: Some(attributes),
                    ..
                } => inline_attributes[attributes.get()] = true,

                _ => {}
            }
        }

        let mut builder = Self {
            tree,
            configuration,
            spacing: vec![None; tree.tokens.len()],
            inline_attributes,
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

            NodeKind::Call { .. } | NodeKind::MethodCall { .. } => {
                if let Some(content) = self.chain(index) {
                    return content;
                }
            }

            NodeKind::Binary {
                left,
                operator,
                right,
            }
            | NodeKind::TypeUnion {
                left: Some(left),
                operator,
                right,
            }
            | NodeKind::TypeIntersection {
                left: Some(left),
                operator,
                right,
            }
            | NodeKind::Assertion {
                expression: left,
                operator,
                annotation: right,
            }
            | NodeKind::CompoundAssignment {
                target: left,
                operator,
                value: right,
            } => {
                return self.binary(*left, *operator, *right);
            }

            NodeKind::TypeUnion {
                left: None,
                operator,
                right,
            }
            | NodeKind::TypeIntersection {
                left: None,
                operator,
                right,
            } => {
                return self.prefixed(*operator, *right);
            }

            NodeKind::Group {
                opening,
                expression,
                closing: Some(closing),
            }
            | NodeKind::TypeGroup {
                opening,
                annotation: expression,
                closing: Some(closing),
            } => {
                return self.delimited(*opening, *expression, *closing, 0);
            }

            NodeKind::TypeOf {
                opening: Some(opening),
                expression,
                closing: Some(closing),
                ..
            }
            | NodeKind::Index {
                opening,
                key: expression,
                closing: Some(closing),
                ..
            }
            | NodeKind::TableField {
                opening: Some(opening),
                key: Some(expression),
                closing: Some(closing),
                ..
            }
            | NodeKind::TypeField {
                opening: Some(opening),
                key: expression,
                closing: Some(closing),
                ..
            }
            | NodeKind::TypeIndexer {
                opening,
                key: expression,
                closing: Some(closing),
                ..
            } => {
                return self.wrapped(index, *opening, *expression, *closing);
            }

            NodeKind::Conditional { then: Some(_), .. } => {
                return self.selection(index);
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
                return self.collection(
                    opening.get()..opening.get() + 1,
                    closing.get()..closing.get() + 1,
                    self.tree.list(values),
                    Layout::Arguments,
                );
            }

            NodeKind::Parameters {
                opening: Some(opening),
                parameters: entries,
                closing: Some(closing),
            }
            | NodeKind::TypePack {
                opening: Some(opening),
                types: entries,
                closing: Some(closing),
            }
            | NodeKind::Generics {
                opening,
                parameters: entries,
                closing: Some(closing),
            }
            | NodeKind::TypeArguments {
                opening,
                arguments: entries,
                closing: Some(closing),
            }
            | NodeKind::AttributeGroup {
                opening,
                attributes: entries,
                closing: Some(closing),
            } => {
                return self.collection(
                    opening.get()..opening.get() + 1,
                    closing.get()..closing.get() + 1,
                    self.tree.list(entries),
                    Layout::Fit,
                );
            }

            NodeKind::InstantiationArguments {
                opening,
                arguments,
                closing: Some(closing),
            } => {
                if let NodeKind::TypeArguments {
                    opening: inner_opening,
                    arguments,
                    closing: Some(inner_closing),
                } = &self.tree.node(*arguments).kind
                {
                    return self.collection(
                        opening.get()..inner_opening.get() + 1,
                        inner_closing.get()..closing.get() + 1,
                        self.tree.list(arguments),
                        Layout::Fit,
                    );
                }
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
                    nesting: Nesting::Always,
                    content: self.block(entries, header_end..closing.get(), false),
                });

                result.push(Document::Line(1));
                result.push(Document::Token(*closing));

                return result;
            }

            NodeKind::Attributes { attributes } => {
                if self.inline_attributes[index.get()] {
                    return self.sequence(start..node.tokens.end.get(), &self.tree.children(index));
                }

                let entries = self.tree.list(attributes);
                let mut result = Vec::new();
                let mut position = start;

                for (offset, entry) in entries.iter().enumerate() {
                    let attribute = self.tree.node(entry.node);

                    let next = entries
                        .get(offset + 1)
                        .map_or(node.tokens.end.get(), |entry| {
                            self.tree.node(entry.node).tokens.start.get()
                        });

                    let tail = self.inline_end(attribute.tokens.end.get(), next);
                    result.extend(self.trivia(position..attribute.tokens.start.get(), true));
                    result.extend(self.node(entry.node));
                    result.extend(self.trivia(attribute.tokens.end.get()..tail, false));

                    if offset + 1 < entries.len() {
                        result.push(Document::Line(1));
                    }

                    position = tail;
                }

                result.extend(self.trivia(position..node.tokens.end.get(), true));

                return result;
            }

            _ => {}
        }

        self.scope(index)
    }
}
