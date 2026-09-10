use std::borrow::Cow;

#[derive(Clone)]
pub(crate) enum Document<'source> {
    Text(Cow<'source, str>),
    Line,
    Soft,
    Hard,
    Blank,
    Choice(Box<Self>, Box<Self>),
    Group(Box<Self>),
    Flat(Box<Self>),
    Indent(Box<Self>),
    Sequence(Vec<Self>),
}

impl<'source> Document<'source> {
    pub(super) fn text(text: impl Into<Cow<'source, str>>) -> Self {
        Self::Text(text.into())
    }

    pub(super) fn sequence(parts: impl IntoIterator<Item = Self>) -> Self {
        Self::Sequence(parts.into_iter().collect())
    }

    pub(super) fn join(separator: &Self, parts: impl IntoIterator<Item = Self>) -> Self {
        let mut result = Vec::new();

        for (index, part) in parts.into_iter().enumerate() {
            if index != 0 {
                result.push(separator.clone());
            }

            result.push(part);
        }

        Self::Sequence(result)
    }

    pub(super) fn group(self) -> Self {
        Self::Group(Box::new(self))
    }

    pub(super) fn indent(self) -> Self {
        Self::Indent(Box::new(self))
    }

    pub(super) fn flattened(self) -> Self {
        match self {
            Self::Line | Self::Hard | Self::Blank => Self::text(" "),
            Self::Soft => Self::text(""),

            Self::Group(inner)
            | Self::Flat(inner)
            | Self::Indent(inner)
            | Self::Choice(inner, _) => inner.flattened(),

            Self::Sequence(parts) => Self::sequence(parts.into_iter().map(Self::flattened)),
            text @ Self::Text(_) => text,
        }
    }

    pub(super) fn width(&self) -> Option<usize> {
        match self {
            Self::Text(text) if !text.contains('\n') => Some(text.chars().count()),
            Self::Line => Some(1),
            Self::Soft => Some(0),

            Self::Group(inner)
            | Self::Flat(inner)
            | Self::Indent(inner)
            | Self::Choice(inner, _) => inner.width(),

            Self::Sequence(parts) => parts
                .iter()
                .try_fold(0usize, |width, part| width.checked_add(part.width()?)),

            _ => None,
        }
    }
}

pub(crate) fn render(document: &Document<'_>, options: &super::Options) -> String {
    let mut output = String::new();
    let mut column = 0;
    let mut pending = vec![(0, false, document)];
    let newline = options.line_endings.text();

    while let Some((depth, flat, document)) = pending.pop() {
        match document {
            Document::Text(text) => {
                output.push_str(text);

                column = text.rsplit_once('\n').map_or_else(
                    || column + text.chars().count(),
                    |(_, last)| last.chars().count(),
                );
            }

            Document::Line if flat => {
                output.push(' ');
                column += 1;
            }

            Document::Soft if flat => {}

            Document::Line | Document::Soft | Document::Hard | Document::Blank => {
                output.truncate(output.trim_end_matches([' ', '\t']).len());

                if matches!(document, Document::Blank) {
                    output.push_str(newline);
                }

                output.push_str(newline);

                match options.indent_type {
                    super::Indentation::Tabs => output.extend(std::iter::repeat_n('\t', depth)),

                    super::Indentation::Spaces => {
                        output.extend(std::iter::repeat_n(' ', depth * options.indent_width));
                    }
                }

                column = depth * options.indent_width;
            }

            Document::Choice(first, second) => {
                pending.push((depth, flat, if flat { first } else { second }));
            }

            Document::Indent(inner) => pending.push((depth + 1, flat, inner)),
            Document::Flat(inner) => pending.push((depth, true, inner)),

            Document::Group(inner) => pending.push((
                depth,
                inner
                    .width()
                    .is_some_and(|width| width <= options.column_width.saturating_sub(column)),
                inner,
            )),

            Document::Sequence(parts) => {
                pending.extend(parts.iter().rev().map(|part| (depth, flat, part)));
            }
        }
    }

    output
}
