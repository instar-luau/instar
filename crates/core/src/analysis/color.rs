use std::{ops::Range, str};

use vermis::{
    token::TokenKind,
    tree::{NodeKind, Tree},
};

use crate::{
    analysis::{Edit, analyze},
    syntax::{bindings::Bindings, expression::number, literal},
};

/// A statically recognized Roblox Color3 constructor.
#[derive(Clone, Debug)]
pub struct Color {
    /// Complete constructor expression.
    pub range: Range<usize>,

    /// Red, green, blue, and alpha in the inclusive zero-to-one interval.
    pub components: [f64; 4],
}

/// Returns validated Color3 constructor replacements for a recognized, comment-free expression.
#[must_use]
pub fn color_presentations(source: &str, range: Range<usize>, components: [f64; 4]) -> Vec<Edit> {
    if components
        .iter()
        .any(|component| !component.is_finite() || !(0.0..=1.0).contains(component))
        || !components[3].total_cmp(&1.0).is_eq()
        || !source.is_char_boundary(range.start)
        || !source.is_char_boundary(range.end)
        || !analyze(source)
            .colors
            .iter()
            .any(|color| color.range == range)
    {
        return Vec::new();
    }

    let Some(expression) = source.get(range.clone()) else {
        return Vec::new();
    };

    let tree = vermis::parse(expression.as_bytes());

    if !tree.diagnostics.is_empty()
        || tree.tokens.iter().any(|token| {
            matches!(
                token.kind,
                TokenKind::Comment | TokenKind::BlockComment | TokenKind::MalformedComment
            )
        })
    {
        return Vec::new();
    }

    let [red, green, blue, _] = components;

    [
        format!("Color3.new({red}, {green}, {blue})"),
        format!(
            "Color3.fromRGB({}, {}, {})",
            red * 255.0,
            green * 255.0,
            blue * 255.0
        ),
    ]
    .into_iter()
    .map(move |replacement| Edit {
        range: range.clone(),
        replacement,
    })
    .collect()
}

pub(super) fn colors(tree: &Tree<'_>, bindings: &Bindings, prefix: usize) -> Vec<Color> {
    tree.nodes
        .iter()
        .filter_map(|node| {
            let NodeKind::Call { callee, arguments } = node.kind else {
                return None;
            };

            let constructor = ["new", "fromRGB", "fromHSV", "fromHex"]
                .into_iter()
                .find(|name| bindings.is_global(tree, callee, &format!("Color3.{name}")))?;

            let NodeKind::Arguments {
                values,
                closing,
                opening,
            } = &tree.node(arguments).kind
            else {
                return None;
            };

            if opening.is_some() && closing.is_none() {
                return None;
            }

            let values = tree.list(values);

            let components = if constructor == "fromHex" {
                let [value] = values else { return None };

                if !matches!(tree.node(value.node).kind, NodeKind::String { .. }) {
                    return None;
                }

                let bytes = literal::decode(tree.text(value.node))?;
                let text = str::from_utf8(&bytes).ok()?;

                hexadecimal(text)?
            } else if constructor == "new" && values.is_empty() {
                [0.0, 0.0, 0.0, 1.0]
            } else {
                let [first, second, third] = values else {
                    return None;
                };

                let first = number(tree, first.node)?;
                let second = number(tree, second.node)?;
                let third = number(tree, third.node)?;

                let limit = if constructor == "fromRGB" { 255.0 } else { 1.0 };

                if [first, second, third]
                    .iter()
                    .any(|value| !value.is_finite() || !(0.0..=limit).contains(value))
                {
                    return None;
                }

                match constructor {
                    "fromRGB" => [first / 255.0, second / 255.0, third / 255.0, 1.0],
                    "fromHSV" => hue_saturation_value(first, second, third),
                    "new" => [first, second, third, 1.0],
                    _ => return None,
                }
            };

            Some(Color {
                range: node.span.start + prefix..node.span.end + prefix,
                components,
            })
        })
        .collect()
}

fn hexadecimal(text: &str) -> Option<[f64; 4]> {
    let text = text.strip_prefix('#').unwrap_or(text);
    let expanded;

    let text = if text.len() == 3 && text.is_ascii() {
        expanded = text
            .chars()
            .flat_map(|character| [character, character])
            .collect::<String>();

        expanded.as_str()
    } else {
        text
    };

    if text.len() != 6 || !text.is_ascii() {
        return None;
    }

    Some([
        f64::from(u8::from_str_radix(&text[0..2], 16).ok()?) / 255.0,
        f64::from(u8::from_str_radix(&text[2..4], 16).ok()?) / 255.0,
        f64::from(u8::from_str_radix(&text[4..6], 16).ok()?) / 255.0,
        1.0,
    ])
}

fn hue_saturation_value(hue: f64, saturation: f64, value: f64) -> [f64; 4] {
    let hue = hue.rem_euclid(1.0) * 6.0;
    let chroma = value * saturation;
    let secondary = chroma * (1.0 - (hue.rem_euclid(2.0) - 1.0).abs());
    let minimum = value - chroma;

    let [red, green, blue] = if hue < 1.0 {
        [chroma, secondary, 0.0]
    } else if hue < 2.0 {
        [secondary, chroma, 0.0]
    } else if hue < 3.0 {
        [0.0, chroma, secondary]
    } else if hue < 4.0 {
        [0.0, secondary, chroma]
    } else if hue < 5.0 {
        [secondary, 0.0, chroma]
    } else {
        [chroma, 0.0, secondary]
    };

    [red + minimum, green + minimum, blue + minimum, 1.0]
}
