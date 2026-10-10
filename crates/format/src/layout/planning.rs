use std::{cell::Cell, collections::BTreeSet, iter, ops::Range};

use instar_shared::{Completion, Options, Reason};
use instar_syntax::{BindingResult, SyntaxSelection, Tree as SnapshotTree, bindings, literal};

use instar_syntax::{
    token::{Keyword, Symbol, Token, TokenKind},
    tree::{ListEntry, NodeIndex, NodeKind, TokenIndex, Tree},
};

use super::{
    document::{Document, Layout},
    quotes::quote,
};

use crate::{
    FormatSettings,
    settings::{BlankLines, BuiltinGroup, Group, Order},
};

pub(crate) struct Plan {
    pub(super) document: Vec<Document>,
    pub(super) quotes: Vec<Option<Vec<u8>>>,
    pub(super) indentation: usize,
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

pub(crate) struct Builder<'tree, 'source> {
    pub(super) tree: &'tree Tree<'source>,
    pub(super) configuration: &'tree FormatSettings,
    pub(super) spacing: Vec<Option<usize>>,
    pub(super) bindings: BindingResult,
    pub(super) options: &'tree Options,
    interruption: Cell<Option<Reason>>,
    pub(super) quotes: Vec<Option<Vec<u8>>>,
}

impl<'tree, 'source> Builder<'tree, 'source> {
    pub(crate) fn build(
        snapshot: &'tree SnapshotTree<'source>,
        selection: Option<&SyntaxSelection>,
        configuration: &'tree FormatSettings,
        options: &'tree Options,
    ) -> Result<Plan, Reason> {
        let tree = snapshot.syntax();
        let bindings = bindings(snapshot, options);

        if bindings.completion != Completion::Complete {
            return Err(Reason::FailedPrerequisite);
        }

        let indentation = selection.map_or(0, |selection| {
            let width = configuration.indent_width.get();

            selection.indentation.iter().fold(0, |column, byte| {
                if *byte == b'\t' {
                    column + width - column % width
                } else {
                    column + 1
                }
            }) / width
                + usize::from(selection.nested)
        });

        let range = selection
            .and_then(|selection| selection.expanded.first())
            .copied();

        let mut builder = Self {
            tree,
            configuration,
            bindings,
            options,
            interruption: Cell::new(None),
            spacing: vec![None; tree.tokens.len()],
            quotes: tree
                .tokens
                .iter()
                .map(|token| {
                    if token.kind == TokenKind::QuotedString
                        && selection.is_none_or(|selection| {
                            selection.expanded.iter().any(|span| {
                                span.start <= token.span.start && token.span.start < span.end
                            })
                        })
                    {
                        quote(token.bytes(tree.source), configuration.quote_style, options)
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
                    tree.tokens.partition_point(|token| {
                        token.span.end <= range.expect("selected span").start
                    })..tree.tokens.partition_point(|token| {
                        token.span.start < range.expect("selected span").end
                    }),
                    indentation == 0,
                )
            },
        );

        if let Some(reason) = builder
            .interruption
            .get()
            .or_else(|| options.interrupted().map(|_| Reason::FailedPrerequisite))
        {
            return Err(reason);
        }

        Ok(Plan {
            document,
            quotes: builder.quotes,
            indentation,
        })
    }

    pub(super) fn interrupted(&self) -> bool {
        let reason = self.interruption.get().or_else(|| {
            self.options
                .interrupted()
                .map(|_| Reason::FailedPrerequisite)
        });

        self.interruption.set(reason);

        reason.is_some()
    }
}

impl Builder<'_, '_> {
    pub(super) fn require(&self, index: NodeIndex) -> Option<Require> {
        if self.configuration.requires.order == Order::Preserve {
            return None;
        }

        let NodeKind::Local {
            bindings, values, ..
        } = &self.tree.node(index).kind
        else {
            return None;
        };

        if self.tree.list(bindings).len() != 1 || self.tree.list(values).len() != 1 {
            return None;
        }

        let NodeKind::Binding {
            name,
            annotation: None,
            ..
        } = &self.tree.node(self.tree.list(bindings)[0].node).kind
        else {
            return None;
        };

        if self.tree.text(*name) == b"require" {
            return None;
        }

        let NodeKind::Call { callee, arguments } =
            &self.tree.node(self.tree.list(values)[0].node).kind
        else {
            return None;
        };

        if !self.bindings.global(self.tree, *callee)
            || !matches!(self.tree.node(*callee).kind, NodeKind::Name { .. })
            || self.tree.text(*callee) != b"require"
        {
            return None;
        }

        let NodeKind::Arguments { values, .. } = &self.tree.node(*arguments).kind else {
            return None;
        };

        if self.tree.list(values).len() != 1 {
            return None;
        }

        Some(Require {
            path: literal::string(self.tree, self.tree.list(values)[0].node)?,
            binding: *name,
        })
    }

