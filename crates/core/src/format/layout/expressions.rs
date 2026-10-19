use std::ops::Range;

use vermis::{
    token::{Keyword, Symbol, TokenKind},
    tree::{NodeIndex, NodeKind, TokenIndex},
};

use super::{
    document::{Document, Layout},
    plan::Builder,
};

impl Builder<'_, '_> {
    pub(super) fn binary(
        &self,
        left: NodeIndex,
        operator: TokenIndex,
        right: NodeIndex,
    ) -> Vec<Document> {
        let family = operator_family(self.tree.token(operator).kind);
        let mut terms = vec![(operator, right)];
        let mut first = left;

        while let NodeKind::Binary {
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
        } = &self.tree.node(first).kind
        {
            if operator_family(self.tree.token(*operator).kind) != family {
                break;
            }

            terms.push((*operator, *right));
            first = *left;
        }

        let mut content = self.node(first);
        let mut continuation = Vec::new();
        let mut previous = self.tree.node(first).tokens.end.get() - 1;
        let mut comments = false;

        for (operator, right) in terms.into_iter().rev() {
            let start = self.tree.node(right).tokens.start.get();
            comments |= self.comments(previous + 1..start);
            continuation.extend(self.breakpoint(previous, operator.get(), 1));
            continuation.push(Document::Token(operator));
            continuation.extend(self.gap(operator.get(), start));
            continuation.extend(self.node(right));
            previous = self.tree.node(right).tokens.end.get() - 1;
        }

        content.push(Document::Indent {
            conditional: true,
            content: continuation,
        });

        vec![Document::Group {
            layout: if comments {
                Layout::Fit
            } else {
                Layout::Arguments
            },
            content,
        }]
    }

    pub(super) fn delimited(
        &self,
        opening: TokenIndex,
        expression: NodeIndex,
        closing: TokenIndex,
        spaces: usize,
    ) -> Vec<Document> {
        self.framed(
            opening,
            self.tree.node(expression).tokens.start.get()
                ..self.tree.node(expression).tokens.end.get(),
            closing,
            spaces,
            self.node(expression),
        )
    }

    pub(super) fn wrapped(
        &self,
        index: NodeIndex,
        opening: TokenIndex,
        expression: NodeIndex,
        closing: TokenIndex,
    ) -> Vec<Document> {
        let node = self.tree.node(index);
        let children = self.tree.children(index);
        let mut result = self.sequence(node.tokens.start.get()..opening.get() + 1, &children);

        result
            .pop()
            .expect("internal error: missing opening delimiter");

        result.extend(self.delimited(opening, expression, closing, 0));

        let mut suffix = self.sequence(closing.get()..node.tokens.end.get(), &children);
        suffix.remove(0);
        result.extend(suffix);

        result
    }

    pub(super) fn prefixed(&self, prefix: TokenIndex, expression: NodeIndex) -> Vec<Document> {
        let mut continuation = self.breakpoint(
            prefix.get(),
            self.tree.node(expression).tokens.start.get(),
            1,
        );

        let condition = self.tree.token(prefix).kind == TokenKind::Keyword(Keyword::Until);

        continuation.extend(if condition {
            self.condition(expression)
        } else {
            self.node(expression)
        });

        vec![Document::Group {
            layout: if condition {
                Layout::Fit
            } else {
                Layout::Arguments
            },
            content: vec![
                Document::Token(prefix),
                Document::Indent {
                    conditional: true,
                    content: continuation,
                },
            ],
        }]
    }

    pub(super) fn condition(&self, index: NodeIndex) -> Vec<Document> {
        match &self.tree.node(index).kind {
            NodeKind::Binary {
                left,
                operator,
                right,
            } if matches!(
                self.tree.token(*operator).kind,
                TokenKind::Keyword(Keyword::And | Keyword::Or)
            ) =>
            {
                let mut content = self.condition(*left);

                content.extend(self.breakpoint(
                    self.tree.node(*left).tokens.end.get() - 1,
                    operator.get(),
                    1,
                ));

                content.push(Document::Token(*operator));
                content.extend(self.gap(operator.get(), self.tree.node(*right).tokens.start.get()));
                content.extend(self.condition(*right));

                content
            }

            NodeKind::Group {
                opening,
                expression,
                closing: Some(closing),
            } => self.framed(
                *opening,
                self.tree.node(*expression).tokens.start.get()
                    ..self.tree.node(*expression).tokens.end.get(),
                *closing,
                0,
                self.condition(*expression),
            ),

            _ => self.node(index),
        }
    }

    pub(super) fn framed(
        &self,
        opening: TokenIndex,
        interior: Range<usize>,
        closing: TokenIndex,
        spaces: usize,
        content: Vec<Document>,
    ) -> Vec<Document> {
        let mut continuation = self.breakpoint(opening.get(), interior.start, spaces);
        continuation.extend(content);

        let mut content = vec![
            Document::Token(opening),
            Document::Indent {
                conditional: true,
                content: continuation,
            },
        ];

        content.extend(self.breakpoint(interior.end - 1, closing.get(), spaces));
        content.push(Document::Token(closing));

        vec![Document::Group {
            layout: Layout::Fit,
            content,
        }]
    }

