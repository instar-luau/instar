use std::str;

use vermis::{
    token::{Symbol, TokenKind},
    tree::{NodeIndex, NodeKind, Tree},
};

pub(crate) fn unwrap(tree: &Tree<'_>, mut node: NodeIndex) -> NodeIndex {
    while let NodeKind::Group { expression, .. }
    | NodeKind::Assertion { expression, .. }
    | NodeKind::Instantiate { expression, .. } = tree.node(node).kind
    {
        node = expression;
    }

    node
}

pub(crate) fn number(tree: &Tree<'_>, node: NodeIndex) -> Option<f64> {
    match &tree.node(unwrap(tree, node)).kind {
        NodeKind::Unary { operator, operand }
            if tree.token(*operator).kind == TokenKind::Symbol(Symbol::Subtract) =>
        {
            number(tree, *operand).map(|value| -value)
        }

        NodeKind::Binary {
            left,
            operator,
            right,
        } => {
            let left = number(tree, *left)?;
            let right = number(tree, *right)?;

            match tree.token(*operator).kind {
                TokenKind::Symbol(Symbol::Add) => Some(left + right),
                TokenKind::Symbol(Symbol::Subtract) => Some(left - right),
                TokenKind::Symbol(Symbol::Multiply) => Some(left * right),
                TokenKind::Symbol(Symbol::Divide) => Some(left / right),
                TokenKind::Symbol(Symbol::FloorDivide) => Some((left / right).floor()),
                TokenKind::Symbol(Symbol::Modulo) => Some(left - right * (left / right).floor()),
                TokenKind::Symbol(Symbol::Power) => Some(left.powf(right)),
                _ => None,
            }
        }

        NodeKind::Number { token } => {
            let text = str::from_utf8(tree.token(*token).span.bytes(tree.source))
                .ok()?
                .replace('_', "")
                .to_ascii_lowercase();

            if text.ends_with('i') {
                return None;
            }

            if let Some(digits) = text.strip_prefix("0b") {
                return digits.chars().try_fold(0.0, |value, digit| {
                    digit
                        .to_digit(2)
                        .map(|digit| value * 2.0 + f64::from(digit))
                });
            }

            if let Some(digits) = text.strip_prefix("0x") {
                let (significand, exponent) =
                    digits
                        .split_once('p')
                        .map_or(Some((digits, 0)), |(value, exponent)| {
                            exponent
                                .parse::<i32>()
                                .ok()
                                .map(|exponent| (value, exponent))
                        })?;

                let mut value = 0.0;
                let mut scale = None;

                for digit in significand.chars() {
                    if digit == '.' && scale.is_none() {
                        scale = Some(1.0);
                    } else {
                        let digit = f64::from(digit.to_digit(16)?);

                        if let Some(scale) = &mut scale {
                            *scale /= 16.0;
                            value += digit * *scale;
                        } else {
                            value = value * 16.0 + digit;
                        }
                    }
                }

                Some(value * 2.0_f64.powi(exponent))
            } else {
                text.parse().ok()
            }
        }

        _ => None,
    }
}
