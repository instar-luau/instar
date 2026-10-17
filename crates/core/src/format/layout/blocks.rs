use std::ops::Range;

use vermis::{
    token::TokenKind,
    tree::{ListEntry, NodeIndex, NodeKind, TokenIndex},
};

use super::{document::Document, plan::Builder};

use crate::{
    format::imports::{self, Require},
    settings::format::{BlankLines, Order},
};

struct Statement {
    content: Vec<Document>,
    blank: bool,
    require: Option<Require>,
}

impl Builder<'_, '_> {
    pub(super) fn scope(&self, index: NodeIndex) -> Vec<Document> {
        let node = self.tree.node(index);
        let start = node.tokens.start.get();
        let end = node.tokens.end.get();

        match &node.kind {
            NodeKind::Function {
                parameters,
                returns,
                body: Some(body),
                end: Some(closing),
                ..
            } => {
                let header_end = self
                    .tree
                    .node(returns.unwrap_or(*parameters))
                    .tokens
                    .end
                    .get();

                return self.body(
                    index,
                    start..header_end,
                    *body,
                    header_end..closing.get(),
                    closing.get()..end,
                );
            }

            NodeKind::While {
                do_keyword: Some(keyword),
                body,
                end: Some(closing),
                ..
            }
            | NodeKind::NumericFor {
                do_keyword: Some(keyword),
                body,
                end_keyword: Some(closing),
                ..
            }
            | NodeKind::GenericFor {
                do_keyword: Some(keyword),
                body,
                end: Some(closing),
                ..
            }
            | NodeKind::Do {
                keyword,
                body,
                end: Some(closing),
            } => {
                let header_end = keyword.get() + 1;

                return self.body(
                    index,
                    start..header_end,
                    *body,
                    header_end..closing.get(),
                    closing.get()..end,
                );
            }

            NodeKind::Branch {
                then: Some(then),
                body,
                ..
            } => {
                return self.body(
                    index,
                    start..then.get() + 1,
                    *body,
                    then.get() + 1..end,
                    end..end,
                );
            }

            NodeKind::Else { keyword, body } => {
                return self.body(
                    index,
                    start..keyword.get() + 1,
                    *body,
                    keyword.get() + 1..end,
                    end..end,
                );
            }

            NodeKind::Repeat {
                keyword,
                body,
                until: Some(until),
                ..
            } => {
                return self.body(
                    index,
                    start..keyword.get() + 1,
                    *body,
                    keyword.get() + 1..until.get(),
                    until.get()..end,
                );
            }

            _ => {}
        }

        self.sequence(start..end, &self.tree.children(index))
    }

    pub(super) fn conditional(
        &self,
        branches: &[ListEntry],
        otherwise: Option<NodeIndex>,
        closing: TokenIndex,
    ) -> Vec<Document> {
        let mut result = Vec::new();

        let children = branches
            .iter()
            .map(|entry| entry.node)
            .chain(otherwise)
            .collect::<Vec<_>>();

        for (offset, &child) in children.iter().enumerate() {
            let child_node = self.tree.node(child);

            let finish = children.get(offset + 1).map_or(closing.get(), |next| {
                self.tree.node(*next).tokens.start.get()
            });

            if offset > 0 {
                result.push(Document::Line(1));
            }

            match &child_node.kind {
                NodeKind::Branch {
                    then: Some(keyword),
                    body,
                    ..
                }
                | NodeKind::Else { keyword, body } => {
                    let header_end = keyword.get() + 1;

                    result.extend(self.body(
                        child,
                        child_node.tokens.start.get()..header_end,
                        *body,
                        header_end..finish,
                        finish..finish,
                    ));
                }

                _ => {}
            }
        }

        result.push(Document::Line(1));
        result.push(Document::Token(closing));

        result
    }

