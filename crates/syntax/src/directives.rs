use instar_shared::{
    Completion, Diagnostic, DiagnosticCategory, Location, Options, RelatedLocation, SourceVersion,
    Span,
};

use serde::{Deserialize, Serialize};
use vermis::{token::TokenKind, tree::NodeKind};

use crate::{Tree, parsing::diagnostic};

/// A directive and its exact lexical scope.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct Directive {
    /// Original comment span.
    pub location: Span,

    /// Governed source, statement or region.
    pub scope: Span,

    /// Directive operation.
    pub kind: DirectiveKind,
}

/// Recognized scoped source policies.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub enum DirectiveKind {
    /// Formatting is disabled in the scope.
    Format,

    /// A Luau source pragma retains its source scope.
    Source(String),

    /// Named lint rules are suppressed in the scope; empty means all rules.
    Lint(Vec<String>),
}

/// Scoped source directives and validation findings.
#[derive(Clone, Debug)]
pub struct DirectiveResult {
    /// Exact revision supplying these directive scopes.
    pub source: SourceVersion,

    /// Recognized policies.
    pub directives: Vec<Directive>,

    /// Invalid, unknown or unmatched directive findings.
    pub diagnostics: Vec<Diagnostic>,

    /// Whether all comments were examined.
    pub completion: Completion,
}

/// Collects and validates scoped formatting, lint and source directives.
#[must_use]
pub fn directives(tree: &Tree<'_>, options: &Options) -> DirectiveResult {
    let syntax = tree.syntax();

    let whole = Span {
        start: 0,
        end: syntax.source.len(),
    };

    let mut result = DirectiveResult {
        source: tree.snapshot().version,
        directives: Vec::new(),
        diagnostics: Vec::new(),
        completion: tree.coverage().clone(),
    };

    let mut region = None;
    let mut lint_region: Option<(Span, Vec<String>)> = None;

    for token in &syntax.tokens {
        if let Some(completion) = options.interrupted() {
            result.completion = completion;
            break;
        }

        if !matches!(token.kind, TokenKind::Comment | TokenKind::BlockComment) {
            continue;
        }

        let text = String::from_utf8_lossy(token.bytes(syntax.source));
        let body = text.trim_start_matches('-').trim();

        let location = Span {
            start: token.span.start,
            end: token.span.end,
        };

        let command = body
            .strip_prefix("instar-format")
            .or_else(|| body.strip_prefix("format:"))
            .or_else(|| body.strip_prefix("stylua:"));

        if let Some(command) = command {
            format(tree, command, location, whole, &mut region, &mut result);
        } else if let Some(command) = body.strip_prefix("instar-lint") {
            lint(
                tree,
                command,
                location,
                whole,
                &mut lint_region,
                &mut result,
            );
        } else if let Some(names) = body.strip_prefix("!nolint") {
            result.directives.push(Directive {
                location,
                scope: whole,
                kind: DirectiveKind::Lint(names.split_whitespace().map(str::to_owned).collect()),
            });
        } else if let Some(names) = body.strip_prefix("instar-ignore") {
            if let Some(scope) = statement(tree, location.end) {
                result.directives.push(Directive {
                    location,
                    scope,
                    kind: DirectiveKind::Lint(
                        names.split_whitespace().map(str::to_owned).collect(),
                    ),
                });
            } else {
                result.diagnostics.push(diagnostic(
                    tree.snapshot(),
                    location,
                    "lint.directive",
                    "lint directive has no following statement",
                    DiagnosticCategory::Lint,
                ));
            }
        } else if body.starts_with('!') {
            source(tree, body, location, whole, &mut result);
        }
    }

    if let Some(location) = region {
        result.diagnostics.push(diagnostic(
            tree.snapshot(),
            location,
            "format.directive",
            "unmatched formatting region start",
            DiagnosticCategory::Formatting,
        ));
    }

    if let Some((location, _)) = lint_region {
        result.diagnostics.push(diagnostic(
            tree.snapshot(),
            location,
            "lint.directive",
            "unmatched lint suppression region start",
            DiagnosticCategory::Lint,
        ));
    }

    result
}

