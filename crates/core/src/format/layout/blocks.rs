use std::ops::Range;

use vermis::{
    token::TokenKind,
    tree::{ListEntry, NodeIndex, NodeKind, TokenIndex},
};

use super::{
    document::{Document, Nesting},
    plan::Builder,
};

use crate::{
    format::imports::{self, Require},
    settings::format::{BlankLines, Order},
};

struct Statement {
    content: Vec<Document>,
    has_blank_line: bool,
    is_inline: bool,
    require: Option<Require>,
}

impl Builder<'_, '_> {
    pub(super) fn scope(&self, index: NodeIndex) -> Vec<Document> {
        let node = self.tree.node(index);
        let start = node.tokens.start.get();
        let end = node.tokens.end.get();

        match &node.kind {
            NodeKind::Local {
                bindings, values, ..
            }
            | NodeKind::Constant {
                bindings, values, ..
            }
            | NodeKind::Assignment {
                targets: bindings,
                values,
                ..
            } => {
                return self.separated(
                    start..end,
                    &[self.tree.list(bindings), self.tree.list(values)],
                );
            }

            NodeKind::Return { values, .. } => {
                return self.separated(start..end, &[self.tree.list(values)]);
            }

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

        if end == start + 1 {
            return vec![Document::Token(TokenIndex::new(start))];
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
        is_root: bool,
    ) -> Vec<Document> {
        let mut result = Vec::new();
        let mut position = range.start;
        let mut is_inline = false;

        if is_root {
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

            let tail = (finish..tail)
                .find(|position| self.directives.marker(*position).is_some())
                .unwrap_or(tail);

            if let Some(boundary) = self
                .directives
                .boundary(self.tree, entry.node)
                .filter(|boundary| position < *boundary)
            {
                units.push(Statement {
                    content: self.trivia(position..boundary, true),
                    has_blank_line: position > range.start
                        && self.has_blank_line(position, node.tokens.start.get()),
                    is_inline,
                    require: None,
                });

                position = boundary;
                is_inline = false;
            }

            let mut content = self.trivia(position..node.tokens.start.get(), true);
            content.extend(self.node(entry.node));

            if let Some(separator) = entry.separator {
                content.extend(self.gap(node.tokens.end.get() - 1, separator.get()));
                content.push(Document::Token(separator));
            }

            content.extend(self.trivia(finish..tail, false));

            let has_blank_line =
                position > range.start && self.has_blank_line(position, node.tokens.start.get());

            units.push(Statement {
                content,
                has_blank_line,
                is_inline,
                require: self
                    .imports
                    .and_then(|imports| imports.require(self.tree, entry.node, self.directives)),
            });

            position = tail;
            is_inline = self.directives.marker(tail).is_some();
        }

        self.order(&mut units);
        let mut previous_path = None;

        for (
            offset,
            Statement {
                content,
                has_blank_line,
                is_inline,
                require,
            },
        ) in units.into_iter().enumerate()
        {
            let path = require.map(|require| require.path);

            if offset > 0 {
                let is_grouped = self.configuration.require.order == Order::Grouped
                    && self.configuration.require.blank_lines == BlankLines::BetweenGroups
                    && previous_path.as_deref().zip(path.as_deref()).is_some_and(
                        |(left, right)| self.prepared.group(left) != self.prepared.group(right),
                    );

                result.push(if is_inline {
                    Document::Space
                } else {
                    Document::Line(if has_blank_line || is_grouped { 2 } else { 1 })
                });
            }

            result.extend(content);
            previous_path = path;
        }

        if self.has_comments(position..range.end) {
            result.push(if is_inline {
                Document::Space
            } else {
                Document::Line(1)
            });

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

        let mut result = match &self.tree.node(index).kind {
            NodeKind::Branch {
                keyword,
                condition,
                then: Some(closing),
                ..
            }
            | NodeKind::While {
                keyword,
                condition,
                do_keyword: Some(closing),
                ..
            } => self.framed(
                *keyword,
                self.tree.node(*condition).tokens.start.get()
                    ..self.tree.node(*condition).tokens.end.get(),
                *closing,
                1,
                self.condition(*condition),
            ),

            NodeKind::GenericFor {
                keyword,
                bindings,
                values,
                do_keyword: Some(closing),
                ..
            } => {
                let interior = keyword.get() + 1..closing.get();

                self.framed(
                    *keyword,
                    interior.clone(),
                    *closing,
                    1,
                    self.separated(
                        interior,
                        &[self.tree.list(bindings), self.tree.list(values)],
                    ),
                )
            }

            NodeKind::NumericFor {
                keyword,
                binding,
                start,
                range_separator,
                end,
                step_separator,
                step,
                do_keyword: Some(closing),
                ..
            } => {
                let binding = [ListEntry {
                    node: *binding,
                    separator: None,
                }];

                let mut values = vec![
                    ListEntry {
                        node: *start,
                        separator: *range_separator,
                    },
                    ListEntry {
                        node: *end,
                        separator: *step_separator,
                    },
                ];

                values.extend(step.map(|step| ListEntry {
                    node: step,
                    separator: None,
                }));

                let interior = keyword.get() + 1..closing.get();

                self.framed(
                    *keyword,
                    interior.clone(),
                    *closing,
                    1,
                    self.separated(interior, &[&binding, &values]),
                )
            }

            _ => self.sequence(header.clone(), &children),
        };

        let interior_start = self.inline_end(header.end, interior.end);
        result.extend(self.trivia(header.end..interior_start, false));
        result.push(Document::Line(1));

        if let NodeKind::Block { statements } = &self.tree.node(body).kind {
            result.push(Document::Indent {
                nesting: Nesting::Always,
                content: self.block(
                    self.tree.list(statements),
                    interior_start..interior.end,
                    false,
                ),
            });
        }

        if !closing.is_empty() {
            result.push(Document::Line(1));

            result.extend(match &self.tree.node(index).kind {
                NodeKind::Repeat {
                    until: Some(until),
                    condition,
                    ..
                } => self.prefixed(*until, *condition),

                _ => self.sequence(closing, &children),
            });
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
                    .expect("Internal error: missing `require` in import run.")
                    .path;

                let right = &right
                    .require
                    .as_ref()
                    .expect("Internal error: missing `require` in import run.")
                    .path;

                imports::compare(self.prepared, left, right)
            });
        }
    }
}
