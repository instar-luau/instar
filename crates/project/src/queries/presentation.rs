use std::io;

use instar_luau::FactsResult;
use instar_shared::{Location, Options, SourceSnapshot, Span, TextEdit};

use instar_syntax::{
    SyntaxSettings, bindings,
    literal::{self, NumberValue},
    parse,
    tree::NodeKind,
};

use super::{
    output::{ColorRepresentation, PresentationOutput, SourceColor},
    request::{PresentationKind, PresentationQuery},
};

use crate::{structure, tokens};

pub(crate) fn presentation(
    request: &PresentationQuery,
    source: &SourceSnapshot,
    syntax: &SyntaxSettings,
    facts: Option<&FactsResult>,
    roblox: bool,
    options: &Options,
) -> io::Result<PresentationOutput> {
    match &request.kind {
        PresentationKind::Tokens { range } => {
            let span = range.unwrap_or(Span {
                start: 0,
                end: source.text.len(),
            });

            let location = Location::new(source, span)?;

            let entries = tokens::tokens(source, syntax, facts, options)?
                .into_iter()
                .filter(|token| token.range[0] >= span.start && token.range[1] <= span.end)
                .collect();

            Ok(PresentationOutput::Tokens { location, entries })
        }

        PresentationKind::Folds => {
            structure::folds(source, syntax, options).map(PresentationOutput::Folds)
        }

        PresentationKind::Selections(offsets) => {
            structure::selections(source, syntax, offsets, options).map(|selections| {
                PresentationOutput::Selections(
                    selections
                        .into_iter()
                        .map(|ranges| {
                            ranges
                                .into_iter()
                                .map(|range| Span {
                                    start: range[0],
                                    end: range[1],
                                })
                                .collect()
                        })
                        .collect(),
                )
            })
        }

        PresentationKind::Colors => {
            colors(source, syntax, roblox, options).map(PresentationOutput::Colors)
        }

        PresentationKind::Color { range, rgba } => {
            if rgba
                .iter()
                .any(|component| !component.is_finite() || !(0.0..=1.0).contains(component))
            {
                return Err(invalid("invalid color components"));
            }

            let location = Location::new(source, *range)?;

            if !colors(source, syntax, roblox, options)?
                .iter()
                .any(|color| color.location == location)
            {
                return Err(invalid(
                    "color representation range is not a supported color expression",
                ));
            }

            let labels = [
                format!("Color3.new({}, {}, {})", rgba[0], rgba[1], rgba[2]),
                format!(
                    "Color3.fromRGB({}, {}, {})",
                    (rgba[0] * 255.0).round(),
                    (rgba[1] * 255.0).round(),
                    (rgba[2] * 255.0).round()
                ),
            ];

            Ok(PresentationOutput::ColorRepresentations(
                labels
                    .into_iter()
                    .map(|label| ColorRepresentation {
                        edit: TextEdit {
                            location,
                            replacement: label.clone(),
                        },
                        label,
                    })
                    .collect(),
            ))
        }
    }
}

fn colors(
    source: &SourceSnapshot,
    syntax: &SyntaxSettings,
    roblox: bool,
    options: &Options,
) -> io::Result<Vec<SourceColor>> {
    let parsed = parse(source, syntax, options);
    let bindings = bindings(&parsed.tree, options);
    let tree = parsed.tree.syntax();
    let mut colors = Vec::new();

    if !roblox {
        return Ok(colors);
    }

    for node in &tree.nodes {
        options.check()?;

        let NodeKind::Call { callee, arguments } = node.kind else {
            continue;
        };

        let NodeKind::Field { receiver, name, .. } = tree.node(callee).kind else {
            continue;
        };

        if tree.text(receiver) != b"Color3" || !bindings.global(tree, receiver) {
            continue;
        }

        let NodeKind::Arguments { ref values, .. } = tree.node(arguments).kind else {
            continue;
        };

        let arguments = tree.list(values);

        if arguments.len() != 3 {
            continue;
        }

        let mut values = Vec::new();

        for argument in arguments {
            if let Some(NumberValue::Float(value)) = literal::number(tree, argument.node) {
                values.push(f64::from_bits(value));
            }
        }

        if values.len() != 3 {
            continue;
        }

        match tree.text(name) {
            b"new" => {}

            b"fromRGB" => {
                for value in &mut values {
                    *value /= 255.0;
                }
            }

            _ => continue,
        }

        if values
            .iter()
            .any(|value| !value.is_finite() || !(0.0..=1.0).contains(value))
        {
            continue;
        }

        colors.push(SourceColor {
            location: Location::new(
                source,
                Span {
                    start: node.span.start,
                    end: node.span.end,
                },
            )?,
            rgba: [values[0], values[1], values[2], 1.0],
        });
    }

    Ok(colors)
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}