    pub(super) fn order(&self, statements: &mut [Statement]) {
        if self.configuration.requires.order == Order::Preserve {
            return;
        }

        let mut start = 0;

        while start < statements.len() {
            if self.interrupted() {
                return;
            }

            if statements[start].require.is_none() {
                start += 1;
                continue;
            }

            let mut end = start + 1;

            while end < statements.len() && statements[end].require.is_some() {
                end += 1;
            }

            let mut bindings = BTreeSet::new();

            if statements[start..end].iter().all(|statement| {
                bindings.insert(
                    self.tree
                        .text(statement.require.as_ref().expect("require run").binding),
                )
            }) {
                statements[start..end].sort_by(|left, right| {
                    let left = &left.require.as_ref().expect("require run").path;
                    let right = &right.require.as_ref().expect("require run").path;

                    if self.configuration.requires.order == Order::Grouped {
                        self.group(left)
                            .cmp(&self.group(right))
                            .then_with(|| left.cmp(right))
                    } else {
                        left.cmp(right)
                    }
                });
            }

            start = end;
        }
    }

    pub(super) fn group(&self, path: &str) -> usize {
        self.configuration
            .requires
            .groups
            .iter()
            .position(|group| match group {
                Group::Builtin(BuiltinGroup::Alias) => path.starts_with('@'),

                Group::Builtin(BuiltinGroup::Relative) => {
                    path.starts_with("./") || path.starts_with("../")
                }

                Group::Builtin(BuiltinGroup::Other) => true,

                Group::Custom(group) => group.patterns.iter().any(|pattern| {
                    globset::Glob::new(pattern)
                        .is_ok_and(|pattern| pattern.compile_matcher().is_match(path))
                }),
            })
            .unwrap_or(self.configuration.requires.groups.len())
    }
}

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
                content: iter::once(Document::Soft(1)).chain(interior).collect(),
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

impl Builder<'_, '_> {
    pub(super) fn prepare_spacing(&mut self) -> Result<(), Reason> {
        let tree = self.tree;

        for node in &tree.nodes {
            if let Some(reason) = self
                .options
                .interrupted()
                .map(|_| Reason::FailedPrerequisite)
            {
                return Err(reason);
            }

            match &node.kind {
                NodeKind::Unary { operator, operand } => {
                    self.spacing[tree.node(*operand).tokens.start.get()] = Some(usize::from(
                        tree.token(*operator).kind == TokenKind::Keyword(Keyword::Not),
                    ));
                }

                NodeKind::MethodCall { colon, method, .. }
                | NodeKind::FunctionName {
                    colon: Some(colon),
                    method: Some(method),
                    ..
                } => {
                    self.spacing[colon.get()] = Some(0);
                    self.spacing[tree.node(*method).tokens.start.get()] = Some(0);
                }

                NodeKind::Generics {
                    opening, closing, ..
                }
                | NodeKind::TypeArguments {
                    opening, closing, ..
                }
                | NodeKind::InstantiationArguments {
                    opening, closing, ..
                } => {
                    self.spacing[opening.get()] = Some(0);

                    if let Some(next) = self.next(opening.get() + 1, node.tokens.end.get()) {
                        self.spacing[next] = Some(0);
                    }

                    if let Some(closing) = closing {
                        self.spacing[closing.get()] = Some(0);
                    }
                }

                NodeKind::GenericPack {
                    ellipsis: Some(ellipsis),
                    ..
                }
                | NodeKind::Generic {
                    ellipsis: Some(ellipsis),
                    ..
                } => {
                    self.spacing[ellipsis.get()] = Some(0);
                }

                NodeKind::VariadicType { annotation, .. } => {
                    self.spacing[tree.node(*annotation).tokens.start.get()] = Some(0);
                }

                NodeKind::TypeIndexer {
                    access: Some(_),
                    opening,
                    ..
                }
                | NodeKind::TypeField {
                    access: Some(_),
                    opening: Some(opening),
                    ..
                } => {
                    self.spacing[opening.get()] = Some(1);
                }

                _ => {}
            }
        }

        Ok(())
    }

