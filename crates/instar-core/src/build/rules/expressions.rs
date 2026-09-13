use super::{Context, atomic, identifier, plain_string, replace_keep_lines, span, text};
use crate::build::{configuration::Rules, mapping::Edit, syntax::quote};
use std::cmp::Ordering;
use vermis::{Kind, Parts, View};

pub(super) fn apply(
    context: &Context<'_, '_, '_, '_, '_>,
    settings: &Rules,
    edits: &mut Vec<Edit>,
) {
    for node_index in 0..context.tree.nodes.len() {
        let Some(view) = context.tree.view(node_index) else {
            continue;
        };

        if settings.convert_index_to_field {
            index(view, edits);
        }

        if settings.convert_luau_number && view.kind() == Kind::Number {
            number(view, edits);
        }

        if settings.remove_if_expression && view.kind() == Kind::Conditional {
            conditional(view, context.text, edits);
        }

        if settings.compute_expression
            && matches!(view.kind(), Kind::Unary | Kind::Binary | Kind::Group)
        {
            compute(view, context.text, edits);
        }
    }
}

fn index(view: View<'_, '_>, edits: &mut Vec<Edit>) {
    match view.parts() {
        Some(Parts::Index { receiver, key }) if key.kind() == Kind::String => {
            let Some(name) = plain_string(key).filter(|name| identifier(name)) else {
                return;
            };

            edits.push(Edit {
                range: receiver.span().end..view.span().end,
                text: format!(".{name}"),
            });
        }

        Some(Parts::TableField {
            key: Some(key),
            indexed: true,
            ..
        }) if key.kind() == Kind::String => {
            let Some(name) = plain_string(key).filter(|name| identifier(name)) else {
                return;
            };

            if let Some(equal) = text(view).find('=') {
                edits.push(Edit {
                    range: view.span().start..view.span().start + equal,
                    text: format!("{name} "),
                });
            }
        }

        _ => {}
    }
}

fn number(view: View<'_, '_>, edits: &mut Vec<Edit>) {
    let value = text(view);
    let cleaned = value.replace('_', "");
    let lower = cleaned.to_ascii_lowercase();

    let replacement = if let Some(bits) = lower.strip_prefix("0b") {
        u64::from_str_radix(bits, 2)
            .ok()
            .map(|number| format!("0x{number:X}"))
    } else {
        (cleaned != value).then_some(cleaned)
    };

    if let Some(replacement) = replacement {
        edits.push(Edit {
            range: span(view),
            text: replacement,
        });
    }
}

fn operand(view: View<'_, '_>) -> String {
    if atomic(view) {
        text(view).to_owned()
    } else {
        format!("({})", text(view))
    }
}

fn conditional(view: View<'_, '_>, source: &str, edits: &mut Vec<Edit>) {
    if let Some(replacement) = conditional_text(view) {
        replace_keep_lines(source, span(view), &replacement, edits);
    }
}

fn conditional_text(view: View<'_, '_>) -> Option<String> {
    let Parts::Conditional {
        condition,
        truthy,
        falsy,
    } = view.parts()?
    else {
        return None;
    };

    if truth(truthy) != Some(true) {
        return None;
    }

    let falsy = if falsy.kind() == Kind::Conditional {
        format!("({})", conditional_text(falsy)?)
    } else {
        operand(falsy)
    };

    Some(format!(
        "{} and {} or {falsy}",
        operand(condition),
        operand(truthy)
    ))
}

#[derive(Clone)]
pub(super) enum Constant {
    Nil,
    Boolean(bool),
    Number(f64),
    String(String),
}

impl Constant {
    pub(super) fn truth(&self) -> bool {
        !matches!(self, Self::Nil | Self::Boolean(false))
    }

    fn render(&self) -> String {
        match self {
            Self::Nil => "nil".into(),
            Self::Boolean(value) => value.to_string(),
            Self::Number(value) => value.to_string(),
            Self::String(value) => quote(value),
        }
    }
}

pub(super) fn evaluate(view: View<'_, '_>) -> Option<Constant> {
    match view.kind() {
        Kind::Nil => Some(Constant::Nil),
        Kind::Boolean => Some(Constant::Boolean(text(view) == "true")),
        Kind::Number => Some(Constant::Number(parse_number(text(view))?)),
        Kind::String => plain_string(view).map(|value| Constant::String(value.to_owned())),

        Kind::Group => match view.parts()? {
            Parts::Group { expression } => evaluate(expression),
            _ => None,
        },

        Kind::Unary => {
            let Parts::Unary { operator, operand } = view.parts()? else {
                return None;
            };

            let value = evaluate(operand)?;

            match text(operator) {
                "not" => Some(Constant::Boolean(!value.truth())),

                "-" => match value {
                    Constant::Number(value) => Some(Constant::Number(-value)),
                    _ => None,
                },

                "#" => match value {
                    Constant::String(value) => {
                        Some(Constant::Number(value.len().to_string().parse().ok()?))
                    }

                    _ => None,
                },

                _ => None,
            }
        }

        Kind::Binary => {
            let Parts::Binary {
                left,
                operator,
                right,
            } = view.parts()?
            else {
                return None;
            };

            binary(evaluate(left)?, text(operator), evaluate(right)?)
        }

        _ => None,
    }
}

pub(super) fn truth(view: View<'_, '_>) -> Option<bool> {
    evaluate(view)
        .map(|value| value.truth())
        .or_else(|| matches!(view.kind(), Kind::Table | Kind::Function).then_some(true))
}