    pub(super) fn block(
        &self,
        entries: &[ListEntry],
        range: Range<usize>,
        root: bool,
    ) -> Vec<Document> {
        let mut result = Vec::new();
        let mut position = range.start;

        if root {
            while position < range.end {
                let token = self.tree.tokens[position];

                if token.kind == TokenKind::Whitespace {
                    position += 1;
                } else if token.kind == TokenKind::Comment
                    && token.bytes(self.tree.source).starts_with(b"--!")
                {
                    result.push(Document::Token(TokenIndex::new(position)));
                    result.push(Document::Line(1));
                    position += 1;
                } else {
                    break;
                }
            }
        }

        let mut units = Vec::new();

        for (offset, entry) in entries.iter().enumerate() {
            let node = self.tree.node(entry.node);

            let next = entries.get(offset + 1).map_or(range.end, |entry| {
                self.tree.node(entry.node).tokens.start.get()
            });

            let finish = entry
                .separator
                .map_or(node.tokens.end.get(), |separator| separator.get() + 1);

            let tail = self.inline_end(finish, next);

            let mut content = self.trivia(position..node.tokens.start.get(), true);
            content.extend(self.node(entry.node));

            if let Some(separator) = entry.separator {
                content.extend(self.gap(node.tokens.end.get() - 1, separator.get()));
                content.push(Document::Token(separator));
            }

            content.extend(self.trivia(finish..tail, false));
            let blank = position > range.start && self.blank(position, node.tokens.start.get());

            units.push(Statement {
                content,
                blank,
                require: self
                    .imports
                    .and_then(|imports| imports.require(self.tree, entry.node, self.directives)),
            });

            position = tail;
        }

        self.order(&mut units);
        let mut previous_path = None;

        for (
            offset,
            Statement {
                content,
                blank,
                require,
            },
        ) in units.into_iter().enumerate()
        {
            let path = require.map(|require| require.path);

            if offset > 0 {
                let grouped = self.configuration.require.order == Order::Grouped
                    && self.configuration.require.blank_lines == BlankLines::BetweenGroups
                    && previous_path.as_deref().zip(path.as_deref()).is_some_and(
                        |(left, right)| self.prepared.group(left) != self.prepared.group(right),
                    );

                result.push(Document::Line(if blank || grouped { 2 } else { 1 }));
            }

            result.extend(content);
            previous_path = path;
        }

        if self.comments(position..range.end) {
            result.push(Document::Line(1));
            result.extend(self.trivia(position..range.end, true));
        }

        result
    }

    fn body(
        &self,
        index: NodeIndex,
        header: Range<usize>,
        body: NodeIndex,
        interior: Range<usize>,
        closing: Range<usize>,
    ) -> Vec<Document> {
        let children = self.tree.children(index);
        let mut result = self.sequence(header.clone(), &children);
        let interior_start = self.inline_end(header.end, interior.end);
        result.extend(self.trivia(header.end..interior_start, false));
        result.push(Document::Line(1));

        if let NodeKind::Block { statements } = &self.tree.node(body).kind {
            result.push(Document::Indent {
                conditional: false,
                content: self.block(
                    self.tree.list(statements),
                    interior_start..interior.end,
                    false,
                ),
            });
        }

        if !closing.is_empty() {
            result.push(Document::Line(1));
            result.extend(self.sequence(closing, &children));
        }

        result
    }

    fn order(&self, statements: &mut [Statement]) {
        for range in imports::runs(
            self.tree,
            statements
                .iter()
                .map(|statement| statement.require.as_ref()),
        ) {
            statements[range].sort_by(|left, right| {
                let left = &left
                    .require
                    .as_ref()
                    .expect("internal error: missing require in import run")
                    .path;

                let right = &right
                    .require
                    .as_ref()
                    .expect("internal error: missing require in import run")
                    .path;

                if self.configuration.require.order == Order::Grouped {
                    self.prepared
                        .group(left)
                        .cmp(&self.prepared.group(right))
                        .then_with(|| left.cmp(right))
                } else {
                    left.cmp(right)
                }
            });
        }
    }
}