    pub(super) fn next(&self, start: usize, end: usize) -> Option<usize> {
        (start..end).find(|&position| {
            !matches!(
                self.tree.tokens[position].kind,
                TokenKind::Whitespace | TokenKind::EndOfFile
            )
        })
    }

    pub(super) fn previous(&self, start: usize, end: usize) -> Option<usize> {
        (start..end).rev().find(|&position| {
            !matches!(
                self.tree.tokens[position].kind,
                TokenKind::Whitespace | TokenKind::EndOfFile
            )
        })
    }

    pub(super) fn lines(&self, start: usize, end: usize) -> usize {
        self.tree.source[start..end]
            .split(|&byte| byte == b'\n')
            .take(3)
            .count()
            - 1
    }

    pub(super) fn comments(&self, range: Range<usize>) -> bool {
        self.tree.tokens[range]
            .iter()
            .any(|token| matches!(token.kind, TokenKind::Comment | TokenKind::BlockComment))
    }

    pub(super) fn blank(&self, start: usize, end: usize) -> bool {
        let begin = start
            .checked_sub(1)
            .map_or(0, |position| self.tree.tokens[position].span.end);

        let finish = self
            .next(start, end)
            .map_or(self.tree.tokens[end].span.start, |position| {
                self.tree.tokens[position].span.start
            });

        self.lines(begin, finish) > 1
    }

    pub(super) fn gap(&self, left: usize, right: usize) -> Vec<Document> {
        let mut result = Vec::new();
        let mut previous = left;

        for position in left + 1..right {
            if self.interrupted() {
                break;
            }

            let token = self.tree.tokens[position];

            if !matches!(token.kind, TokenKind::Comment | TokenKind::BlockComment) {
                continue;
            }

            let lines = self.lines(self.tree.tokens[previous].span.end, token.span.start);

            result.push(if lines > 0 {
                Document::Line(lines.min(2))
            } else {
                Document::Space
            });

            result.push(Document::Token(TokenIndex::new(position)));
            previous = position;
        }

        if previous != left {
            let lines = self.lines(
                self.tree.tokens[previous].span.end,
                self.tree.tokens[right].span.start,
            );

            result.push(
                if lines > 0 || self.tree.tokens[previous].kind == TokenKind::Comment {
                    Document::Line(lines.clamp(1, 2))
                } else {
                    Document::Space
                },
            );
        } else if self.space(left, right) {
            result.push(Document::Space);
        }

        result
    }

    pub(super) fn space(&self, left: usize, right: usize) -> bool {
        if let Some(spaces) = self.spacing[right] {
            return spaces > 0;
        }

        let left = self.tree.tokens[left].kind;
        let right = self.tree.tokens[right].kind;

        if matches!(
            left,
            TokenKind::InterpolatedStringStart | TokenKind::InterpolatedStringMiddle
        ) || matches!(
            right,
            TokenKind::InterpolatedStringMiddle | TokenKind::InterpolatedStringEnd
        ) {
            return false;
        }

        if matches!(
            right,
            TokenKind::Symbol(
                Symbol::RightParenthesis
                    | Symbol::RightBracket
                    | Symbol::Comma
                    | Symbol::Semicolon
                    | Symbol::Colon
                    | Symbol::Dot
                    | Symbol::QuestionMark
            )
        ) || matches!(
            left,
            TokenKind::Symbol(Symbol::LeftParenthesis | Symbol::LeftBracket | Symbol::Dot)
        ) {
            return false;
        }

        if matches!(right, TokenKind::Symbol(Symbol::LeftParenthesis)) {
            return !matches!(
                left,
                TokenKind::Name
                    | TokenKind::Keyword(Keyword::Function)
                    | TokenKind::Symbol(
                        Symbol::RightParenthesis
                            | Symbol::RightBracket
                            | Symbol::RightBrace
                            | Symbol::GreaterThan
                            | Symbol::QuestionMark
                    )
            );
        }

        if matches!(right, TokenKind::Symbol(Symbol::LeftBracket)) {
            return matches!(
                left,
                TokenKind::Symbol(Symbol::Colon | Symbol::Comma | Symbol::Assignment)
            );
        }

        if matches!(left, TokenKind::Symbol(Symbol::Colon | Symbol::Comma)) {
            return true;
        }

        if matches!(left, TokenKind::Symbol(Symbol::QuestionMark)) {
            return matches!(
                right,
                TokenKind::Symbol(Symbol::Assignment | Symbol::Pipe | Symbol::Ampersand)
            );
        }

        if matches!(left, TokenKind::Symbol(Symbol::Ellipsis)) {
            return matches!(right, TokenKind::Symbol(Symbol::Assignment));
        }

        if matches!(
            left,
            TokenKind::Symbol(Symbol::RightParenthesis | Symbol::RightBracket)
        ) {
            return matches!(
                right,
                TokenKind::Keyword(_)
                    | TokenKind::Name
                    | TokenKind::Symbol(
                        Symbol::Assignment
                            | Symbol::DoubleColon
                            | Symbol::Arrow
                            | Symbol::Add
                            | Symbol::Subtract
                            | Symbol::Multiply
                            | Symbol::Divide
                            | Symbol::FloorDivide
                            | Symbol::Modulo
                            | Symbol::Power
                            | Symbol::Concatenate
                            | Symbol::Equal
                            | Symbol::NotEqual
                            | Symbol::LessThan
                            | Symbol::LessThanOrEqual
                            | Symbol::GreaterThan
                            | Symbol::GreaterThanOrEqual
                            | Symbol::Pipe
                            | Symbol::Ampersand
                    )
            );
        }

        true
    }