fn parse_number(value: &str) -> Option<f64> {
    let value = value.replace('_', "");
    let lower = value.to_ascii_lowercase();

    if let Some(digits) = lower.strip_prefix("0b") {
        u64::from_str_radix(digits, 2)
            .ok()?
            .to_string()
            .parse()
            .ok()
    } else if let Some(digits) = lower.strip_prefix("0x") {
        u64::from_str_radix(digits, 16)
            .ok()?
            .to_string()
            .parse()
            .ok()
    } else {
        value.parse().ok()
    }
}

fn binary(left: Constant, operator: &str, right: Constant) -> Option<Constant> {
    match (left, operator, right) {
        (Constant::Number(left), "+", Constant::Number(right)) => {
            Some(Constant::Number(left + right))
        }

        (Constant::Number(left), "-", Constant::Number(right)) => {
            Some(Constant::Number(left - right))
        }

        (Constant::Number(left), "*", Constant::Number(right)) => {
            Some(Constant::Number(left * right))
        }

        (Constant::Number(left), "/", Constant::Number(right)) if right != 0.0 => {
            Some(Constant::Number(left / right))
        }

        (Constant::Number(left), "%", Constant::Number(right)) if right != 0.0 => {
            Some(Constant::Number(left % right))
        }

        (Constant::Number(left), "^", Constant::Number(right)) => {
            Some(Constant::Number(left.powf(right)))
        }

        (Constant::String(left), "..", Constant::String(right)) => {
            Some(Constant::String(format!("{left}{right}")))
        }

        (left, "and", right) => Some(if left.truth() { right } else { left }),
        (left, "or", right) => Some(if left.truth() { left } else { right }),
        (left, "==", right) => Some(Constant::Boolean(equal(&left, &right))),
        (left, "~=", right) => Some(Constant::Boolean(!equal(&left, &right))),

        (Constant::Number(left), "<", Constant::Number(right)) => {
            Some(Constant::Boolean(left < right))
        }

        (Constant::Number(left), "<=", Constant::Number(right)) => {
            Some(Constant::Boolean(left <= right))
        }

        (Constant::Number(left), ">", Constant::Number(right)) => {
            Some(Constant::Boolean(left > right))
        }

        (Constant::Number(left), ">=", Constant::Number(right)) => {
            Some(Constant::Boolean(left >= right))
        }

        _ => None,
    }
}

fn equal(left: &Constant, right: &Constant) -> bool {
    match (left, right) {
        (Constant::Nil, Constant::Nil) => true,
        (Constant::Boolean(left), Constant::Boolean(right)) => left == right,

        (Constant::Number(left), Constant::Number(right)) => {
            left.partial_cmp(right) == Some(Ordering::Equal)
        }

        (Constant::String(left), Constant::String(right)) => left == right,
        _ => false,
    }
}

fn compute(view: View<'_, '_>, source: &str, edits: &mut Vec<Edit>) {
    if let Some(value) = evaluate(view) {
        replace_keep_lines(source, span(view), &value.render(), edits);
    }
}

enum Piece {
    Text(String),
    Value(String),
}

pub(super) fn interpolate(view: View<'_, '_>, strategy: &str, edits: &mut Vec<Edit>) {
    let Some(pieces) = interpolation_pieces(view) else {
        return;
    };

    let tostring_strategy = strategy == "tostring";
    let mut format = String::new();
    let mut values = Vec::new();

    for piece in pieces {
        match piece {
            Piece::Text(value) => format.push_str(&value),

            Piece::Value(value) => {
                format.push_str(if tostring_strategy { "%*" } else { "%s" });

                values.push(if tostring_strategy {
                    value.trim().to_owned()
                } else {
                    format!("tostring({})", value.trim())
                });
            }
        }
    }

    let output = if values.is_empty() {
        format!("\"{}\"", format.replace("%%", "%"))
    } else {
        format!("string.format(\"{format}\", {})", values.join(", "))
    };

    edits.push(Edit {
        range: span(view),
        text: output,
    });
}

fn interpolation_pieces(view: View<'_, '_>) -> Option<Vec<Piece>> {
    let Parts::Interpolation { segments } = view.parts()? else {
        return None;
    };

    let segments = segments.collect::<Vec<_>>();
    let mut pieces = Vec::new();

    for (index, segment) in segments.iter().enumerate() {
        if index % 2 == 1 {
            pieces.push(Piece::Value(text(*segment).to_owned()));
            continue;
        }

        let raw = text(*segment);
        let end = raw.len().checked_sub(1)?;
        pieces.push(Piece::Text(interpolation_literal(raw.get(1..end)?)?));
    }

    Some(pieces)
}

fn interpolation_literal(value: &str) -> Option<String> {
    let bytes = value.as_bytes();
    let mut output = String::new();
    let mut index = 0;

    while index < bytes.len() {
        match bytes[index] {
            b'\\' => {
                let next = *bytes.get(index + 1)?;

                if matches!(next, b'{' | b'`') {
                    output.push(char::from(next));
                } else {
                    output.push('\\');
                    output.push(char::from(next));
                }

                index += 2;
            }

            b'%' => {
                output.push_str("%%");
                index += 1;
            }

            b'"' => {
                output.push_str("\\\"");
                index += 1;
            }

            _ => {
                let character = value[index..].chars().next()?;
                output.push(character);
                index += character.len_utf8();
            }
        }
    }

    Some(output)
}
