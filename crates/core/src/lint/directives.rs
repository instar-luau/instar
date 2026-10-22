use std::{ops::Range, str};

use vermis::{
    token::TokenKind,
    tree::{NodeKind, Tree},
};

use crate::lint::{Error, Finding, catalog::RULES};

struct Directive<'source> {
    range: Range<usize>,
    rules: Vec<&'source str>,
}

pub(super) fn suppress(
    source: &str,
    tree: &Tree<'_>,
    findings: &mut Vec<Finding>,
) -> Result<(), Error> {
    if !source.contains("instar-lint")
        && !source.contains("instar-ignore")
        && !source.contains("!nolint")
    {
        return Ok(());
    }

    let mut statements = Vec::new();

    for node in &tree.nodes {
        if let NodeKind::Block {
            statements: entries,
        } = &node.kind
        {
            statements.extend(
                tree.list(entries)
                    .iter()
                    .map(|entry| tree.node(entry.node).span),
            );
        }
    }

    statements.sort_by_key(|span| (span.start, span.end));
    let mut directives = Vec::new();
    let mut region: Option<Directive<'_>> = None;
    let mut is_leading = true;

    for token in &tree.tokens {
        if matches!(token.kind, TokenKind::Whitespace | TokenKind::EndOfFile) {
            continue;
        }

        if !matches!(token.kind, TokenKind::Comment | TokenKind::BlockComment) {
            is_leading = false;
            continue;
        }

        let text = str::from_utf8(token.bytes(tree.source)).map_err(|error| Error {
            range: token.span.start..token.span.end,
            message: error.to_string(),
        })?;

        let body = text.trim_start_matches('-').trim();
        let location = token.span.start..token.span.end;

        let invalid = |message: &str| Error {
            range: location.clone(),
            message: message.to_owned(),
        };

        let following = || {
            statements
                .get(statements.partition_point(|span| span.start < token.span.end))
                .map(|span| span.start..span.end)
                .ok_or_else(|| invalid("This lint directive must be followed by a statement."))
        };

        let (range, names, is_opening) = if let Some(command) = body.strip_prefix("instar-lint") {
            let words: Vec<_> = command.split_whitespace().collect();

            match words.as_slice() {
                ["off", rules @ ..] => {
                    if region.is_some() {
                        return Err(invalid("An `instar-lint off` region is already open."));
                    }

                    (location.clone(), rules.join(" "), true)
                }

                ["on"] => {
                    let mut directive = region.take().ok_or_else(|| {
                        invalid("`instar-lint on` has no matching `instar-lint off`.")
                    })?;

                    directive.range.end = location.end;
                    directives.push(directive);
                    continue;
                }

                ["ignore", "all", rules @ ..] => (0..source.len(), rules.join(" "), false),
                ["ignore", rules @ ..] => (following()?, rules.join(" "), false),
                _ => return Err(invalid("Lint suppression directive is not recognized.")),
            }
        } else if let Some(names) = body.strip_prefix("instar-ignore") {
            (following()?, names.to_owned(), false)
        } else if let Some(names) = body.strip_prefix("!nolint").filter(|_| is_leading) {
            (0..source.len(), names.to_owned(), false)
        } else {
            continue;
        };

        let mut rules = Vec::new();

        for name in names
            .split(|character: char| character.is_whitespace() || character == ',')
            .filter(|name| !name.is_empty())
        {
            let (name, _) = RULES
                .iter()
                .find(|(rule, _)| *rule == name)
                .ok_or_else(|| invalid(&format!("Lint rule `{name}` is not recognized.")))?;

            rules.push(*name);
        }

        let directive = Directive { range, rules };

        if is_opening {
            region = Some(directive);
        } else {
            directives.push(directive);
        }
    }

    if let Some(region) = region {
        return Err(Error {
            range: region.range,
            message: "This `instar-lint off` region has no matching `instar-lint on`.".to_owned(),
        });
    }

    findings.retain(|finding| {
        !directives.iter().any(|directive| {
            directive.range.start <= finding.range.start
                && finding.range.start < directive.range.end
                && (directive.rules.is_empty() || directive.rules.contains(&finding.rule.as_str()))
        })
    });

    Ok(())
}