    pub(super) fn trivia(&self, range: Range<usize>, leading: bool) -> Vec<Document> {
        let mut result = Vec::new();
        let mut previous = None;

        for position in range {
            if self.interrupted() {
                break;
            }

            let token = self.tree.tokens[position];

            if !matches!(token.kind, TokenKind::Comment | TokenKind::BlockComment) {
                continue;
            }

            if let Some(previous) = previous {
                let prior: &Token = &self.tree.tokens[previous];
                let lines = self.lines(prior.span.end, token.span.start);

                result.push(if lines > 0 || prior.kind == TokenKind::Comment {
                    Document::Line(lines.clamp(1, 2))
                } else {
                    Document::Space
                });
            } else if !leading {
                result.push(Document::Space);
            }

            result.push(Document::Token(TokenIndex::new(position)));
            previous = Some(position);
        }

        if leading && previous.is_some() {
            result.push(Document::Line(1));
        }

        result
    }

    pub(super) fn sequence(&self, range: Range<usize>, children: &[NodeIndex]) -> Vec<Document> {
        let mut result = Vec::new();
        let mut position = range.start;
        let mut previous = None;

        for &child in children {
            if self.interrupted() {
                break;
            }

            let node = self.tree.node(child);
            let start = node.tokens.start.get();
            let end = node.tokens.end.get();

            if start < position || end > range.end || start == end {
                continue;
            }

            self.tokens(position..start, &mut previous, &mut result);

            if let Some(previous) = previous {
                result.extend(self.gap(previous, start));
            }

            result.extend(self.node(child));
            previous = self.previous(start, end);
            position = end;
        }

        self.tokens(position..range.end, &mut previous, &mut result);

        result
    }

    pub(super) fn tokens(
        &self,
        range: Range<usize>,
        previous: &mut Option<usize>,
        result: &mut Vec<Document>,
    ) {
        for position in range {
            if self.interrupted() {
                break;
            }

            if matches!(
                self.tree.tokens[position].kind,
                TokenKind::Whitespace
                    | TokenKind::Comment
                    | TokenKind::BlockComment
                    | TokenKind::EndOfFile
            ) {
                continue;
            }

            if let Some(previous) = *previous {
                result.extend(self.gap(previous, position));
            }

            result.push(Document::Token(TokenIndex::new(position)));
            *previous = Some(position);
        }
    }

    pub(super) fn inline_end(&self, start: usize, end: usize) -> usize {
        let mut position = start;

        while position < end {
            let token = self.tree.tokens[position];

            if token.kind == TokenKind::Whitespace && token.bytes(self.tree.source).contains(&b'\n')
            {
                break;
            }

            if !matches!(
                token.kind,
                TokenKind::Whitespace | TokenKind::Comment | TokenKind::BlockComment
            ) {
                break;
            }

            position += 1;
        }

        position
    }
}
