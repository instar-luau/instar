use std::{cmp::Reverse, collections::BTreeSet, ops::Range};

use vermis::{
    token::TokenKind,
    tree::{NodeIndex, NodeKind, Tree},
};

use crate::format::Error;

#[derive(Copy, Clone, Eq, PartialEq)]
pub(crate) enum Kind {
    Verbatim,
    Remainder,
    Attached,
    Global,
}

pub(crate) struct Directive {
    pub(crate) scope: Range<usize>,
    pub(crate) kind: Kind,
}

pub(crate) struct Directives {
    pub(crate) policies: Vec<Directive>,
    barriers: BTreeSet<usize>,
    boundaries: Vec<Range<usize>>,
}

impl Directives {
    pub(crate) fn is_blocked(&self, tree: &Tree<'_>, index: NodeIndex) -> bool {
        let node = tree.node(index);

        self.barriers.contains(&index.get())
            || self.policies.iter().any(|directive| {
                directive.kind != Kind::Global
                    && directive.scope.start < node.span.end
                    && node.span.start < directive.scope.end
            })
    }

    pub(crate) fn boundary(&self, tree: &Tree<'_>, index: NodeIndex) -> Option<usize> {
        let end = tree.node(index).tokens.start.get();

        let start = tree.tokens[..end]
            .iter()
            .rposition(|token| {
                !matches!(
                    token.kind,
                    TokenKind::Whitespace | TokenKind::Comment | TokenKind::BlockComment
                )
            })
            .map_or(0, |position| position + 1);

        self.boundaries
            .iter()
            .filter(|boundary| start < boundary.end && boundary.end <= end)
            .map(|boundary| boundary.end)
            .max()
    }

    pub(crate) fn marker(&self, position: usize) -> Option<usize> {
        self.boundaries
            .iter()
            .enumerate()
            .find_map(|(index, boundary)| {
                if position == boundary.start {
                    Some(index * 2)
                } else if position + 1 == boundary.end {
                    Some(index * 2 + 1)
                } else {
                    None
                }
            })
    }

    pub(crate) fn regions(&self) -> Result<Vec<&Directive>, Error> {
        let mut regions: Vec<_> = self
            .policies
            .iter()
            .filter(|directive| matches!(directive.kind, Kind::Verbatim | Kind::Remainder))
            .collect();

        regions.sort_by_key(|directive| (directive.scope.start, Reverse(directive.scope.end)));
        let mut result = Vec::new();
        let mut end = 0;

        for directive in regions {
            if directive.scope.end <= end {
                continue;
            }

            if directive.scope.start < end {
                return Err(Error::input(
                    directive.scope.clone(),
                    "Formatting directives overlap.",
                ));
            }

            end = directive.scope.end;
            result.push(directive);
        }

        Ok(result)
    }
}

