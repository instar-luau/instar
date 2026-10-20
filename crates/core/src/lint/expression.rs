use std::{cmp::Ordering, collections::BTreeMap, mem};

use vermis::{
    token::{Keyword, Symbol, TokenKind},
    tree::{NodeIndex, NodeKind, NodeList, Tree},
};

use crate::{
    lint::{
        Edit,
        context::Context,
        syntax::{is_same, is_truthy},
    },
    syntax::{
        bindings::{can_expand, text},
        expression::{number, unwrap},
        literal,
    },
};

const PRIMITIVES: &[&str] = &[
    "nil", "boolean", "userdata", "number", "string", "table", "function", "thread", "buffer",
    "vector",
];

pub(super) fn check(context: &mut Context<'_, '_>, node: NodeIndex) {
    let tree = context.tree;

    match &tree.node(node).kind {
        NodeKind::String { token }
            if context.is_active(&["bad_string_escape"])
                && tree.token(*token).kind != TokenKind::RawString =>
        {
            let is_interpolated = tree.token(*token).kind != TokenKind::QuotedString;
            let mut bytes = tree.token(*token).bytes(tree.source).iter();

            while let Some(byte) = bytes.next() {
                if *byte == b'\\'
                    && let Some(escape) = bytes.next()
                    && !matches!(
                        escape,
                        b'a' | b'b' | b'f' | b'n' | b'r' | b't' | b'v' | b'z' | b'x' | b'u' | b'0'
                            ..=b'9' | b'\\' | b'\'' | b'"' | b'\n' | b'\r'
                    )
                    && !(is_interpolated && matches!(escape, b'`' | b'{'))
                {
                    context.emit(
                        node,
                        "bad_string_escape",
                        "String contains an unrecognized escape sequence.",
                    );

                    break;
                }
            }
        }

        NodeKind::Binary {
            left,
            operator,
            right,
        } => {
            let operator = tree.token(*operator).kind;

            if context.is_active(&["divide_by_zero"])
                && matches!(
                    operator,
                    TokenKind::Symbol(Symbol::Divide | Symbol::FloorDivide | Symbol::Modulo)
                )
                && number(tree, *right) == Some(0.0)
            {
                context.emit(
                    *right,
                    "divide_by_zero",
                    "Division or modulo uses a zero divisor.",
                );
            }

            if matches!(
                operator,
                TokenKind::Symbol(Symbol::Equal | Symbol::NotEqual)
            ) {
                if context.is_active(&["compare_nan"])
                    && (number(tree, *left).is_some_and(f64::is_nan)
                        || number(tree, *right).is_some_and(f64::is_nan))
                {
                    context.emit(
                        node,
                        "compare_nan",
                        "Comparison with NaN has a fixed result.",
                    );
                }

                if context.is_active(&["constant_table_comparison"])
                    && (matches!(tree.node(unwrap(tree, *left)).kind, NodeKind::Table { .. })
                        || matches!(tree.node(unwrap(tree, *right)).kind, NodeKind::Table { .. }))
                {
                    context.emit(
                        node,
                        "constant_table_comparison",
                        "Fresh tables are compared by identity.",
                    );
                }
            }

            if context.is_active(&["and_or_conditional"])
                && operator == TokenKind::Keyword(Keyword::Or)
                && let NodeKind::Binary {
                    operator, right, ..
                } = &tree.node(unwrap(tree, *left)).kind
                && tree.token(*operator).kind == TokenKind::Keyword(Keyword::And)
                && is_truthy(tree, *right) != Some(false)
            {
                context.emit(node, "and_or_conditional", "An `and`/`or` expression is used as a conditional; consider an `if` expression.");
            }
        }

        NodeKind::Table { fields, .. } if context.is_active(&["mixed_table"]) => {
            let mut has_positional_fields = false;
            let mut has_keyed_fields = false;

            for field in tree.list(fields) {
                if let NodeKind::TableField { key, opening, .. } = &tree.node(field.node).kind {
                    if key.is_some() || opening.is_some() {
                        has_keyed_fields = true;
                    } else {
                        has_positional_fields = true;
                    }
                }
            }

            if has_positional_fields && has_keyed_fields {
                context.emit(
                    node,
                    "mixed_table",
                    "Table mixes positional and keyed entries.",
                );
            }
        }

        _ => {}
    }

    match &tree.node(node).kind {
        NodeKind::Assignment {
            targets, values, ..
        }
        | NodeKind::Local {
            bindings: targets,
            values,
            ..
        }
        | NodeKind::Constant {
            bindings: targets,
            values,
            ..
        } => {
            let targets = tree.list(targets);
            let values = tree.list(values);

            if context.is_active(&["UnbalancedAssignment"])
                && let Some(last) = values.last()
                && (values.len() > targets.len()
                    || (values.len() < targets.len()
                        && !can_expand(tree, last.node)
                        && !matches!(tree.node(last.node).kind, NodeKind::Nil { .. })))
            {
                context.emit(
                    node,
                    "UnbalancedAssignment",
                    "Assignment has a different number of values and bindings.",
                );
            }

            if matches!(tree.node(node).kind, NodeKind::Assignment { .. }) {
                for (target, value) in targets.iter().zip(values) {
                    if context.is_active(&["self_assignment"])
                        && context.bindings().is_same(tree, target.node, value.node)
                    {
                        let span = tree.node(node).span;

                        let is_fixable = targets.len() == 1
                            && values.len() == 1
                            && context.bindings().references[target.node.get()].is_some()
                            && !tree.tokens.iter().any(|token| {
                                matches!(token.kind, TokenKind::Comment | TokenKind::BlockComment)
                                    && token.span.start < span.end
                                    && span.start < token.span.end
                            });

                        let edits = if is_fixable {
                            vec![Edit {
                                range: span.start..span.end,
                                expected: context.source[span.start..span.end].to_owned(),
                                replacement: ";".to_owned(),
                            }]
                        } else {
                            Vec::new()
                        };

                        context.emit_with_edits(
                            if is_fixable { node } else { target.node },
                            "self_assignment",
                            "Assignment leaves a value unchanged.",
                            edits,
                        );
                    }
                }

                if context.is_active(&["string_concat_in_loop"])
                    && let ([target], [value]) = (targets, values)
                    && context.bindings().loops[node.get()].is_some()
                    && let NodeKind::Binary { left, operator, .. } =
                        tree.node(unwrap(tree, value.node)).kind
                    && tree.token(operator).kind == TokenKind::Symbol(Symbol::Concatenate)
                    && context.bindings().is_same(tree, target.node, left)
                {
                    context.emit(
                        node,
                        "string_concat_in_loop",
                        "String concatenation inside a loop can cause quadratic copying.",
                    );
                }
            }
        }

        NodeKind::CompoundAssignment { operator, .. }
            if context.is_active(&["string_concat_in_loop"])
                && context.bindings().loops[node.get()].is_some()
                && tree.token(*operator).kind
                    == TokenKind::Symbol(Symbol::ConcatenateAssignment) =>
        {
            context.emit(
                node,
                "string_concat_in_loop",
                "String concatenation inside a loop can cause quadratic copying.",
            );
        }

        NodeKind::Binary {
            left,
            operator,
            right,
        } if context.is_active(&[
            "ComparisonPrecedence",
            "UnknownType",
            "MisleadingAndOr",
            "DuplicateCondition",
        ]) =>
        {
            binary(context, node, *left, tree.token(*operator).kind, *right);
        }

        NodeKind::Table { fields, .. } | NodeKind::TypeTable { fields, .. }
            if context.is_active(&["TableLiteral"]) =>
        {
            table(context, fields);
        }

        NodeKind::Number { .. } if context.is_active(&["IntegerParsing"]) => {
            let literal = text(tree, node).replace('_', "").to_ascii_lowercase();
            let integer = literal.strip_suffix('i');
            let digits = integer.unwrap_or(&literal);

            let (digits, radix) = if let Some(digits) = digits.strip_prefix("0x") {
                (digits, 16)
            } else if let Some(digits) = digits.strip_prefix("0b") {
                (digits, 2)
            } else {
                (digits, 10)
            };

            if !digits.is_empty() && digits.chars().all(|digit| digit.is_digit(radix)) {
                let parsed = u128::from_str_radix(digits, radix).ok();

                let is_imprecise = if integer.is_some() {
                    parsed.is_none_or(|value| value > (1_u128 << 63))
                } else {
                    parsed.is_none_or(|value| {
                        value > u128::from(u64::MAX)
                            || (value > (1_u128 << 53)
                                && value.trailing_zeros() < 128 - value.leading_zeros() - 53)
                    })
                };

                if is_imprecise {
                    context.emit(
                        node,
                        "IntegerParsing",
                        "Number literal exceeds its available precision or range.",
                    );
                }
            }
        }

        _ => {}
    }
}