    pub(super) fn chain(&self, mut index: NodeIndex) -> Option<Vec<Document>> {
        let mut spine = Vec::new();
        let mut calls = 0;

        loop {
            let receiver = match &self.tree.node(index).kind {
                NodeKind::MethodCall { receiver, .. } => {
                    calls += 1;

                    *receiver
                }

                NodeKind::Call { callee, .. } => {
                    calls += 1;

                    *callee
                }

                NodeKind::Field { receiver, .. } | NodeKind::Index { receiver, .. } => *receiver,
                NodeKind::Instantiate { expression, .. } => *expression,
                _ => break,
            };

            spine.push((index, receiver));
            index = receiver;
        }

        if calls < 2 {
            return None;
        }

        spine.reverse();

        let mut content = self.node(index);
        let mut continuation = Vec::new();
        let mut invoked = false;

        for (offset, entry) in spine.iter().enumerate() {
            let (index, receiver) = *entry;
            let node = self.tree.node(index);

            let (start, split) = match &node.kind {
                NodeKind::MethodCall { colon, .. } => (colon.get(), true),

                NodeKind::Call { callee, arguments } => {
                    let mut callee = *callee;

                    while let NodeKind::Instantiate { expression, .. } =
                        &self.tree.node(callee).kind
                    {
                        callee = *expression;
                    }

                    (
                        self.tree.node(*arguments).tokens.start.get(),
                        invoked && !matches!(self.tree.node(callee).kind, NodeKind::Field { .. }),
                    )
                }

                NodeKind::Field { dot, .. } => (
                    dot.get(),
                    spine[offset + 1..]
                        .iter()
                        .find(|(index, _)| {
                            !matches!(self.tree.node(*index).kind, NodeKind::Instantiate { .. })
                        })
                        .is_some_and(|(index, _)| {
                            matches!(self.tree.node(*index).kind, NodeKind::Call { .. })
                        }),
                ),

                NodeKind::Index { opening, .. } => (opening.get(), false),

                NodeKind::Instantiate { arguments, .. } => {
                    (self.tree.node(*arguments).tokens.start.get(), false)
                }

                _ => unreachable!("internal error: invalid call chain"),
            };

            let target = if split || !continuation.is_empty() {
                &mut continuation
            } else {
                &mut content
            };

            let previous = self.tree.node(receiver).tokens.end.get() - 1;

            target.extend(if split {
                self.breakpoint(previous, start, 0)
            } else {
                self.gap(previous, start)
            });

            target.extend(match &node.kind {
                NodeKind::Index {
                    opening,
                    key,
                    closing: Some(closing),
                    ..
                } => self.delimited(*opening, *key, *closing, 0),

                _ => self.sequence(start..node.tokens.end.get(), &self.tree.children(index)),
            });

            invoked |= matches!(
                node.kind,
                NodeKind::Call { .. } | NodeKind::MethodCall { .. }
            );
        }

        content.push(Document::Indent {
            conditional: true,
            content: continuation,
        });

        Some(vec![Document::Group {
            layout: Layout::Arguments,
            content,
        }])
    }

    pub(super) fn selection(&self, mut index: NodeIndex) -> Vec<Document> {
        let mut content = Vec::new();
        let mut continuation = Vec::new();

        while let NodeKind::Conditional {
            keyword,
            condition,
            then: Some(then),
            truthy,
            else_keyword,
            falsy,
        } = &self.tree.node(index).kind
        {
            let condition_node = self.tree.node(*condition);
            let mut header = self.breakpoint(keyword.get(), condition_node.tokens.start.get(), 1);
            header.extend(self.condition(*condition));

            let mut header = vec![Document::Indent {
                conditional: true,
                content: header,
            }];

            header.extend(self.breakpoint(condition_node.tokens.end.get() - 1, then.get(), 1));
            header.push(Document::Token(*then));

            if content.is_empty() {
                header = vec![Document::Indent {
                    conditional: true,
                    content: header,
                }];
            }

            header.insert(0, Document::Token(*keyword));

            let header = Document::Group {
                layout: Layout::Fit,
                content: header,
            };

            if content.is_empty() {
                content.push(header);
            } else {
                continuation.push(header);
            }

            let truthy_node = self.tree.node(*truthy);
            let mut value = self.breakpoint(then.get(), truthy_node.tokens.start.get(), 1);
            value.extend(self.node(*truthy));

            continuation.push(Document::Indent {
                conditional: true,
                content: value,
            });

            if let Some(otherwise) = else_keyword {
                continuation.extend(self.breakpoint(
                    truthy_node.tokens.end.get() - 1,
                    otherwise.get(),
                    1,
                ));

                continuation.push(Document::Token(*otherwise));

                let mut value = self.breakpoint(
                    otherwise.get(),
                    self.tree.node(*falsy).tokens.start.get(),
                    1,
                );

                value.extend(self.node(*falsy));

                continuation.push(Document::Indent {
                    conditional: true,
                    content: value,
                });

                break;
            }

            continuation.extend(self.breakpoint(
                truthy_node.tokens.end.get() - 1,
                self.tree.node(*falsy).tokens.start.get(),
                1,
            ));

            index = *falsy;
        }

        content.push(Document::Indent {
            conditional: true,
            content: continuation,
        });

        vec![Document::Group {
            layout: Layout::Fit,
            content,
        }]
    }

    fn breakpoint(&self, left: usize, right: usize, spaces: usize) -> Vec<Document> {
        if self.comments(left + 1..right) {
            self.gap(left, right)
        } else {
            vec![Document::Soft(spaces)]
        }
    }
}

fn operator_family(kind: TokenKind) -> TokenKind {
    match kind {
        TokenKind::Symbol(Symbol::Add | Symbol::Subtract) => TokenKind::Symbol(Symbol::Add),

        TokenKind::Symbol(
            Symbol::Multiply | Symbol::Divide | Symbol::FloorDivide | Symbol::Modulo,
        ) => TokenKind::Symbol(Symbol::Multiply),

        _ => kind,
    }
}
