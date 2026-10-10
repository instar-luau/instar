//! Resolved module links and documentation URL locations.

use std::{
    collections::BTreeMap,
    io,
    path::{Path, PathBuf},
    slice,
    time::Instant,
};

use instar_analysis::{Location, Options, error::invalid};
use vermis::{lexer::Lexer, token::TokenKind};

use crate::project::{Project, absolute};

/// Destination of a document link.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    /// A resolved module's backing source.
    Source(PathBuf),

    /// An HTTP or HTTPS documentation URL in a source comment.
    Documentation(String),
}

/// A source-anchored navigation link.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Link {
    /// Exact source revision and byte range containing the link.
    pub location: Location<PathBuf>,

    /// Destination to present to a source consumer.
    pub target: Target,
}

impl Project {
    /// Returns module targets agreed upon by every context, plus comment URLs.
    ///
    /// # Errors
    /// Returns source, configuration, map, source agreement, or interruption failures.
    pub fn document_links(&mut self, path: &Path, options: &Options) -> io::Result<Vec<Link>> {
        let started = Instant::now();

        self.during(
            |project| {
                let path = absolute(path)?;
                let source = project.source(&path)?;

                let entries =
                    project.entries(slice::from_ref(&path), &options.remaining(started))?;

                let mut common: Option<BTreeMap<_, _>> = None;

                for entry in entries {
                    options.check(started)?;
                    let node = project.links(&entry.source, entry.context.as_ref())?;

                    if node.document.revision != source.revision {
                        return Err(invalid("document link source revision mismatch"));
                    }

                    let mut current = BTreeMap::new();

                    for site in &node.sites {
                        options.check(started)?;

                        if let Some(target) = &site.target {
                            current.insert(site.range, target.source.clone());
                        }
                    }

                    match &mut common {
                        Some(common) => {
                            common.retain(|range, target| current.get(range) == Some(target));
                        }

                        None => common = Some(current),
                    }
                }

                let mut links = common
                    .unwrap_or_default()
                    .into_iter()
                    .map(|(range, target)| Link {
                        location: Location {
                            module: path.clone(),
                            revision: source.revision,
                            range,
                        },
                        target: Target::Source(target),
                    })
                    .collect::<Vec<_>>();

                for token in Lexer::new(source.text.as_bytes()) {
                    options.check(started)?;

                    if !matches!(
                        token.kind,
                        TokenKind::Comment | TokenKind::BlockComment | TokenKind::MalformedComment
                    ) {
                        continue;
                    }

                    let Some(text) = source.text.get(token.span.start..token.span.end) else {
                        continue;
                    };

                    for range in documentation(text, options, started)? {
                        links.push(Link {
                            location: Location {
                                module: path.clone(),
                                revision: source.revision,
                                range: [token.span.start + range[0], token.span.start + range[1]],
                            },
                            target: Target::Documentation(text[range[0]..range[1]].to_owned()),
                        });
                    }
                }

                links.sort_by_key(|link| link.location.range);
                options.check(started)?;

                Ok(links)
            },
            options,
        )
    }
}

fn documentation(text: &str, options: &Options, started: Instant) -> io::Result<Vec<[usize; 2]>> {
    let mut result = Vec::new();
    let mut covered = 0;

    for (colon, _) in text.match_indices("://") {
        options.check(started)?;

        let start = ["https", "http"].into_iter().find_map(|scheme| {
            let start = colon.checked_sub(scheme.len())?;

            text.get(start..colon)
                .is_some_and(|value| value.eq_ignore_ascii_case(scheme))
                .then_some(start)
        });

        let Some(start) = start else {
            continue;
        };

        if start < covered
            || text[..start]
                .chars()
                .next_back()
                .is_some_and(|character| character.is_alphanumeric() || character == '_')
        {
            continue;
        }

        let end = start + address_length(&text[start..], options, started)?;
        covered = end;
        result.push([start, end]);
    }

    Ok(result)
}

fn address_length(text: &str, options: &Options, started: Instant) -> io::Result<usize> {
    let mut parentheses = 0_usize;
    let mut brackets = 0_usize;

    for (offset, character) in text.char_indices() {
        options.check(started)?;

        if character.is_whitespace()
            || character.is_control()
            || matches!(character, '<' | '>' | '"' | '\'' | '`')
        {
            return Ok(offset);
        }

        match character {
            '(' => parentheses += 1,
            ')' if parentheses == 0 => return Ok(offset),
            ')' => parentheses -= 1,
            '[' => brackets += 1,
            ']' if brackets == 0 => return Ok(offset),
            ']' => brackets -= 1,
            _ => {}
        }
    }

    Ok(text.len())
}