fn binary(
    context: &mut Context<'_, '_>,
    node: NodeIndex,
    left: NodeIndex,
    operator: TokenKind,
    right: NodeIndex,
) {
    let tree = context.tree;

    if context.is_active(&["ComparisonPrecedence"]) && is_comparison(operator) {
        let is_negated = |node| matches!(tree.node(node).kind, NodeKind::Unary { operator, .. } if tree.token(operator).kind == TokenKind::Keyword(Keyword::Not));

        if (is_negated(left) && !is_negated(right))
            || matches!(tree.node(left).kind, NodeKind::Binary { operator, .. } if is_comparison(tree.token(operator).kind))
        {
            context.emit(
                node,
                "ComparisonPrecedence",
                "Comparison precedence may be misleading; add parentheses or separate the comparisons.",
            );
        }
    }

    if context.is_active(&["UnknownType"])
        && matches!(
            operator,
            TokenKind::Symbol(Symbol::Equal | Symbol::NotEqual)
        )
    {
        for (call, literal) in [(left, right), (right, left)] {
            if let NodeKind::Call { callee, .. } = tree.node(call).kind
                && let Some(literal_text) = string(tree, literal)
                && let Ok(name) = String::from_utf8(literal_text)
            {
                let is_primitive = PRIMITIVES.contains(&name.as_str());
                let is_valid = is_primitive || context.symbols.types.contains(&name);

                if (context.bindings().is_global(tree, callee, "type") && !is_primitive)
                    || (context.bindings().is_global(tree, callee, "typeof") && !is_valid)
                {
                    context.emit(
                        literal,
                        "UnknownType",
                        &format!("Type name `{name}` is not recognized by this check."),
                    );
                }
            }
        }
    }

    if context.is_active(&["MisleadingAndOr"])
        && operator == TokenKind::Keyword(Keyword::Or)
        && let NodeKind::Binary {
            operator, right, ..
        } = tree.node(left).kind
        && tree.token(operator).kind == TokenKind::Keyword(Keyword::And)
        && matches!(
            tree.node(right).kind,
            NodeKind::Nil { .. } | NodeKind::Boolean { .. }
        )
        && is_truthy(tree, right) == Some(false)
    {
        context.emit(
            node,
            "MisleadingAndOr",
            "This `and`/`or` expression always evaluates its second alternative.",
        );
    }

    if context.is_active(&["DuplicateCondition"])
        && matches!(operator, TokenKind::Keyword(Keyword::And | Keyword::Or))
    {
        let parent = context.bindings().parents[node.get()];
        let is_in_chain = parent.is_some_and(|parent| matches!(tree.node(parent).kind, NodeKind::Binary { operator: parent_operator, .. } if tree.token(parent_operator).kind == operator));

        let is_ternary = parent.is_some_and(|parent| matches!(tree.node(parent).kind, NodeKind::Binary { operator: parent_operator, left: parent_left, .. } if parent_left == node && tree.token(parent_operator).kind == TokenKind::Keyword(Keyword::Or)))
            && operator == TokenKind::Keyword(Keyword::And)
            && ![left, right].iter().any(|child| matches!(tree.node(*child).kind, NodeKind::Binary { operator: child_operator, .. } if tree.token(child_operator).kind == operator));

        if !is_in_chain && !is_ternary {
            let mut conditions = Vec::new();
            chain(tree, node, operator, &mut conditions);
            duplicates(context, &conditions);
        }
    }
}

