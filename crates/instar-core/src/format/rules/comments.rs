use super::{Document, DocumentBuilder, Kind, TokenKind, View, io};

impl<'tree, 'source> DocumentBuilder<'tree, 'source> {
    pub(super) fn trivia(
        &self,
        start: usize,
        end: usize,
        before: bool,
        after: bool,
    ) -> Vec<Document<'source>> {
        let mut documents = Vec::new();
        let mut cursor = start;
        let mut previous = before;

        for token in self.tree.tokens.iter().filter(|token| {
            token.span.start >= start
                && token.span.end <= end
                && matches!(token.kind, TokenKind::Comment | TokenKind::BlockComment)
        }) {
            if previous {
                let gap = &self.source[cursor..token.span.start];

                documents.push(if gap.matches('\n').count() > 1 {
                    Document::Blank
                } else if gap.contains('\n') {
                    Document::Hard
                } else {
                    Document::text(" ")
                });
            }

            documents.push(Document::text(
                &self.source[token.span.start..token.span.end],
            ));

            cursor = token.span.end;
            previous = true;
        }

        if previous && after {
            documents.push(if self.source[cursor..end].matches('\n').count() > 1 {
                Document::Blank
            } else {
                Document::Hard
            });
        }

        documents
    }

    pub(super) fn commented(&self, view: View<'tree, 'source>) -> io::Result<Document<'source>> {
        let table = view.kind() == Kind::Table;

        let (opening, closing) = if table { ("{", "}") } else { ("(", ")") };

        let mut children = view.children().peekable();
        let mut documents = vec![Document::Hard];
        let mut cursor = view.span().start + 1;
        let mut previous = false;

        while let Some(child) = children.next() {
            documents.extend(self.trivia(cursor, child.span().start, previous, true));
            documents.push(self.node(child)?);

            if children.peek().is_some() || table && self.options.trailing_separator {
                documents.push(Document::text(","));
            }

            cursor = child.span().end;
            previous = true;
        }

        documents.extend(self.trivia(cursor, view.span().end - 1, previous, false));

        Ok(Document::sequence([
            Document::text(opening),
            Document::sequence(documents).indent(),
            Document::Hard,
            Document::text(closing),
        ]))
    }
}
