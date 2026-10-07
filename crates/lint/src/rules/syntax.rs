use std::cmp::Ordering::{Equal, Greater, Less};

use vermis::{
    token::{Keyword, Symbol, TokenKind},
    tree::{NodeIndex, NodeKind, Tree},
};

pub(super) fn unwrap(tree: &Tree<'_>, node: NodeIndex) -> NodeIndex {
    match &tree.node(node).kind {
        NodeKind::Group { expression, .. }
        | NodeKind::Assertion { expression, .. }
        | NodeKind::Instantiate { expression, .. } => unwrap(tree, *expression),

        _ => node,
    }
}

pub(super) fn number(tree: &Tree<'_>, node: NodeIndex) -> Option<f64> {
    let node = unwrap(tree, node);

    if let NodeKind::Unary { operator, operand } = &tree.node(node).kind
        && tree.token(*operator).kind == TokenKind::Symbol(Symbol::Subtract)
    {
        return number(tree, *operand).map(|number| -number);
    }

    if let NodeKind::Binary {
        left,
        operator,
        right,
    } = &tree.node(node).kind
    {
        let (left, right) = (number(tree, *left)?, number(tree, *right)?);

        return match tree.token(*operator).kind {
            TokenKind::Symbol(Symbol::Add) => Some(left + right),
            TokenKind::Symbol(Symbol::Subtract) => Some(left - right),
            TokenKind::Symbol(Symbol::Multiply) => Some(left * right),
            TokenKind::Symbol(Symbol::Divide) => Some(left / right),
            TokenKind::Symbol(Symbol::FloorDivide) => Some((left / right).floor()),
            TokenKind::Symbol(Symbol::Modulo) => Some(left - right * (left / right).floor()),
            TokenKind::Symbol(Symbol::Power) => Some(left.powf(right)),
            _ => None,
        };
    }

    if !matches!(tree.node(node).kind, NodeKind::Number { .. }) {
        return None;
    }

    let value = String::from_utf8_lossy(tree.text(node)).replace('_', "");

    if let Some(digits) = value
        .strip_prefix("0x")
        .or_else(|| value.strip_prefix("0X"))
    {
        return integer(digits, 16);
    }

    if let Some(digits) = value
        .strip_prefix("0b")
        .or_else(|| value.strip_prefix("0B"))
    {
        return integer(digits, 2);
    }

    value.parse().ok()
}

pub(super) fn integer(digits: &str, radix: u32) -> Option<f64> {
    if digits.is_empty() {
        return None;
    }

    digits.chars().try_fold(0.0, |value, digit| {
        digit
            .to_digit(radix)
            .map(|digit| value * f64::from(radix) + f64::from(digit))
    })
}

pub(super) fn truth(tree: &Tree<'_>, node: NodeIndex) -> Option<bool> {
    let node = unwrap(tree, node);

    if number(tree, node).is_some() {
        return Some(true);
    }

    match &tree.node(node).kind {
        NodeKind::Nil { .. } => Some(false),

        NodeKind::Boolean { token } => {
            Some(tree.token(*token).kind == TokenKind::Keyword(Keyword::True))
        }

        NodeKind::String { .. } | NodeKind::Table { .. } | NodeKind::Function { .. } => Some(true),

        NodeKind::Unary { operator, operand }
            if tree.token(*operator).kind == TokenKind::Keyword(Keyword::Not) =>
        {
            truth(tree, *operand).map(|value| !value)
        }

        NodeKind::Binary {
            left,
            operator,
            right,
        } => {
            let ordering = number(tree, *left)?.partial_cmp(&number(tree, *right)?);

            match tree.token(*operator).kind {
                TokenKind::Symbol(Symbol::Equal) => Some(ordering == Some(Equal)),
                TokenKind::Symbol(Symbol::NotEqual) => Some(ordering != Some(Equal)),
                TokenKind::Symbol(Symbol::LessThan) => Some(ordering == Some(Less)),
                TokenKind::Symbol(Symbol::GreaterThan) => Some(ordering == Some(Greater)),

                TokenKind::Symbol(Symbol::LessThanOrEqual) => {
                    Some(matches!(ordering, Some(Less | Equal)))
                }

                TokenKind::Symbol(Symbol::GreaterThanOrEqual) => {
                    Some(matches!(ordering, Some(Greater | Equal)))
                }

                _ => None,
            }
        }

        _ => None,
    }
}

pub(super) fn nan(tree: &Tree<'_>, node: NodeIndex) -> bool {
    number(tree, node).is_some_and(f64::is_nan)
}

pub(super) fn comparison(tree: &Tree<'_>, node: NodeIndex) -> bool {
    let NodeKind::Binary { operator, .. } = &tree.node(unwrap(tree, node)).kind else {
        return false;
    };

    matches!(
        tree.token(*operator).kind,
        TokenKind::Symbol(
            Symbol::Equal
                | Symbol::NotEqual
                | Symbol::LessThan
                | Symbol::GreaterThan
                | Symbol::LessThanOrEqual
                | Symbol::GreaterThanOrEqual
        )
    )
}

pub(super) fn range(tree: &Tree<'_>, node: NodeIndex) -> [usize; 2] {
    let span = tree.node(node).span;

    [span.start, span.end]
}

