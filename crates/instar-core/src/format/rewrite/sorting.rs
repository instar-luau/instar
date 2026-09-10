use crate::format::{
    Options,
    configuration::{Grouping, Indexer, Order},
    emit::Emitter,
};
use std::{cmp::Ordering, io, ops::Range};
use vermis::{Kind, Parts, TokenKind, Tree, View};

pub(super) fn sort(
    source: &str,
    tree: &Tree<'_>,
    options: &Options,
    held: &[Range<usize>],
) -> io::Result<String> {
    if !options.sort_requires.enabled
        && options.sort_tables.order == Order::None
        && options.sort_table_types.order == Order::None
    {
        return Ok(source.to_owned());
    }

    let root = tree
        .root_view()
        .ok_or_else(|| io::Error::other("missing syntax root"))?;

    Sorter {
        source,
        tree,
        options,
        held,
    }
    .render(root)
}

struct Sorter<'tree, 'source> {
    source: &'source str,
    tree: &'tree Tree<'source>,
    options: &'tree Options,
    held: &'tree [Range<usize>],
}

impl<'tree, 'source> Sorter<'tree, 'source> {
    fn bounds(&self, view: View<'_, '_>) -> Range<usize> {
        if view.kind() == Kind::Root {
            return 0..self.source.len();
        }

        if view.kind() != Kind::Block {
            return view.span().start..view.span().end;
        }

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
            .rfind(|token| significant(token.kind) && token.span.end <= view.span().start)
            .map_or(0, |token| token.span.end);

        let end = self
            .tree
            .tokens
            .iter()
            .find(|token| significant(token.kind) && token.span.start >= view.span().end)
            .map_or(self.source.len(), |token| token.span.start);

        start..end
    }

    fn render(&self, view: View<'tree, 'source>) -> io::Result<String> {
        let range = self.bounds(view);

        if self
            .held
            .iter()
            .any(|held| held.start <= range.start && held.end >= range.end)
        {
            return Ok(self.source[range].to_owned());
        }

        let mut children = view
            .children()
            .map(|child| Ok((child, self.render(child)?)))
            .collect::<io::Result<Vec<_>>>()?;

        if view.kind() == Kind::Block && self.options.sort_requires.enabled {
            return Ok(self.requires(range, children));
        }

        if matches!(view.kind(), Kind::Table | Kind::TypeTable) {
            self.properties(view, &mut children)?;
        }

        let mut output = String::new();
        let mut cursor = range.start;

        for (child, text) in children {
            let child = self.bounds(child);

            if child.start < cursor || child.end > range.end {
                return Err(io::Error::other(
                    "overlapping syntax ranges in format rewrite",
                ));
            }

            output.push_str(&self.source[cursor..child.start]);
            output.push_str(&text);
            cursor = child.end;
        }

        output.push_str(&self.source[cursor..range.end]);

        Ok(output)
    }

    fn properties(
        &self,
        view: View<'tree, 'source>,
        children: &mut [(View<'tree, 'source>, String)],
    ) -> io::Result<()> {
        let typed = view.kind() == Kind::TypeTable;

        let order = if typed {
            self.options.sort_table_types.order
        } else {
            self.options.sort_tables.order
        };

        if order == Order::None
            || typed && !self.options.table_types.enabled
            || self
                .held
                .iter()
                .any(|range| range.start < view.span().end && range.end > view.span().start)
            || self.tree.tokens.iter().any(|token| {
                token.span.start >= view.span().start
                    && token.span.end <= view.span().end
                    && matches!(token.kind, TokenKind::Comment | TokenKind::BlockComment)
            })
        {
            return Ok(());
        }

        let emitter = Emitter::new(self.source, self.tree, self.options, self.held);
        let mut entries = Vec::new();

        for (field, text) in children.iter() {
            let key = match field.parts() {
                Some(Parts::TableField {
                    key: Some(key),
                    indexed,
                    ..
                }) if !indexed || key.kind() == Kind::String => key,

                Some(Parts::TypeField { key, .. }) => key,
                _ => return Ok(()),
            };

            let key = key.text().to_string();
            let key = key.trim_matches(['\'', '"']).to_owned();

            if typed
                && field.kind() != Kind::TypeIndexer
                && !key
                    .chars()
                    .all(|character| character.is_alphanumeric() || character == '_')
            {
                return Ok(());
            }

            let key = if field.kind() == Kind::TypeIndexer {
                String::new()
            } else {
                key
            };

            let width = emitter.node(*field)?.width().unwrap_or(usize::MAX);
            entries.push((key, width, field.kind() == Kind::TypeIndexer, text.clone()));
        }

        entries.sort_by(|left, right| {
            let position = if typed {
                self.options.sort_table_types.position()
            } else {
                Indexer::Sorted
            };

            let indexer = match position {
                Indexer::First => right.2.cmp(&left.2),
                Indexer::Last => left.2.cmp(&right.2),
                Indexer::Sorted => Ordering::Equal,
            };

            indexer
                .then_with(|| match order {
                    Order::Ascending => left.0.len().cmp(&right.0.len()),
                    Order::Descending => right.0.len().cmp(&left.0.len()),
                    Order::SizeAscending => left.1.cmp(&right.1),
                    Order::SizeDescending => right.1.cmp(&left.1),
                    _ => Ordering::Equal,
                })
                .then_with(|| left.0.cmp(&right.0))
        });

        for ((_, text), entry) in children.iter_mut().zip(entries) {
            *text = entry.3;
        }

        Ok(())
    }

