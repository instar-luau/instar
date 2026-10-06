use std::ops::Range;

use vermis::{
    emitter::{Emitter, LineEnding as EmittedLineEnding},
    token::{Keyword, Symbol, TokenKind},
    tree::{ListEntry, NodeIndex, NodeKind, TokenIndex, Tree},
};

use crate::configuration::{
    BlankLines, BuiltinGroup, Configuration, Group, IndentStyle, LineEnding, Order, QuoteStyle,
};

#[derive(Clone, Copy)]
enum Layout {
    Fit,
    Vertical,
    Pressed,
}

enum Document {
    Token(TokenIndex),
    Symbol(Symbol),
    Space,
    Line(usize),
    Soft(usize),
    TrailingComma,

    Indent {
        conditional: bool,
        content: Vec<Self>,
    },

    Group {
        layout: Layout,
        content: Vec<Self>,
    },
}

struct Builder<'tree, 'source> {
    tree: &'tree Tree<'source>,
    configuration: &'tree Configuration,
    spacing: Vec<Option<usize>>,
    quotes: Vec<Option<Vec<u8>>>,
}

impl<'tree, 'source> Builder<'tree, 'source> {
    fn new(tree: &'tree Tree<'source>, configuration: &'tree Configuration) -> Self {
        let mut builder = Self {
            tree,
            configuration,
            spacing: vec![None; tree.tokens.len()],
            quotes: tree
                .tokens
                .iter()
                .map(|token| {
                    (token.kind == TokenKind::QuotedString)
                        .then(|| quote(token.bytes(tree.source), configuration.quote_style))
                })
                .collect(),
        };

        for node in &tree.nodes {
            match &node.kind {
                NodeKind::Unary { operator, operand } => {
                    builder.spacing[tree.node(*operand).tokens.start.get()] = Some(usize::from(
                        tree.token(*operator).kind == TokenKind::Keyword(Keyword::Not),
                    ));
                }

                NodeKind::MethodCall { colon, method, .. }
                | NodeKind::FunctionName {
                    colon: Some(colon),
                    method: Some(method),
                    ..
                } => {
                    builder.spacing[colon.get()] = Some(0);
                    builder.spacing[tree.node(*method).tokens.start.get()] = Some(0);
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
                    builder.spacing[opening.get()] = Some(0);

                    if let Some(next) = builder.next(opening.get() + 1, node.tokens.end.get()) {
                        builder.spacing[next] = Some(0);
                    }

                    if let Some(closing) = closing {
                        builder.spacing[closing.get()] = Some(0);
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
                    builder.spacing[ellipsis.get()] = Some(0);
                }

                NodeKind::VariadicType { annotation, .. } => {
                    builder.spacing[tree.node(*annotation).tokens.start.get()] = Some(0);
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
                    builder.spacing[opening.get()] = Some(1);
                }

                _ => {}
            }
        }

        builder
    }

    fn next(&self, start: usize, end: usize) -> Option<usize> {
        (start..end).find(|&position| {
            !matches!(
                self.tree.tokens[position].kind,
                TokenKind::Whitespace | TokenKind::EndOfFile
            )
        })
    }

    fn previous(&self, start: usize, end: usize) -> Option<usize> {
        (start..end).rev().find(|&position| {
            !matches!(
                self.tree.tokens[position].kind,
                TokenKind::Whitespace | TokenKind::EndOfFile
            )
        })
    }

    fn lines(&self, start: usize, end: usize) -> usize {
        self.tree.source[start..end]
            .split(|&byte| byte == b'\n')
            .take(3)
            .count()
            - 1
    }

    fn comments(&self, range: Range<usize>) -> bool {
        self.tree.tokens[range]
            .iter()
            .any(|token| matches!(token.kind, TokenKind::Comment | TokenKind::BlockComment))
    }

    fn blank(&self, start: usize, end: usize) -> bool {
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

    fn gap(&self, left: usize, right: usize) -> Vec<Document> {
        let mut result = Vec::new();
        let mut previous = left;

        for position in left + 1..right {
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

    fn space(&self, left: usize, right: usize) -> bool {
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

    fn trivia(&self, range: Range<usize>, leading: bool) -> Vec<Document> {
        let mut result = Vec::new();
        let mut previous = None;

        for position in range {
            let token = self.tree.tokens[position];

            if !matches!(token.kind, TokenKind::Comment | TokenKind::BlockComment) {
                continue;
            }

            if let Some(previous) = previous {
                let prior: &vermis::token::Token = &self.tree.tokens[previous];
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

    fn sequence(&self, range: Range<usize>, children: &[NodeIndex]) -> Vec<Document> {
        let mut result = Vec::new();
        let mut position = range.start;
        let mut previous = None;

        for &child in children {
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

    fn tokens(
        &self,
        range: Range<usize>,
        previous: &mut Option<usize>,
        result: &mut Vec<Document>,
    ) {
        for position in range {
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

    fn node(&self, index: NodeIndex) -> Vec<Document> {
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

    fn scope(&self, index: NodeIndex) -> Vec<Document> {
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

    fn conditional(
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

    fn inline_end(&self, start: usize, end: usize) -> usize {
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

    fn block(&self, entries: &[ListEntry], range: Range<usize>, root: bool) -> Vec<Document> {
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
            units.push((entry.node, content, blank, self.require(entry.node)));
            position = tail;
        }

        if self.configuration.requires.order != Order::Preserve {
            let mut start = 0;

            while start < units.len() {
                if units[start].3.is_none() {
                    start += 1;
                    continue;
                }

                let mut end = start + 1;

                while end < units.len() && units[end].3.is_some() {
                    end += 1;
                }

                units[start..end].sort_by(|left, right| {
                    let left_path = left.3.as_deref().unwrap_or_default();
                    let right_path = right.3.as_deref().unwrap_or_default();

                    if self.configuration.requires.order == Order::Grouped {
                        self.group(left_path)
                            .cmp(&self.group(right_path))
                            .then_with(|| left_path.cmp(right_path))
                    } else {
                        left_path.cmp(right_path)
                    }
                });

                start = end;
            }
        }

        let mut previous_path = None;

        for (offset, (_, content, blank, path)) in units.into_iter().enumerate() {
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

    fn require(&self, index: NodeIndex) -> Option<String> {
        let NodeKind::Local {
            bindings, values, ..
        } = &self.tree.node(index).kind
        else {
            return None;
        };

        if self.tree.list(bindings).len() != 1 || self.tree.list(values).len() != 1 {
            return None;
        }

        let NodeKind::Call { callee, arguments } =
            &self.tree.node(self.tree.list(values)[0].node).kind
        else {
            return None;
        };

        if !matches!(self.tree.node(*callee).kind, NodeKind::Name { .. })
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

        let NodeKind::String { token } = self.tree.node(self.tree.list(values)[0].node).kind else {
            return None;
        };

        if self.tree.token(token).kind != TokenKind::QuotedString {
            return None;
        }

        let bytes = self.tree.token(token).bytes(self.tree.source);
        let content = &bytes[1..bytes.len() - 1];

        if content.contains(&b'\\') {
            return None;
        }

        String::from_utf8(content.to_vec()).ok()
    }

    fn group(&self, path: &str) -> usize {
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
                    glob::Pattern::new(pattern).is_ok_and(|pattern| pattern.matches(path))
                }),
            })
            .unwrap_or(self.configuration.requires.groups.len())
    }

    fn table(
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

    fn arguments(
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

        let pressed = documents
            .iter()
            .any(|document| self.width(document).is_none());

        let mut interior = vec![Document::Soft(0)];
        let mut position = opening.get() + 1;

        for (offset, (entry, document)) in entries.iter().zip(documents).enumerate() {
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
            layout: if pressed {
                Layout::Pressed
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

    fn width(&self, document: &[Document]) -> Option<usize> {
        document.iter().try_fold(0usize, |width, item| {
            let length = match item {
                Document::Token(index) => {
                    let bytes = self.quotes[index.get()]
                        .as_deref()
                        .unwrap_or_else(|| self.tree.token(*index).bytes(self.tree.source));

                    if bytes.contains(&b'\n') {
                        return None;
                    }

                    bytes.len()
                }

                Document::Symbol(_) | Document::Space => 1,
                Document::Soft(spaces) => *spaces,
                Document::TrailingComma => 0,

                Document::Line(_)
                | Document::Group {
                    layout: Layout::Vertical,
                    ..
                } => return None,

                Document::Group { content, .. } | Document::Indent { content, .. } => {
                    self.width(content)?
                }
            };

            width.checked_add(length)
        })
    }
}

struct Renderer<'tree, 'source> {
    builder: &'tree Builder<'tree, 'source>,
    emitter: Emitter<'source>,
    replacements: &'tree [Option<(Tree<'source>, TokenIndex)>],
    indentation: usize,
    column: usize,
    lines: usize,
    space: bool,
    emitted: bool,
}

impl Renderer<'_, '_> {
    fn render(&mut self, document: &[Document], flat: bool) {
        for item in document {
            match item {
                Document::Token(index) => {
                    self.separate();

                    let bytes = if let Some((tree, token)) = &self.replacements[index.get()] {
                        self.emitter.token(tree, *token);

                        tree.token(*token).bytes(tree.source)
                    } else {
                        self.emitter.token(self.builder.tree, *index);

                        self.builder
                            .tree
                            .token(*index)
                            .bytes(self.builder.tree.source)
                    };

                    self.column = bytes
                        .iter()
                        .rposition(|&byte| byte == b'\n')
                        .map_or(self.column + bytes.len(), |position| {
                            bytes.len() - position - 1
                        });

                    self.emitted = true;
                }

                Document::Symbol(symbol) => {
                    self.separate();
                    self.emitter.symbol(*symbol);
                    self.column += 1;
                    self.emitted = true;
                }

                Document::Space => self.space = true,

                Document::Line(lines) => {
                    self.lines = self.lines.max(*lines);
                    self.space = false;
                }

                Document::Soft(spaces) => {
                    if flat {
                        self.space |= *spaces > 0;
                    } else {
                        self.lines = self.lines.max(1);
                        self.space = false;
                    }
                }

                Document::TrailingComma => {
                    if !flat {
                        self.separate();
                        self.emitter.symbol(Symbol::Comma);
                        self.column += 1;
                    }
                }

                Document::Indent {
                    conditional,
                    content,
                } => {
                    let increment = usize::from(!conditional || !flat);
                    self.indentation += increment;
                    self.render(content, flat);
                    self.indentation -= increment;
                }

                Document::Group { layout, content } => {
                    let column = if self.lines > 0 || !self.emitted {
                        self.indentation * self.builder.configuration.indent_width.get()
                    } else {
                        self.column + usize::from(self.space)
                    };

                    let flat = match layout {
                        Layout::Vertical => false,
                        Layout::Pressed => true,

                        Layout::Fit => {
                            flat || self.builder.width(content).is_some_and(|width| {
                                column + width <= self.builder.configuration.width.get()
                            })
                        }
                    };

                    self.render(content, flat);
                }
            }
        }
    }

    fn separate(&mut self) {
        if self.lines > 0 || !self.emitted {
            if self.emitted {
                for _ in 0..self.lines {
                    self.emitter.newline();
                }
            }

            match self.builder.configuration.indent_style {
                IndentStyle::Tabs => self.emitter.tabs(self.indentation),

                IndentStyle::Spaces => self
                    .emitter
                    .spaces(self.indentation * self.builder.configuration.indent_width.get()),
            }

            self.column = self.indentation * self.builder.configuration.indent_width.get();
        } else if self.space {
            self.emitter.spaces(1);
            self.column += 1;
        }

        self.lines = 0;
        self.space = false;
    }
}

pub(crate) fn format(source: &[u8], configuration: &Configuration) -> Result<Vec<u8>, String> {
    configuration.validate()?;
    let tree = vermis::parse(source);

    if !tree.diagnostics.is_empty() {
        return Err(tree
            .diagnostics
            .iter()
            .map(|diagnostic| {
                format!(
                    "{}..{}: {}",
                    diagnostic.span.start, diagnostic.span.end, diagnostic.message
                )
            })
            .collect::<Vec<_>>()
            .join("\n"));
    }

    let builder = Builder::new(&tree, configuration);
    let document = builder.node(tree.root);

    let line_ending = match configuration.line_ending {
        LineEnding::Lf => EmittedLineEnding::LineFeed,
        LineEnding::Crlf => EmittedLineEnding::CarriageReturnLineFeed,
    };

    let replacement_sources = builder
        .quotes
        .iter()
        .map(|bytes| {
            bytes.as_ref().map(|bytes| {
                let mut emitter = Emitter::new(line_ending);
                emitter.keyword(Keyword::Return);
                emitter.spaces(1);
                let mut source = emitter.finish().into_owned();
                source.extend_from_slice(bytes);

                source
            })
        })
        .collect::<Vec<_>>();

    let replacements = replacement_sources
        .iter()
        .map(|source| {
            source
                .as_deref()
                .map(|source| {
                    let tree = vermis::parse(source);

                    if !tree.diagnostics.is_empty() {
                        return Err(
                            "quoted-string transformation produced invalid syntax".to_owned()
                        );
                    }

                    let token = tree
                        .tokens
                        .iter()
                        .position(|token| token.kind == TokenKind::QuotedString)
                        .ok_or_else(|| {
                            "quoted-string transformation did not produce a string token".to_owned()
                        })?;

                    Ok((tree, TokenIndex::new(token)))
                })
                .transpose()
        })
        .collect::<Result<Vec<_>, String>>()?;

    let mut renderer = Renderer {
        builder: &builder,
        emitter: Emitter::from_tree(&tree, line_ending),
        replacements: &replacements,
        indentation: 0,
        column: 0,
        lines: 0,
        space: false,
        emitted: false,
    };

    renderer.render(&document, false);

    if renderer.emitted {
        renderer.emitter.newline();
    }

    Ok(renderer.emitter.finish().into_owned())
}

fn quote(bytes: &[u8], style: QuoteStyle) -> Vec<u8> {
    if style == QuoteStyle::Preserve {
        return bytes.to_vec();
    }

    let mut content = Vec::new();
    let mut position = 1;

    while position + 1 < bytes.len() {
        let byte = bytes[position];

        if byte == b'\\' && position + 2 < bytes.len() {
            let next = bytes[position + 1];

            if matches!(next, b'\'' | b'"') {
                content.push(next);
            } else {
                content.extend_from_slice(&bytes[position..position + 2]);
            }

            position += 2;
        } else {
            content.push(byte);
            position += 1;
        }
    }

    let single = content.contains(&b'\'');
    let double = content.contains(&b'"');

    let delimiter = match style {
        QuoteStyle::Single => b'\'',
        QuoteStyle::Double | QuoteStyle::Preserve => b'"',

        QuoteStyle::PreferSingle => {
            if single && !double {
                b'"'
            } else {
                b'\''
            }
        }

        QuoteStyle::PreferDouble => {
            if double && !single {
                b'\''
            } else {
                b'"'
            }
        }
    };

    let mut result = vec![delimiter];
    let mut position = 0;

    while position < content.len() {
        let byte = content[position];

        if byte == b'\\' && position + 1 < content.len() {
            result.extend_from_slice(&content[position..position + 2]);
            position += 2;
        } else {
            if byte == delimiter {
                result.push(b'\\');
            }

            result.push(byte);
            position += 1;
        }
    }

    result.push(delimiter);

    result
}
