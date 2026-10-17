use std::iter;

use vermis::{
    token::Symbol,
    tree::{ListEntry, NodeIndex, TokenIndex},
};

use super::{
    document::{Document, Layout},
    plan::Builder,
};

impl Builder<'_, '_> {
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