pub(super) fn same(tree: &Tree<'_>, left: NodeIndex, right: NodeIndex) -> bool {
    let tokens = |node: NodeIndex| {
        let span = tree.node(node).span;

        tree.tokens
            .iter()
            .filter(move |token| {
                token.span.start >= span.start
                    && token.span.end <= span.end
                    && !matches!(
                        token.kind,
                        TokenKind::Whitespace | TokenKind::Comment | TokenKind::BlockComment
                    )
            })
            .map(|token| (token.kind, token.bytes(tree.source)))
    };

    tokens(left).eq(tokens(right))
}

pub(super) fn stable(tree: &Tree<'_>, node: NodeIndex) -> bool {
    match &tree.node(node).kind {
        NodeKind::Name { .. } => true,
        NodeKind::Field { receiver, .. } => stable(tree, *receiver),

        NodeKind::Index { receiver, key, .. } => {
            stable(tree, *receiver)
                && matches!(
                    tree.node(*key).kind,
                    NodeKind::String { .. } | NodeKind::Number { .. }
                )
        }

        _ => false,
    }
}

pub(super) fn body(tree: &Tree<'_>, node: NodeIndex) -> NodeIndex {
    if let NodeKind::Else { body, .. } = &tree.node(node).kind {
        *body
    } else {
        node
    }
}

pub(super) fn empty(tree: &Tree<'_>, node: NodeIndex) -> bool {
    let node = body(tree, node);

    matches!(&tree.node(node).kind, NodeKind::Block { statements } if tree.list(statements).is_empty())
}

pub(super) fn empty_table(tree: &Tree<'_>, node: NodeIndex) -> bool {
    matches!(&tree.node(unwrap(tree, node)).kind, NodeKind::Table { fields, .. } if tree.list(fields).is_empty())
}

pub(super) fn single(tree: &Tree<'_>, node: NodeIndex) -> Option<NodeIndex> {
    let NodeKind::Block { statements } = &tree.node(body(tree, node)).kind else {
        return None;
    };

    let [statement] = tree.list(statements) else {
        return None;
    };

    Some(statement.node)
}

pub(super) fn assignment(tree: &Tree<'_>, node: NodeIndex) -> Option<(NodeIndex, NodeIndex)> {
    let NodeKind::Assignment {
        targets, values, ..
    } = &tree.node(node).kind
    else {
        return None;
    };

    let ([target], [value]) = (tree.list(targets), tree.list(values)) else {
        return None;
    };

    Some((target.node, value.node))
}

pub(super) fn arguments_values(tree: &Tree<'_>, node: NodeIndex) -> Vec<NodeIndex> {
    if let NodeKind::Arguments { values, .. } = &tree.node(node).kind {
        tree.list(values).iter().map(|value| value.node).collect()
    } else {
        Vec::new()
    }
}

pub(super) fn in_loop(tree: &Tree<'_>, node: NodeIndex, ancestors: &[NodeIndex]) -> bool {
    for ancestor in ancestors.iter().rev() {
        match &tree.node(*ancestor).kind {
            NodeKind::Function { .. } => return false,

            NodeKind::While { body, .. }
            | NodeKind::Repeat { body, .. }
            | NodeKind::NumericFor { body, .. }
            | NodeKind::GenericFor { body, .. } => {
                let body = tree.node(*body).span;
                let span = tree.node(node).span;

                if body.start <= span.start && span.end <= body.end {
                    return true;
                }
            }

            _ => {}
        }
    }

    false
}

pub(super) fn complexity(tree: &Tree<'_>, node: NodeIndex) -> usize {
    let branch = match &tree.node(node).kind {
        NodeKind::Function { .. } => return 0,

        NodeKind::Branch { .. }
        | NodeKind::While { .. }
        | NodeKind::Repeat { .. }
        | NodeKind::NumericFor { .. }
        | NodeKind::GenericFor { .. }
        | NodeKind::Conditional { .. } => 1,

        NodeKind::Binary { operator, .. } => usize::from(matches!(
            tree.token(*operator).kind,
            TokenKind::Keyword(Keyword::And | Keyword::Or)
        )),

        _ => 0,
    };

    branch
        + tree
            .children(node)
            .into_iter()
            .map(|child| complexity(tree, child))
            .sum::<usize>()
}

pub(super) fn path(tree: &Tree<'_>, node: NodeIndex) -> Option<(NodeIndex, String)> {
    match &tree.node(node).kind {
        NodeKind::Name { .. } => {
            Some((node, String::from_utf8_lossy(tree.text(node)).into_owned()))
        }

        NodeKind::Field { receiver, name, .. }
        | NodeKind::MethodCall {
            receiver,
            method: name,
            ..
        } => {
            let (root, path) = path(tree, *receiver)?;

            Some((
                root,
                format!("{path}.{}", String::from_utf8_lossy(tree.text(*name))),
            ))
        }

        NodeKind::Group { expression, .. } => path(tree, *expression),
        _ => None,
    }
}

pub(super) fn symbol_path(tree: &Tree<'_>, node: NodeIndex) -> Option<String> {
    path(tree, node).map(|(_, path)| path)
}

pub(super) fn builtin(name: &[u8]) -> bool {
    [
        b"assert".as_slice(),
        b"bit32",
        b"buffer",
        b"coroutine",
        b"debug",
        b"error",
        b"getmetatable",
        b"ipairs",
        b"math",
        b"next",
        b"os",
        b"pairs",
        b"pcall",
        b"print",
        b"rawequal",
        b"rawget",
        b"rawset",
        b"require",
        b"select",
        b"setmetatable",
        b"string",
        b"table",
        b"tonumber",
        b"tostring",
        b"type",
        b"typeof",
        b"utf8",
        b"xpcall",
        b"_G",
        b"_VERSION",
    ]
    .contains(&name)
}
