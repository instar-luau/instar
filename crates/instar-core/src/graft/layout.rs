use crate::{
    configuration::format::Options,
    format::document::{self, Document},
};
use serde::Deserialize;
use std::io;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Reply {
    version: u32,
    document: Layout,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum Layout {
    Empty,
    Source(usize, usize),
    Text(String),
    Line,
    Soft,
    Hard,
    Blank,
    Choice(Box<Self>, Box<Self>),
    Group(Box<Self>),
    Indent(Box<Self>),
    Sequence(Vec<Self>),

    Host {
        start: usize,
        end: usize,
        #[serde(default)]
        parse: Parse,
    },
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Parse {
    #[default]
    Block,

    Expression,
}

impl Layout {
    fn document<'source>(
        self,
        source: &'source str,
        options: &Options,
    ) -> io::Result<Document<'source>> {
        Ok(match self {
            Self::Empty => Document::Text("".into()),

            Self::Source(start, end) => Document::Text(
                source
                    .get(start..end)
                    .ok_or_else(|| io::Error::other("invalid graft source span"))?
                    .into(),
            ),

            Self::Text(text) => Document::Text(text.into()),
            Self::Line => Document::Line,
            Self::Soft => Document::Soft,
            Self::Hard => Document::Hard,
            Self::Blank => Document::Blank,

            Self::Choice(first, second) => Document::Choice(
                Box::new(first.document(source, options)?),
                Box::new(second.document(source, options)?),
            ),

            Self::Group(inner) => Document::Group(Box::new(inner.document(source, options)?)),
            Self::Indent(inner) => Document::Indent(Box::new(inner.document(source, options)?)),

            Self::Sequence(parts) => Document::Sequence(
                parts
                    .into_iter()
                    .map(|part| part.document(source, options))
                    .collect::<io::Result<Vec<_>>>()?,
            ),

            Self::Host { start, end, parse } => crate::format::fragment(
                source
                    .get(start..end)
                    .ok_or_else(|| io::Error::other("invalid graft host span"))?,
                options,
                matches!(parse, Parse::Expression),
            )?,
        })
    }
}

pub(super) fn format(source: &[u8], reply: &[u8], options: &Options) -> io::Result<Vec<u8>> {
    let reply: Reply = serde_json::from_slice(reply).map_err(io::Error::other)?;

    if reply.version != 1 {
        return Err(io::Error::other("unsupported graft layout version"));
    }

    let text = std::str::from_utf8(source).map_err(io::Error::other)?;
    let document = reply.document.document(text, options)?;
    let mut output = document::render(&document, options);
    output.truncate(output.trim_end_matches([' ', '\t', '\r', '\n']).len());

    if options.final_newline {
        output.push_str(options.line_endings.text());
    }

    crate::format::validate(source, output.as_bytes())?;

    Ok(output.into_bytes())
}
