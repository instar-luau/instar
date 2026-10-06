use super::super::document::Document;
use super::Builder;
use instar_analysis::Reason;
use std::ops::Range;
use vermis::token::{Keyword, Symbol, TokenKind};
use vermis::tree::{NodeIndex, NodeKind, TokenIndex};

impl Builder<'_, '_> {
    pub(super) fn prepare_spacing(&mut self) -> Result<(), Reason> {
        let tree = self.tree;

        for node in &tree.nodes {
            if let Some(reason) = self.options.interrupted(self.started) {
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