pub(crate) fn collect(tree: &Tree<'_>) -> Result<Directives, Error> {
    let mut policies = Vec::new();
    let mut barriers = BTreeSet::new();
    let mut boundaries = Vec::new();
    let mut formatting: Option<Directive> = None;
    let mut linting: Option<Range<usize>> = None;
    let mut is_leading = true;

    let mut statements: Vec<_> = tree
        .nodes
        .iter()
        .filter_map(|node| {
            if let NodeKind::Block { statements } = &node.kind {
                Some(tree.list(statements))
            } else {
                None
            }
        })
        .flatten()
        .map(|entry| entry.node)
        .collect();

    statements.sort_by_key(|index| {
        let span = tree.node(*index).span;

        (span.start, span.end - span.start)
    });

    let header = tree
        .tokens
        .iter()
        .take_while(|token| {
            token.kind == TokenKind::Whitespace
                || token.kind == TokenKind::Comment && token.bytes(tree.source).starts_with(b"--!")
        })
        .count();

    for (position, token) in tree.tokens.iter().enumerate() {
        if matches!(token.kind, TokenKind::Whitespace | TokenKind::EndOfFile) {
            continue;
        }

        if !matches!(token.kind, TokenKind::Comment | TokenKind::BlockComment) {
            is_leading = false;
            continue;
        }

        let text = String::from_utf8_lossy(token.bytes(tree.source));
        let body = text.trim_start_matches('-').trim();
        let location = token.span.start..token.span.end;

        let command = body
            .strip_prefix("instar-format")
            .or_else(|| body.strip_prefix("format:"))
            .or_else(|| body.strip_prefix("stylua:"));

        let lint = body.strip_prefix("instar-lint");

        if command.is_none()
            && lint.is_none()
            && !body.starts_with("instar-ignore")
            && !body.starts_with('!')
        {
            continue;
        }

        let following = statement(tree, &statements, location.end);

        if position >= header
            && !command.is_some_and(|command| {
                matches!(command.trim(), "off" | "ignore start" | "on" | "ignore end")
            })
            && let Some(index) = following
        {
            barriers.insert(index.get());
        }

        let directive = if let Some(command) = command {
            let (scope, kind) = match command.trim() {
                "off" | "ignore start" => {
                    if formatting.is_some() {
                        return Err(Error::input(
                            location,
                            "Formatting regions cannot be nested.",
                        ));
                    }

                    formatting = Some(Directive {
                        scope: location,
                        kind: if body.starts_with("stylua:") && command.trim() == "ignore start" {
                            Kind::Remainder
                        } else {
                            Kind::Verbatim
                        },
                    });

                    continue;
                }

                "on" | "ignore end" => {
                    let opening = formatting.take().ok_or_else(|| {
                        Error::input(
                            location.clone(),
                            "Formatting region end has no matching start.",
                        )
                    })?;

                    let start = tree
                        .tokens
                        .partition_point(|token| token.span.start < opening.scope.start);

                    boundaries.push(start..position + 1);

                    (opening.scope.start..location.end, Kind::Verbatim)
                }

                "ignore" => (
                    following_scope(
                        tree,
                        following,
                        &location,
                        "Formatting directive has no following statement.",
                    )?,
                    Kind::Verbatim,
                ),

                "ignore all" => (0..tree.source.len(), Kind::Remainder),
                _ => return Err(Error::input(location, "Unknown formatting directive.")),
            };

            Directive { scope, kind }
        } else if let Some(command) = lint {
            let words: Vec<_> = command.split_whitespace().collect();

            let (scope, kind) = match words.as_slice() {
                ["off", ..] => {
                    if linting.is_some() {
                        return Err(Error::input(
                            location,
                            "An `instar-lint off` region is already open.",
                        ));
                    }

                    linting = Some(location);
                    continue;
                }

                ["on"] => {
                    let opening = linting.take().ok_or_else(|| {
                        Error::input(
                            location.clone(),
                            "`instar-lint on` has no matching `instar-lint off`.",
                        )
                    })?;

                    (opening.start..location.end, Kind::Attached)
                }

                ["ignore", "all", ..] => (0..tree.source.len(), Kind::Global),

                ["ignore", ..] => (
                    following_scope(
                        tree,
                        following,
                        &location,
                        "This lint directive must be followed by a statement.",
                    )?,
                    Kind::Attached,
                ),

                _ => {
                    return Err(Error::input(
                        location,
                        "Lint suppression directive is not recognized.",
                    ));
                }
            };

            Directive { scope, kind }
        } else if body.starts_with("!nolint") || is_leading && body.starts_with('!') {
            Directive {
                scope: 0..tree.source.len(),
                kind: Kind::Global,
            }
        } else if body.starts_with("instar-ignore") {
            Directive {
                scope: following_scope(
                    tree,
                    following,
                    &location,
                    "This lint directive must be followed by a statement.",
                )?,
                kind: Kind::Attached,
            }
        } else {
            let scope = following.map_or(location, |index| {
                let span = tree.node(index).span;

                span.start..span.end
            });

            Directive {
                scope,
                kind: Kind::Attached,
            }
        };

        policies.push(directive);
    }

    if let Some(opening) = formatting {
        if opening.kind != Kind::Remainder {
            return Err(Error::input(
                opening.scope,
                "Formatting region start has no matching end.",
            ));
        }

        policies.push(Directive {
            scope: opening.scope.start..tree.source.len(),
            kind: Kind::Remainder,
        });
    }

    if let Some(location) = linting {
        return Err(Error::input(
            location,
            "This `instar-lint off` region has no matching `instar-lint on`.",
        ));
    }

    Ok(Directives {
        policies,
        barriers,
        boundaries,
    })
}

fn statement(tree: &Tree<'_>, statements: &[NodeIndex], offset: usize) -> Option<NodeIndex> {
    let position = statements.partition_point(|index| tree.node(*index).span.start < offset);

    statements.get(position).copied()
}

fn following_scope(
    tree: &Tree<'_>,
    following: Option<NodeIndex>,
    location: &Range<usize>,
    message: &str,
) -> Result<Range<usize>, Error> {
    let index = following.ok_or_else(|| Error::input(location.clone(), message))?;
    let span = tree.node(index).span;

    Ok(span.start..span.end)
}
