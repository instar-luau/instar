pub(crate) mod document;
mod literals;
mod regions;
mod rules;
mod scope;

use std::{borrow::Cow, io};

use crate::project::configuration::format::{Options, Quotes, Zero};
use vermis::{Kind, TokenKind, Tree};

/// # Errors
/// Returns invalid source, invalid options, or an output validation failure.
pub fn format(source: &[u8], options: &Options) -> io::Result<Vec<u8>> {
    if !options.enabled {
        return Ok(source.to_vec());
    }

    if options.indentation.width == 0 || options.column_width == 0 {
        return Err(io::Error::other("format widths must be positive"));
    }

    let original = std::str::from_utf8(source).map_err(io::Error::other)?;
    let tokens = vermis::tokenize(source.into());
    let mut normalized = Cow::Borrowed(source);
    let mut previous = None;

    for token in &tokens {
        if matches!(
            token.kind,
            TokenKind::Whitespace | TokenKind::Comment | TokenKind::BlockComment
        ) {
            continue;
        }

        if token.kind == TokenKind::Byte(b';')
            && matches!(previous, None | Some(TokenKind::Byte(b';')))
        {
            normalized.to_mut()[token.span.start] = b' ';

            let start = original[..token.span.start]
                .rfind('\n')
                .map_or(0, |index| index + 1);

            if let Some(end) = original[token.span.end..]
                .find('\n')
                .map(|index| index + token.span.end)
                && original[start..token.span.start].trim().is_empty()
                && original[token.span.end..end].trim().is_empty()
            {
                normalized.to_mut()[end] = b' ';
            }
        }

        previous = Some(token.kind);
    }

    let text = std::str::from_utf8(&normalized).map_err(io::Error::other)?;
    let tree = vermis::parse(normalized.as_ref().into());

    if let Some(diagnostic) = tree.diagnostics.first() {
        return Err(io::Error::other(format!(
            "syntax error at byte {}: {}",
            diagnostic.span.start, diagnostic.message
        )));
    }

    for index in 0..tree.nodes.len() {
        if let Some(vermis::Parts::Call { callee, arguments }) =
            tree.view(index).and_then(vermis::View::parts)
            && arguments.text().to_string().starts_with('(')
            && text[callee.span().end..arguments.span().start].contains('\n')
        {
            return Err(io::Error::other(
                "ambiguous syntax: call parentheses start on another line",
            ));
        }
    }

    let held = regions::held(text, &tree);

    if held
        .iter()
        .any(|range| range.start == 0 && range.end == source.len())
    {
        return Ok(original.as_bytes().to_vec());
    }

    let prepared = rules::prepare(text, &tree, options, &held)?;
    let text = prepared.as_str();
    let source = text.as_bytes();
    let tree = vermis::parse(source.into());
    let held = regions::held(text, &tree);
    let document = rules::emit(text, &tree, options, &held)?;
    let mut output = document::render(&document, options);
    output.truncate(output.trim_end_matches([' ', '\t', '\r', '\n']).len());

    if options.final_newline {
        output.push_str(options.line_endings.text());
    }

    validate(source, output.as_bytes())?;

    Ok(output.into_bytes())
}

pub(crate) fn validate(source: &[u8], output: &[u8]) -> io::Result<()> {
    let text = std::str::from_utf8(source).map_err(io::Error::other)?;
    let output = std::str::from_utf8(output).map_err(io::Error::other)?;
    let tree = vermis::parse(source.into());
    let parsed = vermis::parse(output.as_bytes().into());

    if !parsed.diagnostics.is_empty() || structure(&tree, text) != structure(&parsed, output) {
        return Err(io::Error::other(
            "formatted output changed syntax; source was not written",
        ));
    }

    let held = regions::held(text, &tree)
        .into_iter()
        .map(|range| &text[range])
        .collect::<Vec<_>>();

    let output_held = regions::held(output, &parsed)
        .into_iter()
        .map(|range| &output[range])
        .collect::<Vec<_>>();

    if held != output_held {
        return Err(io::Error::other(
            "formatted output changed a suppressed region; source was not written",
        ));
    }

    if comments(&tree, source) != comments(&parsed, output.as_bytes()) {
        return Err(io::Error::other(
            "formatted output changed comments; source was not written",
        ));
    }

    Ok(())
}

pub(crate) fn fragment(
    source: &str,
    options: &Options,
    expression: bool,
) -> io::Result<document::Document<'static>> {
    let source = if expression {
        format!("return {source}")
    } else {
        source.to_owned()
    };

    let tree = vermis::parse(source.as_bytes().into());

    if !tree.diagnostics.is_empty() {
        return Err(io::Error::other("invalid graft host span"));
    }

    let held = regions::held(&source, &tree);
    let source = rules::prepare(&source, &tree, options, &held)?;
    let tree = vermis::parse(source.as_bytes().into());
    let held = regions::held(&source, &tree);

    if expression {
        let block = tree
            .root_view()
            .and_then(|root| root.children().next())
            .ok_or_else(|| io::Error::other("missing graft expression"))?;

        if block.children().count() != 1 {
            return Err(io::Error::other(
                "graft expression span contains statements",
            ));
        }

        let Some(vermis::Parts::Return { values }) =
            block.children().next().and_then(vermis::View::parts)
        else {
            return Err(io::Error::other("missing graft expression"));
        };

        if values.clone().count() != 1 {
            return Err(io::Error::other(
                "graft expression span contains multiple values",
            ));
        }

        Ok(rules::node(
            &source,
            &tree,
            options,
            &held,
            values.clone().next().expect("one expression"),
        )?
        .owned())
    } else {
        Ok(rules::emit(&source, &tree, options, &held)?.owned())
    }
}

fn structure<'source>(tree: &Tree<'_>, source: &'source str) -> Vec<(Kind, Cow<'source, str>)> {
    let mut result = Vec::new();
    let mut pending = vec![tree.root];

    while let Some(index) = pending.pop() {
        let node = &tree.nodes[index];

        if matches!(node.kind, Kind::TypeUnion | Kind::TypeIntersection) && node.children.len() == 1
        {
            pending.extend(tree.children[node.children.clone()].iter().copied());
            continue;
        }

        let text = if matches!(
            node.kind,
            Kind::Name
                | Kind::Number
                | Kind::String
                | Kind::Boolean
                | Kind::Nil
                | Kind::Operator
                | Kind::Break
                | Kind::Continue
        ) {
            &source[node.span.start..node.span.end]
        } else {
            ""
        };

        let text = match node.kind {
            Kind::String => literals::quote(text, Quotes::Double),
            Kind::Number => literals::number(text, Zero::Add),
            _ => Cow::Borrowed(text),
        };

        result.push((node.kind, text));
        pending.extend(tree.children[node.children.clone()].iter().rev().copied());
    }

    result
}

fn comments<'source>(tree: &Tree<'_>, source: &'source [u8]) -> Vec<&'source [u8]> {
    tree.tokens
        .iter()
        .filter(|token| matches!(token.kind, TokenKind::Comment | TokenKind::BlockComment))
        .map(|token| &source[token.span.start..token.span.end])
        .collect()
}
