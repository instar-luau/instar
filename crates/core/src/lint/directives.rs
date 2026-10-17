use std::{ops::Range, str};

use vermis::{token::TokenKind, tree::NodeKind};

use crate::lint::{Error, Finding, catalog::RULES, syntax};

struct Directive<'source> {
    range: Range<usize>,
    rules: Vec<&'source str>,
}

/// Apply file, region, and following-statement lint suppressions.
///
/// # Errors
/// Rejects unknown rules, malformed directives, unmatched regions, and missing statements.
pub fn suppress(source: &str, findings: &mut Vec<Finding>) -> Result<(), Error> {
    if !source.contains("instar-lint")
        && !source.contains("instar-ignore")
        && !source.contains("!nolint")
    {
        return Ok(());
    }

    let prefix = usize::from(source.starts_with('\u{feff}')) * 3;
    let tree = syntax::parse(&source[prefix..])?;
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
    let mut leading = true;

    for token in &tree.tokens {
        if matches!(token.kind, TokenKind::Whitespace | TokenKind::EndOfFile) {
            continue;
        }

        if !matches!(token.kind, TokenKind::Comment | TokenKind::BlockComment) {
            leading = false;
            continue;
        }

        let text = str::from_utf8(token.bytes(tree.source)).map_err(|error| Error {
            range: token.span.start + prefix..token.span.end + prefix,
            message: error.to_string(),
        })?;

        let body = text.trim_start_matches('-').trim();
        let location = token.span.start + prefix..token.span.end + prefix;

        let invalid = |message: &str| Error {
            range: location.clone(),
            message: message.to_owned(),
        };

        let following = || {
            statements
                .get(statements.partition_point(|span| span.start < token.span.end))
                .map(|span| span.start + prefix..span.end + prefix)
                .ok_or_else(|| invalid("lint directive has no following statement"))
        };

        let (range, names, opening) = if let Some(command) = body.strip_prefix("instar-lint") {
            let words: Vec<_> = command.split_whitespace().collect();

            match words.as_slice() {
                ["off", rules @ ..] => {
                    if region.is_some() {
                        return Err(invalid("nested lint suppression region"));
                    }

                    (location.clone(), rules.join(" "), true)
                }

                ["on"] => {
                    let mut directive = region
                        .take()
                        .ok_or_else(|| invalid("unmatched lint suppression region end"))?;

                    directive.range.end = location.end;
                    directives.push(directive);
                    continue;
                }

                ["ignore", "all", rules @ ..] => (0..source.len(), rules.join(" "), false),
                ["ignore", rules @ ..] => (following()?, rules.join(" "), false),
                _ => return Err(invalid("unknown lint suppression directive")),
            }
        } else if let Some(names) = body.strip_prefix("instar-ignore") {
            (following()?, names.to_owned(), false)
        } else if let Some(names) = body.strip_prefix("!nolint").filter(|_| leading) {
            (0..source.len(), names.to_owned(), false)
        } else {
            continue;
        };

        let mut rules = Vec::new();

        for name in names
            .split(|character: char| character.is_whitespace() || character == ',')
            .filter(|name| !name.is_empty())
        {
            let definition = RULES
                .iter()
                .find(|rule| rule.name == name)
                .ok_or_else(|| invalid(&format!("unknown lint rule `{name}`")))?;

            rules.push(definition.name.as_str());
        }

        let directive = Directive { range, rules };

        if opening {
            region = Some(directive);
        } else {
            directives.push(directive);
        }
    }

    if let Some(region) = region {
        return Err(Error {
            range: region.range,
            message: "unmatched lint suppression region start".to_owned(),
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