pub(super) fn duplicates(context: &mut Context<'_, '_>, conditions: &[NodeIndex]) {
    for (index, condition) in conditions.iter().enumerate() {
        if conditions[index.saturating_sub(5)..index]
            .iter()
            .any(|previous| is_similar(context, *previous, *condition))
        {
            context.emit(
                *condition,
                "DuplicateCondition",
                "Condition repeats an earlier check.",
            );
        }
    }
}

fn is_similar(context: &Context<'_, '_>, left: NodeIndex, right: NodeIndex) -> bool {
    let tree = context.tree;
    let left_kind = &tree.node(left).kind;
    let right_kind = &tree.node(right).kind;

    if mem::discriminant(left_kind) != mem::discriminant(right_kind) {
        return false;
    }

    match (left_kind, right_kind) {
        (NodeKind::Name { .. }, _) => context.bindings().is_same(tree, left, right),

        (NodeKind::Number { .. }, _) => number(tree, left)
            .zip(number(tree, right))
            .is_some_and(|(left, right)| left.partial_cmp(&right) == Some(Ordering::Equal)),

        (NodeKind::String { .. }, _) => string(tree, left)
            .zip(string(tree, right))
            .is_some_and(|(left, right)| left == right),

        (NodeKind::Function { .. } | NodeKind::Assertion { .. }, _) => false,

        (
            NodeKind::Binary { operator: left, .. } | NodeKind::Unary { operator: left, .. },
            NodeKind::Binary {
                operator: right, ..
            }
            | NodeKind::Unary {
                operator: right, ..
            },
        ) if tree.token(*left).kind != tree.token(*right).kind => false,

        _ => {
            let left_children = tree.children(left);
            let right_children = tree.children(right);

            if left_children.is_empty() {
                is_same(tree, left, right)
            } else {
                left_children.len() == right_children.len()
                    && left_children
                        .iter()
                        .zip(right_children)
                        .all(|(left, right)| is_similar(context, *left, right))
            }
        }
    }
}

