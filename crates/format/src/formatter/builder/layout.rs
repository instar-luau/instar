use std::ops::Range;

use vermis::{
    token::{Symbol, TokenKind},
    tree::{ListEntry, NodeIndex, NodeKind, TokenIndex},
};

use super::{
    super::document::{Document, Layout},
    Builder, Statement,
};

use crate::configuration::{BlankLines, Order};

impl Builder<'_, '_> {
    pub(super) fn node(&self, index: NodeIndex) -> Vec<Document> {
        if self.interrupted() {
            return Vec::new();
        }

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
                    if self.interrupted() {
                        break;
                    }

                    result.extend(self.node(entry.node));
                    result.push(Document::Line(1));
                }

                return result;
            }

            _ => {}
        }

        self.scope(index)
    }

    pub(super) fn scope(&self, index: NodeIndex) -> Vec<Document> {
        if self.interrupted() {
            return Vec::new();
        }

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

    pub(super) fn body(
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
            if self.interrupted() {
                break;
            }

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
            if self.interrupted() {
                break;
            }

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
                require: self.require(entry.node),
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
            if self.interrupted() {
                break;
            }

            let path = require.map(|require| require.path);

            if offset > 0 {
                let grouped = self.configuration.requires.order == Order::Grouped
                    && self.configuration.requires.blank_lines == BlankLines::BetweenGroups
                    && previous_path
                        .as_deref()
                        .zip(path.as_deref())
                        .is_some_and(|(left, right)| self.group(left) != self.group(right));

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

    pub(super) fn table(
        &self,
        opening: TokenIndex,
        closing: TokenIndex,
        entries: &[ListEntry],
        element: Option<NodeIndex>,
    ) -> Vec<Document> {
        if entries.is_empty()
            && element.is_none()
            && !self.comments(opening.get() + 1..closing.get())
        {
            return vec![Document::Token(opening), Document::Token(closing)];
        }

        let forced = entries
            .last()
            .is_some_and(|entry| entry.separator.is_some())
            || self.comments(opening.get() + 1..closing.get());

        let mut interior = Vec::new();
        let mut position = opening.get() + 1;

        if let Some(element) = element {
            interior.extend(self.sequence(position..closing.get(), &[element]));
        } else {
            for (offset, entry) in entries.iter().enumerate() {
                if self.interrupted() {
                    break;
                }

                let node = self.tree.node(entry.node);

                let next = entries.get(offset + 1).map_or(closing.get(), |entry| {
                    self.tree.node(entry.node).tokens.start.get()
                });

                let finish = entry
                    .separator
                    .map_or(node.tokens.end.get(), |separator| separator.get() + 1);

                let tail = self.inline_end(finish, next);

                if offset > 0 {
                    if self.blank(position, node.tokens.start.get()) {
                        interior.push(Document::Line(2));
                    } else {
                        interior.push(Document::Soft(1));
                    }
                }

                interior.extend(self.trivia(position..node.tokens.start.get(), true));
                interior.extend(self.node(entry.node));

                if let Some(separator) = entry.separator {
                    interior.extend(self.gap(node.tokens.end.get() - 1, separator.get()));
                }

                if offset + 1 < entries.len() {
                    interior.push(Document::Symbol(Symbol::Comma));
                } else {
                    interior.push(Document::TrailingComma);
                }

                interior.extend(self.trivia(finish..tail, false));
                position = tail;
            }

            if self.comments(position..closing.get()) {
                interior.push(Document::Line(1));
                interior.extend(self.trivia(position..closing.get(), true));
            }
        }

        let content = vec![
            Document::Token(opening),
            Document::Indent {
                conditional: true,
                content: std::iter::once(Document::Soft(1)).chain(interior).collect(),
            },
            Document::Soft(1),
            Document::Token(closing),
        ];

        vec![Document::Group {
            layout: if forced {
                Layout::Vertical
            } else {
                Layout::Fit
            },
            content,
        }]
    }

    pub(super) fn arguments(
        &self,
        opening: TokenIndex,
        closing: TokenIndex,
        entries: &[ListEntry],
    ) -> Vec<Document> {
        if entries.is_empty() && !self.comments(opening.get() + 1..closing.get()) {
            return vec![Document::Token(opening), Document::Token(closing)];
        }

        let documents = entries
            .iter()
            .map(|entry| self.node(entry.node))
            .collect::<Vec<_>>();

        let multiline = documents
            .iter()
            .flatten()
            .any(|item| item.multiline(self.tree));

        let mut interior = vec![Document::Soft(0)];
        let mut position = opening.get() + 1;

        for (offset, (entry, document)) in entries.iter().zip(documents).enumerate() {
            if self.interrupted() {
                break;
            }

            let node = self.tree.node(entry.node);

            if offset > 0 {
                interior.push(Document::Soft(1));
            }

            interior.extend(self.trivia(position..node.tokens.start.get(), true));
            interior.extend(document);

            if let Some(separator) = entry.separator {
                interior.extend(self.gap(node.tokens.end.get() - 1, separator.get()));
            }

            if offset + 1 < entries.len() {
                interior.push(Document::Symbol(Symbol::Comma));
            }

            let finish = entry
                .separator
                .map_or(node.tokens.end.get(), |separator| separator.get() + 1);

            let next = entries.get(offset + 1).map_or(closing.get(), |entry| {
                self.tree.node(entry.node).tokens.start.get()
            });

            let tail = self.inline_end(finish, next);
            interior.extend(self.trivia(finish..tail, false));
            position = tail;
        }

        interior.extend(self.trivia(position..closing.get(), true));

        vec![Document::Group {
            layout: if multiline {
                Layout::Arguments
            } else {
                Layout::Fit
            },
            content: vec![
                Document::Token(opening),
                Document::Indent {
                    conditional: true,
                    content: interior,
                },
                Document::Soft(0),
                Document::Token(closing),
            ],
        }]
    }
}
