use std::{iter, ops::Range, rc::Rc};

use vermis::{
    token::{Symbol, TokenKind},
    tree::{ListEntry, NodeIndex, TokenIndex},
};

use super::{
    document::{Choice, Document, Layout, Nesting},
    plan::Builder,
};

struct List {
    content: Vec<Document>,
    has_comments: bool,
    continuation: usize,
}

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
            && !self.has_comments(opening.get() + 1..closing.get())
        {
            return vec![Document::Token(opening), Document::Token(closing)];
        }

        let is_forced = entries
            .last()
            .is_some_and(|entry| entry.separator.is_some())
            || self.has_comments(opening.get() + 1..closing.get());

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
                    if self.has_blank_line(position, node.tokens.start.get()) {
                        interior.push(Document::Line(2));
                    } else {
                        interior.push(Document::Soft(1));
                    }
                }

                interior.extend(self.trivia(position..node.tokens.start.get(), true));
                interior.extend(self.node(entry.node));

                if let Some(separator) = entry.separator {
                    interior.extend(self.separator(node.tokens.end.get(), separator));
                } else if offset + 1 < entries.len() {
                    interior.push(Document::Symbol(Symbol::Comma));
                }

                interior.extend(self.trivia(finish..tail, false));
                position = tail;
            }

            if self.has_comments(position..closing.get()) {
                interior.push(Document::Line(1));
                interior.extend(self.trivia(position..closing.get(), true));
            }
        }

        let content = vec![
            Document::Token(opening),
            Document::Indent {
                nesting: Nesting::Broken,
                content: iter::once(Document::Soft(1)).chain(interior).collect(),
            },
            Document::Soft(1),
            Document::Token(closing),
        ];

        vec![Document::Group {
            layout: if is_forced {
                Layout::Vertical
            } else {
                Layout::Table
            },
            content,
        }]
    }

    pub(super) fn collection(
        &self,
        opening: Range<usize>,
        closing: Range<usize>,
        entries: &[ListEntry],
        layout: Layout,
    ) -> Vec<Document> {
        if entries.is_empty() && !self.has_comments(opening.end..closing.start) {
            return self.sequence(opening.start..closing.end, &[]);
        }

        if let [entry] = entries
            && entry.separator.is_none()
            && self.is_enclosed(opening.end..closing.start, entry.node)
        {
            return self.sequence(opening.start..closing.end, &[entry.node]);
        }

        let list = self.list(opening.end..closing.start, entries);
        let mut content = self.sequence(opening, &[]);

        content.push(Document::Indent {
            nesting: Nesting::Broken,
            content: iter::once(Document::Soft(0)).chain(list.content).collect(),
        });

        content.push(Document::Soft(0));
        content.extend(self.sequence(closing, &[]));

        vec![Document::Group {
            layout: if list.has_comments {
                Layout::Fit
            } else {
                layout
            },
            content,
        }]
    }

    pub(super) fn separated(&self, range: Range<usize>, lists: &[&[ListEntry]]) -> Vec<Document> {
        let populated: Vec<_> = lists.iter().filter(|entries| !entries.is_empty()).collect();

        if let [bindings, values] = populated.as_slice() {
            let start = self.tree.node(bindings[0].node).tokens.start.get();

            let end = self
                .tree
                .node(
                    bindings
                        .last()
                        .expect("Internal error: binding list is empty.")
                        .node,
                )
                .tokens
                .end
                .get();

            let value_start = self.tree.node(values[0].node).tokens.start.get();

            let value_end = self
                .tree
                .node(
                    values
                        .last()
                        .expect("Internal error: value list is empty.")
                        .node,
                )
                .tokens
                .end
                .get();

            let mut prefix = self.sequence(range.start..start, &[]);

            if let Some(previous) = self.previous(range.start, start) {
                prefix.extend(self.gap(previous, start));
            }

            let bindings = self.list(start..end, bindings);
            let values_list = self.list(value_start..value_end, values);
            let mut connection = Vec::new();
            let mut previous = Some(end - 1);
            self.tokens(end..value_start, &mut previous, &mut connection);

            let values = if values.len() > 1 {
                connection.push(Document::Indent {
                    nesting: Nesting::Broken,
                    content: iter::once(Document::Soft(1))
                        .chain(values_list.content)
                        .collect(),
                });

                connection.extend(self.sequence(value_end..range.end, &[]));

                vec![Document::Group {
                    layout: Layout::Fit,
                    content: connection,
                }]
            } else {
                if let Some(previous) = previous {
                    connection.extend(self.gap(previous, value_start));
                }

                connection.extend(values_list.content);
                connection.extend(self.sequence(value_end..range.end, &[]));

                let inline: Rc<[Document]> = connection.clone().into();

                vec![Document::Choice(Box::new(Choice {
                    range: end..range.end,
                    has_prefix: true,
                    probe: Rc::clone(&inline),
                    inline,
                    expanded: vec![Document::Indent {
                        nesting: Nesting::Always,
                        content: iter::once(Document::Line(1)).chain(connection).collect(),
                    }]
                    .into(),
                }))]
            };

            if bindings.continuation == bindings.content.len() {
                prefix.extend(bindings.content);

                return iter::once(Document::Group {
                    layout: Layout::Fit,
                    content: prefix,
                })
                .chain(values)
                .collect();
            }

            let mut probe = prefix.clone();
            probe.extend(bindings.content.clone());

            let mut inline = vec![Document::Group {
                layout: Layout::Fit,
                content: probe.clone(),
            }];

            inline.extend(values.clone());
            let mut continued = bindings.content;
            let mut continuation = continued.split_off(bindings.continuation);
            prefix.extend(continued);

            continuation.extend(values);

            prefix.push(Document::Indent {
                nesting: Nesting::Always,
                content: continuation,
            });

            return vec![Document::Choice(Box::new(Choice {
                range,
                has_prefix: false,
                probe: probe.into(),
                inline: inline.into(),
                expanded: prefix.into(),
            }))];
        }

        let mut content = Vec::new();
        let mut position = range.start;
        let mut previous = None;
        let mut has_comments = false;

        for entries in lists.iter().filter(|entries| !entries.is_empty()) {
            let start = self.tree.node(entries[0].node).tokens.start.get();

            let end = self
                .tree
                .node(
                    entries
                        .last()
                        .expect("Internal error: expected a nonempty list.")
                        .node,
                )
                .tokens
                .end
                .get();

            self.tokens(position..start, &mut previous, &mut content);

            if let Some(previous) = previous {
                content.extend(self.gap(previous, start));
            }

            let mut list = self.list(start..end, entries);
            let continuation = list.content.split_off(list.continuation);
            content.extend(list.content);

            if !continuation.is_empty() {
                content.push(Document::Indent {
                    nesting: Nesting::Broken,
                    content: continuation,
                });
            }

            has_comments |= list.has_comments;
            previous = Some(end - 1);
            position = end;
        }

        self.tokens(position..range.end, &mut previous, &mut content);

        vec![Document::Group {
            layout: if has_comments {
                Layout::Fit
            } else {
                Layout::Arguments
            },
            content,
        }]
    }

    fn list(&self, range: Range<usize>, entries: &[ListEntry]) -> List {
        let mut list = List {
            content: Vec::new(),
            has_comments: self.has_comments(
                range.start..entries.first().map_or(range.end, |entry| {
                    self.tree.node(entry.node).tokens.start.get()
                }),
            ),
            continuation: 0,
        };

        let interior = &mut list.content;
        let mut position = range.start;

        for (offset, entry) in entries.iter().enumerate() {
            let node = self.tree.node(entry.node);

            if offset > 0 {
                interior.push(Document::Soft(1));
            }

            interior.extend(self.trivia(position..node.tokens.start.get(), true));
            interior.extend(self.node(entry.node));

            if let Some(separator) = entry.separator {
                interior.extend(self.separator(node.tokens.end.get(), separator));
            } else if offset + 1 < entries.len() {
                interior.push(Document::Symbol(Symbol::Comma));
            }

            let finish = entry
                .separator
                .map_or(node.tokens.end.get(), |separator| separator.get() + 1);

            let next = entries.get(offset + 1).map_or(range.end, |entry| {
                self.tree.node(entry.node).tokens.start.get()
            });

            list.has_comments |= self.has_comments(node.tokens.end.get()..next);

            let tail = self.inline_end(finish, next);
            interior.extend(self.trivia(finish..tail, false));
            position = tail;

            if offset == 0 {
                list.continuation = interior.len();
            }
        }

        interior.extend(self.trivia(position..range.end, true));

        list
    }

    fn separator(&self, end: usize, separator: TokenIndex) -> Vec<Document> {
        let mut gap = self.gap(end - 1, separator.get());

        if self.tree.tokens[end..separator.get()]
            .iter()
            .any(|token| token.kind == TokenKind::Comment)
        {
            gap.insert(0, Document::Symbol(Symbol::Comma));
        } else {
            gap.push(Document::Symbol(Symbol::Comma));
        }

        gap
    }
}
