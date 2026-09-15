use super::{Document, DocumentBuilder, Kind, TokenKind, View, io};
use crate::project::configuration::format::Gaps;

impl<'tree, 'source> DocumentBuilder<'tree, 'source> {
    pub(super) fn boundaries(&self, body: View<'tree, 'source>) -> (usize, usize) {
        let significant = |kind| {
            !matches!(
                kind,
                TokenKind::Whitespace | TokenKind::Comment | TokenKind::BlockComment
            )
        };

        let start = self
            .tree
            .tokens
            .iter()
            .rfind(|token| significant(token.kind) && token.span.end <= body.span().start)
            .map_or(body.span().start, |token| token.span.end);

        let end = self
            .tree
            .tokens
            .iter()
            .find(|token| significant(token.kind) && token.span.start >= body.span().end)
            .map_or(body.span().end, |token| token.span.start);

        (start, end)
    }

    pub(super) fn body(&self, body: View<'tree, 'source>) -> io::Result<Document<'source>> {
        let (mut start, end) = self.boundaries(body);
        let mut prefix = Document::text("");

        if let Some(comment) = self.tree.tokens.iter().find(|token| {
            token.span.start >= start
                && token.span.end <= end
                && matches!(token.kind, TokenKind::Comment | TokenKind::BlockComment)
        }) && !self.source[start..comment.span.start].contains('\n')
        {
            prefix = Document::sequence([
                Document::text(" "),
                Document::text(&self.source[comment.span.start..comment.span.end]),
            ]);

            start = comment.span.end;
        }

        if body.children().next().is_none() && self.gap(start, end).is_empty() {
            return Ok(prefix);
        }

        let first = self
            .tree
            .tokens
            .iter()
            .find(|token| {
                token.kind != TokenKind::Whitespace
                    && token.span.start >= start
                    && token.span.start < end
            })
            .map_or(end, |token| token.span.start);

        let last = self
            .tree
            .tokens
            .iter()
            .rfind(|token| {
                token.kind != TokenKind::Whitespace
                    && token.span.end <= end
                    && token.span.end > start
            })
            .map_or(start, |token| token.span.end);

        let preserve = self.options.blocks.blank_lines == Gaps::Preserve;
        let before = preserve && self.source[start..first].matches('\n').count() > 1;
        let after = preserve && self.source[last..end].matches('\n').count() > 1;

        Ok(Document::sequence([
            prefix,
            Document::sequence([
                if before {
                    Document::Blank
                } else {
                    Document::Hard
                },
                self.block(body, start, end)?,
            ])
            .indent(),
            if after {
                Document::Hard
            } else {
                Document::text("")
            },
        ]))
    }

    pub(super) fn collapsed(
        &self,
        body: View<'tree, 'source>,
    ) -> io::Result<Option<Document<'source>>> {
        let mut statements = body.children();

        let Some(statement) = statements.next() else {
            return Ok(None);
        };

        let (start, end) = self.boundaries(body);

        if statements.next().is_some()
            || !matches!(
                statement.kind(),
                Kind::Local
                    | Kind::Constant
                    | Kind::Assignment
                    | Kind::CompoundAssignment
                    | Kind::CallStatement
                    | Kind::Return
                    | Kind::Break
                    | Kind::Continue
            )
            || !self.gap(start, end).is_empty()
            || self.source[start..body.span().start].matches('\n').count() > 1
            || self
                .held
                .iter()
                .any(|range| range.start < end && range.end > start)
        {
            return Ok(None);
        }

        let statement = self.node(statement)?;

        if statement.width().is_none() {
            return Ok(None);
        }

        Ok(Some(Document::sequence([
            Document::sequence([
                Document::Line,
                statement,
                Document::text(
                    if self.options.semicolons
                        == crate::project::configuration::format::Semicolons::Always
                    {
                        ";"
                    } else {
                        ""
                    },
                ),
            ])
            .indent(),
            Document::Line,
        ])))
    }
}