    fn require(&self, view: View<'_, '_>) -> Option<String> {
        if self
            .held
            .iter()
            .any(|range| range.start < view.span().end && range.end > view.span().start)
        {
            return None;
        }

        let Parts::Local { bindings, values } = view.parts()? else {
            return None;
        };

        if bindings.count() != 1 || values.clone().count() != 1 {
            return None;
        }

        let Parts::Call { callee, arguments } = values.clone().next()?.parts()? else {
            return None;
        };

        if callee.kind() != Kind::Name
            || callee.text() != "require"
            || arguments.children().count() != 1
        {
            return None;
        }

        let value = arguments.children().next()?;

        (value.kind() == Kind::String).then(|| {
            value
                .text()
                .to_string()
                .trim_matches(['\'', '"'])
                .to_owned()
        })
    }

    fn requires(
        &self,
        range: Range<usize>,
        children: Vec<(View<'tree, 'source>, String)>,
    ) -> String {
        if children.is_empty() {
            return self.source[range].to_owned();
        }

        let mut pieces: Vec<Piece> = Vec::new();
        let mut cursor = range.start;
        let mut prefix = String::new();

        for (view, text) in children {
            let gap = &self.source[cursor..view.span().start];
            let mut leading = gap;

            if let Some(previous) = pieces.last_mut() {
                if let Some(newline) = gap.find('\n') {
                    previous.trailing.push_str(&gap[..newline]);
                    leading = &gap[newline + 1..];
                } else {
                    previous.trailing.push_str(gap);
                    leading = "";
                }
            } else {
                while let Some((line, rest)) = leading.split_once('\n') {
                    if line.trim_start().starts_with("--!")
                        || !line.trim().is_empty()
                            && !prefix.contains('\n')
                            && range.start != 0
                            && !leading.starts_with('\n')
                    {
                        prefix.push_str(line);
                        prefix.push('\n');
                        leading = rest;
                    } else {
                        break;
                    }
                }
            }

            let blank = leading.starts_with(['\n', '\r']);

            pieces.push(Piece {
                leading: leading.trim_start_matches(['\n', '\r']).to_owned(),
                text,
                trailing: String::new(),
                key: self.require(view),
                blank,
            });

            cursor = view.span().end;
        }

        let tail = &self.source[cursor..range.end];
        let (trailing, tail) = tail.split_once('\n').unwrap_or((tail, ""));

        pieces
            .last_mut()
            .expect("statements exist")
            .trailing
            .push_str(trailing);

        let mut start = 0;

        while start < pieces.len() {
            if pieces[start].key.is_none() {
                start += 1;
                continue;
            }

            let mut end = start + 1;

            while end < pieces.len() && pieces[end].key.is_some() && !pieces[end].blank {
                end += 1;
            }

            let blank = pieces[start].blank;

            pieces[start..end].sort_by_key(|piece| {
                (
                    if self.options.sort_requires.grouping == Grouping::ByKind {
                        kind(piece.key.as_deref().unwrap_or_default())
                    } else {
                        0
                    },
                    piece.key.clone(),
                )
            });

            for piece in &mut pieces[start..end] {
                piece.blank = false;
            }

            pieces[start].blank = blank;

            if self.options.sort_requires.grouping == Grouping::ByKind {
                for index in start + 1..end {
                    if kind(pieces[index - 1].key.as_deref().unwrap_or_default())
                        != kind(pieces[index].key.as_deref().unwrap_or_default())
                    {
                        pieces[index].blank = true;
                    }
                }
            }

            start = end;
        }

        let mut output = prefix;

        for piece in pieces {
            if piece.blank {
                output.push('\n');
            }

            output.push_str(&piece.leading);
            output.push_str(&piece.text);
            output.push_str(&piece.trailing);
            output.push('\n');
        }

        output.push_str(tail);

        output
    }
}

struct Piece {
    leading: String,
    text: String,
    trailing: String,
    key: Option<String>,
    blank: bool,
}
fn kind(path: &str) -> usize {
    if path.starts_with('@') {
        0
    } else if path.starts_with('.') {
        2
    } else {
        1
    }
}