fn format(
    tree: &Tree<'_>,
    command: &str,
    location: Span,
    whole: Span,
    region: &mut Option<Span>,
    result: &mut DirectiveResult,
) {
    match command.trim() {
        "off" | "ignore start" => {
            if let Some(opening) = *region {
                let mut finding = diagnostic(
                    tree.snapshot(),
                    location,
                    "format.directive",
                    "nested formatting region",
                    DiagnosticCategory::Formatting,
                );

                finding.related.push(RelatedLocation {
                    location: Location {
                        source: tree.snapshot().version,
                        span: opening,
                    },
                    message: "formatting region starts here".to_owned(),
                });

                result.diagnostics.push(finding);
            } else {
                *region = Some(location);
            }
        }

        "on" | "ignore end" => {
            if let Some(opening) = region.take() {
                result.directives.push(Directive {
                    location: opening,
                    scope: Span {
                        start: opening.start,
                        end: location.end,
                    },
                    kind: DirectiveKind::Format,
                });
            } else {
                result.diagnostics.push(diagnostic(
                    tree.snapshot(),
                    location,
                    "format.directive",
                    "unmatched formatting region end",
                    DiagnosticCategory::Formatting,
                ));
            }
        }

        "ignore" => {
            if let Some(scope) = statement(tree, location.end) {
                result.directives.push(Directive {
                    location,
                    scope,
                    kind: DirectiveKind::Format,
                });
            } else {
                result.diagnostics.push(diagnostic(
                    tree.snapshot(),
                    location,
                    "format.directive",
                    "formatting directive has no following statement",
                    DiagnosticCategory::Formatting,
                ));
            }
        }

        "ignore all" => result.directives.push(Directive {
            location,
            scope: whole,
            kind: DirectiveKind::Format,
        }),

        _ => result.diagnostics.push(diagnostic(
            tree.snapshot(),
            location,
            "format.directive",
            "unknown formatting directive",
            DiagnosticCategory::Formatting,
        )),
    }
}

fn lint(
    tree: &Tree<'_>,
    command: &str,
    location: Span,
    whole: Span,
    region: &mut Option<(Span, Vec<String>)>,
    result: &mut DirectiveResult,
) {
    let words = command.split_whitespace().collect::<Vec<_>>();

    match words.as_slice() {
        ["off", names @ ..] => {
            if region.is_some() {
                result.diagnostics.push(diagnostic(
                    tree.snapshot(),
                    location,
                    "lint.directive",
                    "nested lint suppression region",
                    DiagnosticCategory::Lint,
                ));
            } else {
                *region = Some((
                    location,
                    names.iter().map(|name| (*name).to_owned()).collect(),
                ));
            }
        }

        ["on"] => {
            if let Some((opening, names)) = region.take() {
                result.directives.push(Directive {
                    location: opening,
                    scope: Span {
                        start: opening.start,
                        end: location.end,
                    },
                    kind: DirectiveKind::Lint(names),
                });
            } else {
                result.diagnostics.push(diagnostic(
                    tree.snapshot(),
                    location,
                    "lint.directive",
                    "unmatched lint suppression region end",
                    DiagnosticCategory::Lint,
                ));
            }
        }

        ["ignore", "all", names @ ..] => result.directives.push(Directive {
            location,
            scope: whole,
            kind: DirectiveKind::Lint(names.iter().map(|name| (*name).to_owned()).collect()),
        }),

        ["ignore", names @ ..] => {
            if let Some(scope) = statement(tree, location.end) {
                result.directives.push(Directive {
                    location,
                    scope,
                    kind: DirectiveKind::Lint(
                        names.iter().map(|name| (*name).to_owned()).collect(),
                    ),
                });
            } else {
                result.diagnostics.push(diagnostic(
                    tree.snapshot(),
                    location,
                    "lint.directive",
                    "lint directive has no following statement",
                    DiagnosticCategory::Lint,
                ));
            }
        }

        _ => result.diagnostics.push(diagnostic(
            tree.snapshot(),
            location,
            "lint.directive",
            "unknown lint suppression directive",
            DiagnosticCategory::Lint,
        )),
    }
}

fn source(tree: &Tree<'_>, body: &str, location: Span, whole: Span, result: &mut DirectiveResult) {
    let syntax = tree.syntax();

    let leading = syntax.source[..location.start]
        .iter()
        .all(u8::is_ascii_whitespace)
        || syntax
            .tokens
            .iter()
            .filter(|previous| previous.span.end <= location.start)
            .all(|previous| {
                matches!(
                    previous.kind,
                    TokenKind::Whitespace | TokenKind::Comment | TokenKind::BlockComment
                )
            });

    let scope = if leading {
        whole
    } else {
        statement(tree, location.end).unwrap_or(location)
    };

    result.directives.push(Directive {
        location,
        scope,
        kind: DirectiveKind::Source(body.to_owned()),
    });
}

fn statement(tree: &Tree<'_>, offset: usize) -> Option<Span> {
    let syntax = tree.syntax();

    syntax
        .nodes
        .iter()
        .filter_map(|node| {
            let NodeKind::Block { statements } = &node.kind else {
                return None;
            };

            syntax
                .list(statements)
                .iter()
                .filter_map(|entry| {
                    let node = syntax.node(entry.node);

                    (node.span.start >= offset).then_some(Span {
                        start: node.span.start,
                        end: node.span.end,
                    })
                })
                .min_by_key(|span| (span.start, span.end - span.start))
        })
        .min_by_key(|span| (span.start, span.end - span.start))
}