fn table(context: &mut Context<'_, '_>, fields: &NodeList) {
    let tree = context.tree;
    let fields = tree.list(fields);

    let count = fields
        .iter()
        .filter(|entry| {
            matches!(
                tree.node(entry.node).kind,
                NodeKind::TableField { key: None, .. }
            )
        })
        .count();

    let mut names = BTreeMap::<(bool, Vec<u8>), u8>::new();

    for field in fields {
        let (key, is_identifier, access) = match tree.node(field.node).kind {
            NodeKind::TableField {
                key: Some(key),
                opening,
                ..
            } => (key, opening.is_none(), 3),

            NodeKind::TypeField { key, access, .. } => (
                key,
                matches!(tree.node(key).kind, NodeKind::Name { .. }),
                access.map_or(3, |access| {
                    if tree.token(access).bytes(tree.source) == b"read" {
                        1
                    } else {
                        2
                    }
                }),
            ),

            _ => continue,
        };

        let key_value = if is_identifier {
            Some((true, tree.text(key).to_vec()))
        } else if let Some(value) = string(tree, key) {
            Some((true, value))
        } else if let Some(value) = number(tree, key)
            .filter(|value| (0.0..=f64::from(i32::MAX)).contains(value) && value.fract() == 0.0)
        {
            if value >= 1.0 && value <= f64::from(u32::try_from(count).unwrap_or(u32::MAX)) {
                context.emit(key, "TableLiteral", "Table index duplicates a list entry.");
            }

            Some((
                false,
                if value == 0.0 {
                    "0".to_owned()
                } else {
                    value.to_string()
                }
                .into_bytes(),
            ))
        } else {
            None
        };

        if let Some(key_value) = key_value {
            let previous = names.entry(key_value).or_default();

            if *previous & access != 0 {
                context.emit(key, "TableLiteral", "Table field is duplicated.");
            }

            *previous |= access;
        }
    }
}

pub(super) fn assignment(tree: &Tree<'_>, node: NodeIndex) -> Option<(NodeIndex, NodeIndex)> {
    if let NodeKind::Assignment {
        targets, values, ..
    } = &tree.node(node).kind
        && let ([target], [value]) = (tree.list(targets), tree.list(values))
    {
        Some((target.node, value.node))
    } else {
        None
    }
}

pub(super) fn is_comparison(operator: TokenKind) -> bool {
    matches!(
        operator,
        TokenKind::Symbol(
            Symbol::Equal
                | Symbol::NotEqual
                | Symbol::LessThan
                | Symbol::LessThanOrEqual
                | Symbol::GreaterThan
                | Symbol::GreaterThanOrEqual
        )
    )
}

fn chain(tree: &Tree<'_>, node: NodeIndex, operator: TokenKind, conditions: &mut Vec<NodeIndex>) {
    match tree.node(node).kind {
        NodeKind::Binary {
            left,
            operator: current,
            right,
        } if tree.token(current).kind == operator => {
            chain(tree, left, operator, conditions);
            chain(tree, right, operator, conditions);
        }

        NodeKind::Group { expression, .. } => chain(tree, expression, operator, conditions),
        _ => conditions.push(node),
    }
}

pub(super) fn string(tree: &Tree<'_>, node: NodeIndex) -> Option<Vec<u8>> {
    let NodeKind::String { token } = tree.node(unwrap(tree, node)).kind else {
        return None;
    };

    let token = tree.token(token);

    if !matches!(
        token.kind,
        TokenKind::RawString | TokenKind::QuotedString | TokenKind::InterpolatedStringSimple
    ) {
        return None;
    }

    literal::decode(token.bytes(tree.source))
}
