use super::{
    configuration::{Constant, Settings},
    mapping::{Edit, Text},
    syntax::{array, field, kind, nodes, quote, range},
};

use crate::source::Source;
use serde_json::Value;
use std::{collections::BTreeSet, io};
use vermis::{Kind, Parts, TokenKind};

pub(super) fn constants(
    text: &Text,
    source: &Source,
    document: &Value,
    settings: &Settings,
) -> io::Result<Text> {
    let nodes = nodes(&document["root"]);
    let mut writes = BTreeSet::new();

    for value in &nodes {
        let targets = match kind(value) {
            "AstStatAssign" => array(&value["vars"]).iter().collect::<Vec<_>>(),
            "AstStatCompoundAssign" => vec![&value["var"]],
            "AstStatFunction" => vec![&value["name"]],
            _ => Vec::new(),
        };

        for target in targets {
            if kind(target) == "AstExprGlobal" {
                writes.insert(field(target, "global"));
            }
        }
    }

    let mut edits = Vec::new();

    for value in nodes {
        if kind(value) != "AstExprGlobal" {
            continue;
        }

        let name = field(value, "global");

        let Some(constant) = settings.constants.get(name) else {
            continue;
        };

        if writes.contains(name) {
            return Err(io::Error::other(format!(
                "build constant is assigned: {name}"
            )));
        }

        let literal = match constant {
            Constant::Boolean(value) => value.to_string(),
            Constant::Number(value) if value.is_finite() => value.to_string(),
            Constant::Number(_) => return Err(io::Error::other("build constants must be finite")),
            Constant::String(value) => quote(value),
        };

        edits.push(Edit {
            range: range(source, value)?,
            text: format!("({literal})"),
        });
    }

    text.edit(edits)
}

pub(super) fn lower(mut text: Text) -> io::Result<Text> {
    loop {
        let tree = vermis::parse(text.text.as_bytes().into());
        let mut edits = Vec::new();

        for index in 0..tree.nodes.len() {
            let Some(view) = tree.view(index) else {
                continue;
            };

            let span = view.span().start..view.span().end;

            if matches!(view.kind(), Kind::Local | Kind::LocalFunction) {
                let start = match view.parts() {
                    Some(Parts::Function {
                        attributes: Some(attributes),
                        ..
                    }) => attributes.span().end,

                    _ => span.start,
                };

                if let Some(token) = tree
                    .tokens
                    .iter()
                    .skip(tree.tokens.partition_point(|token| token.span.end <= start))
                    .find(|token| {
                        !matches!(
                            token.kind,
                            TokenKind::Whitespace | TokenKind::Comment | TokenKind::BlockComment
                        )
                    })
                    && &text.text[token.span.start..token.span.end] == "const"
                {
                    edits.push(Edit {
                        range: token.span.start..token.span.end,
                        text: "local".into(),
                    });
                }
            }

            if matches!(
                view.kind(),
                Kind::TypeAlias | Kind::TypeFunction | Kind::Declaration
            ) {
                edits.push(Edit {
                    range: span,
                    text: String::new(),
                });

                continue;
            }

            match view.parts() {
                Some(Parts::Binding {
                    name,
                    annotation: Some(annotation),
                }) => edits.push(Edit {
                    range: name.span().end..annotation.span().end,
                    text: String::new(),
                }),

                Some(Parts::Function {
                    parameters,
                    returns: Some(returns),
                    ..
                }) => edits.push(Edit {
                    range: parameters.span().end..returns.span().end,
                    text: String::new(),
                }),

                Some(Parts::Generics { .. }) => edits.push(Edit {
                    range: span,
                    text: String::new(),
                }),

                Some(Parts::Variadic {
                    annotation: Some(annotation),
                }) => edits.push(Edit {
                    range: view.span().start + 3..annotation.span().end,
                    text: String::new(),
                }),

                Some(
                    Parts::Assertion { expression, .. } | Parts::Instantiate { expression, .. },
                ) => edits.push(Edit {
                    range: span,
                    text: format!("({})", expression.text()),
                }),

                Some(Parts::Export { declaration, .. }) => edits.push(Edit {
                    range: view.span().start..declaration.span().start,
                    text: String::new(),
                }),

                _ => {}
            }
        }

        if edits.is_empty() {
            return Ok(text);
        }

        edits.sort_by_key(|edit| (edit.range.start, std::cmp::Reverse(edit.range.end)));
        let mut end = 0;

        edits.retain(|edit| {
            if edit.range.start < end {
                false
            } else {
                end = edit.range.end;

                true
            }
        });

        text = text.edit(edits)?;
        validate(&text.text)?;
    }
}

pub(super) fn exports(text: &Text) -> io::Result<Text> {
    let tree = vermis::parse(text.text.as_bytes().into());
    let mut edits = Vec::new();

    for index in 0..tree.nodes.len() {
        if let Some(view) = tree.view(index)
            && let Some(Parts::Export { declaration, .. }) = view.parts()
        {
            edits.push(Edit {
                range: view.span().start..declaration.span().start,
                text: String::new(),
            });
        }
    }

    text.edit(edits)
}

pub(super) fn minify(text: &Text) -> io::Result<Text> {
    let tokens = vermis::tokenize(text.text.as_bytes().into());

    let tokens = tokens
        .iter()
        .filter(|token| {
            !matches!(
                token.kind,
                TokenKind::Eof
                    | TokenKind::Whitespace
                    | TokenKind::Comment
                    | TokenKind::BlockComment
            )
        })
        .collect::<Vec<_>>();

    let mut previous = 0;
    let mut preceding = "";
    let mut edits = Vec::new();

    for token in tokens {
        let current = &text.text[token.span.start..token.span.end];

        if previous < token.span.start {
            let joined = format!("{preceding}{current}");
            let parsed = vermis::tokenize(joined.as_bytes().into());

            let separate = !preceding.is_empty()
                && (parsed
                    .iter()
                    .filter(|token| token.kind != TokenKind::Eof)
                    .count()
                    != 2
                    || parsed[0].span.end != preceding.len()
                    || matches!(parsed[0].kind, TokenKind::Comment | TokenKind::BlockComment));

            edits.push(Edit {
                range: previous..token.span.start,
                text: if separate { " ".into() } else { String::new() },
            });
        }

        previous = token.span.end;
        preceding = current;
    }

    if previous < text.text.len() {
        edits.push(Edit {
            range: previous..text.text.len(),
            text: String::new(),
        });
    }

    let output = text.edit(edits)?;
    validate(&output.text)?;

    Ok(output)
}

pub(super) fn validate(text: &str) -> io::Result<()> {
    let tree = vermis::parse(text.as_bytes().into());

    if let Some(error) = tree.diagnostics.first() {
        return Err(io::Error::other(format!(
            "syntax at byte {}: {}",
            error.span.start, error.message
        )));
    }

    Ok(())
}
