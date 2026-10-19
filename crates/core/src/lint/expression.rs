use std::{cmp::Ordering, collections::BTreeMap, mem};

use vermis::{
    token::{Keyword, Symbol, TokenKind},
    tree::{NodeIndex, NodeKind, NodeList, Tree},
};

use crate::{
    lint::{
        Edit,
        calls::arguments_list,
        context::Context,
        syntax::{body, same, single, truth},
    },
    syntax::{
        bindings::{expands, text},
        expression::{number, unwrap},
        literal,
    },
};

pub(super) const RULES: &[&str] = &[
    "bad_string_escape",
    "compare_nan",
    "constant_condition",
    "constant_table_comparison",
    "length_as_condition",
    "zero_step_loop",
    "divide_by_zero",
    "empty_if",
    "empty_loop",
    "if_same_then_else",
    "mixed_table",
    "and_or_conditional",
    "collapsible_if",
    "else_after_return",
    "negated_condition",
    "parenthesized_conditions",
];

const PRIMITIVES: &[&str] = &[
    "nil", "boolean", "userdata", "number", "string", "table", "function", "thread", "buffer",
    "vector",
];

fn condition(context: &mut Context<'_, '_>, condition: NodeIndex) {
    let tree = context.tree;
    let unwrapped = unwrap(tree, condition);

    if context.active(&["constant_condition"]) && truth(tree, unwrapped).is_some() {
        context.emit(
            condition,
            "constant_condition",
            "condition has a fixed truth value",
            Vec::new(),
        );
    }

    if context.active(&["length_as_condition"])
        && matches!(&tree.node(unwrapped).kind, NodeKind::Unary { operator, .. }
            if tree.token(*operator).kind == TokenKind::Symbol(Symbol::Length))
    {
        context.emit(
            condition,
            "length_as_condition",
            "zero is truthy in Luau; compare the length explicitly",
            Vec::new(),
        );
    }

    if context.active(&["parenthesized_conditions"])
        && matches!(tree.node(condition).kind, NodeKind::Group { .. })
    {
        let mut edits = Vec::new();
        let mut inner = condition;

        while let NodeKind::Group {
            opening,
            expression,
            closing: Some(closing),
        } = &tree.node(inner).kind
        {
            for token in [opening, closing] {
                let span = tree.token(*token).span;

                edits.push(Edit {
                    range: span.start..span.end,
                    expected: context.source[span.start..span.end].to_owned(),
                    replacement: " ".to_owned(),
                });
            }

            inner = *expression;
        }

        context.emit(
            condition,
            "parenthesized_conditions",
            "unnecessary parentheses around condition",
            edits,
        );
    }
}

pub(super) fn visit(context: &mut Context<'_, '_>, node: NodeIndex) {
    let tree = context.tree;

    match &tree.node(node).kind {
        NodeKind::Branch { condition, .. }
        | NodeKind::While { condition, .. }
        | NodeKind::Repeat { condition, .. }
        | NodeKind::Conditional { condition, .. } => self::condition(context, *condition),

        _ => {}
    }

    match &tree.node(node).kind {
        NodeKind::String { token }
            if context.active(&["bad_string_escape"])
                && tree.token(*token).kind != TokenKind::RawString =>
        {
            let interpolated = tree.token(*token).kind != TokenKind::QuotedString;
            let mut bytes = tree.token(*token).bytes(tree.source).iter();

            while let Some(byte) = bytes.next() {
                if *byte == b'\\'
                    && let Some(escape) = bytes.next()
                    && !matches!(
                        escape,
                        b'a' | b'b' | b'f' | b'n' | b'r' | b't' | b'v' | b'z' | b'x' | b'u' | b'0'
                            ..=b'9' | b'\\' | b'\'' | b'"' | b'\n' | b'\r'
                    )
                    && !(interpolated && matches!(escape, b'`' | b'{'))
                {
                    context.emit(
                        node,
                        "bad_string_escape",
                        "unrecognized string escape",
                        Vec::new(),
                    );

                    break;
                }
            }
        }

        NodeKind::NumericFor {
            step: Some(step), ..
        } if context.active(&["zero_step_loop"]) && number(tree, *step) == Some(0.0) => {
            context.emit(
                *step,
                "zero_step_loop",
                "numeric loop step is zero",
                Vec::new(),
            );
        }

        NodeKind::While { body, .. }
        | NodeKind::Repeat { body, .. }
        | NodeKind::NumericFor { body, .. }
        | NodeKind::GenericFor { body, .. }
            if context.active(&["empty_loop"]) && empty(tree, *body) =>
        {
            context.emit(node, "empty_loop", "empty loop body", Vec::new());
        }

        NodeKind::If {
            branches,
            otherwise,
            ..
        } if context.active(&[
            "empty_if",
            "if_same_then_else",
            "else_after_return",
            "negated_condition",
            "collapsible_if",
        ]) =>
        {
            for branch in tree.list(branches) {
                if context.active(&["empty_if"])
                    && let NodeKind::Branch { body, .. } = tree.node(branch.node).kind
                    && empty(tree, body)
                {
                    context.emit(
                        branch.node,
                        "empty_if",
                        "empty conditional branch",
                        Vec::new(),
                    );
                }
            }

            if let Some(otherwise) = otherwise {
                let bodies: Vec<_> = tree
                    .list(branches)
                    .iter()
                    .filter_map(|branch| {
                        if let NodeKind::Branch { body, .. } = tree.node(branch.node).kind {
                            Some(body)
                        } else {
                            None
                        }
                    })
                    .collect();

                if context.active(&["empty_if"]) && empty(tree, *otherwise) {
                    context.emit(
                        *otherwise,
                        "empty_if",
                        "empty conditional branch",
                        Vec::new(),
                    );
                }

                if context.active(&["if_same_then_else"])
                    && let Some(first) = bodies.first()
                    && bodies.iter().all(|body| same(tree, *first, *body))
                    && same(tree, *first, body(tree, *otherwise))
                {
                    context.emit(
                        node,
                        "if_same_then_else",
                        "conditional branches have identical bodies",
                        Vec::new(),
                    );
                }

                if context.active(&["else_after_return"])
                    && bodies.iter().all(|body| {
                        let NodeKind::Block { statements } = &tree.node(*body).kind else {
                            return false;
                        };

                        tree.list(statements).last().is_some_and(|statement| {
                            matches!(tree.node(statement.node).kind, NodeKind::Return { .. })
                        })
                    })
                {
                    context.emit(
                        *otherwise,
                        "else_after_return",
                        "else follows branches that return",
                        Vec::new(),
                    );
                }

                if context.active(&["negated_condition"])
                    && let [branch] = tree.list(branches)
                    && let NodeKind::Branch { condition, .. } = tree.node(branch.node).kind
                    && matches!(&tree.node(unwrap(tree, condition)).kind, NodeKind::Unary { operator, .. }
                        if tree.token(*operator).kind == TokenKind::Keyword(Keyword::Not))
                {
                    context.emit(
                        condition,
                        "negated_condition",
                        "negated condition can be expressed by swapping branches",
                        Vec::new(),
                    );
                }
            } else if let [branch] = tree.list(branches)
                && context.active(&["collapsible_if"])
                && let NodeKind::Branch { body, .. } = tree.node(branch.node).kind
                && let Some(inner) = single(tree, body)
                && let NodeKind::If {
                    branches,
                    otherwise: None,
                    ..
                } = &tree.node(inner).kind
                && tree.list(branches).len() == 1
            {
                context.emit(
                    node,
                    "collapsible_if",
                    "nested single-branch conditionals can be combined",
                    Vec::new(),
                );
            }
        }

        NodeKind::Binary {
            left,
            operator,
            right,
        } => {
            let operator = tree.token(*operator).kind;

            if context.active(&["divide_by_zero"])
                && matches!(
                    operator,
                    TokenKind::Symbol(Symbol::Divide | Symbol::FloorDivide | Symbol::Modulo)
                )
                && number(tree, *right) == Some(0.0)
            {
                context.emit(
                    *right,
                    "divide_by_zero",
                    "division or modulo by zero",
                    Vec::new(),
                );
            }

            if matches!(
                operator,
                TokenKind::Symbol(Symbol::Equal | Symbol::NotEqual)
            ) {
                if context.active(&["compare_nan"])
                    && (number(tree, *left).is_some_and(f64::is_nan)
                        || number(tree, *right).is_some_and(f64::is_nan))
                {
                    context.emit(
                        node,
                        "compare_nan",
                        "comparison with NaN has a fixed result",
                        Vec::new(),
                    );
                }

                if context.active(&["constant_table_comparison"])
                    && (matches!(tree.node(unwrap(tree, *left)).kind, NodeKind::Table { .. })
                        || matches!(tree.node(unwrap(tree, *right)).kind, NodeKind::Table { .. }))
                {
                    context.emit(
                        node,
                        "constant_table_comparison",
                        "fresh tables are compared by identity",
                        Vec::new(),
                    );
                }
            }

            if context.active(&["and_or_conditional"])
                && operator == TokenKind::Keyword(Keyword::Or)
                && let NodeKind::Binary {
                    operator, right, ..
                } = &tree.node(unwrap(tree, *left)).kind
                && tree.token(*operator).kind == TokenKind::Keyword(Keyword::And)
                && truth(tree, *right) != Some(false)
            {
                context.emit(
                    node,
                    "and_or_conditional",
                    "use an if expression instead of an and/or conditional",
                    Vec::new(),
                );
            }
        }

        NodeKind::Table { fields, .. } if context.active(&["mixed_table"]) => {
            let mut positional = false;
            let mut keyed = false;

            for field in tree.list(fields) {
                if let NodeKind::TableField { key, opening, .. } = &tree.node(field.node).kind {
                    if key.is_some() || opening.is_some() {
                        keyed = true;
                    } else {
                        positional = true;
                    }
                }
            }

            if positional && keyed {
                context.emit(
                    node,
                    "mixed_table",
                    "table mixes positional and keyed entries",
                    Vec::new(),
                );
            }
        }

        _ => {}
    }
}

pub(super) fn analyze(context: &mut Context<'_, '_>, node: NodeIndex) {
    let tree = context.tree;

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

            if context.active(&["UnbalancedAssignment"])
                && let Some(last) = values.last()
                && (values.len() > targets.len()
                    || (values.len() < targets.len()
                        && !expands(tree, last.node)
                        && !matches!(tree.node(last.node).kind, NodeKind::Nil { .. })))
            {
                context.warn(
                    node,
                    "UnbalancedAssignment",
                    "assignment has a different number of values and bindings",
                );
            }

            if matches!(tree.node(node).kind, NodeKind::Assignment { .. }) {
                for (target, value) in targets.iter().zip(values) {
                    if context.active(&["self_assignment"])
                        && context.bindings().same(tree, target.node, value.node)
                    {
                        let span = tree.node(node).span;

                        let fixable = targets.len() == 1
                            && values.len() == 1
                            && context.bindings().references[target.node.get()].is_some()
                            && !tree.tokens.iter().any(|token| {
                                matches!(token.kind, TokenKind::Comment | TokenKind::BlockComment)
                                    && token.span.start < span.end
                                    && span.start < token.span.end
                            });

                        let edits = if fixable {
                            vec![Edit {
                                range: span.start..span.end,
                                expected: context.source[span.start..span.end].to_owned(),
                                replacement: ";".to_owned(),
                            }]
                        } else {
                            Vec::new()
                        };

                        context.emit(
                            if fixable { node } else { target.node },
                            "self_assignment",
                            "assignment leaves a value unchanged",
                            edits,
                        );
                    }
                }

                if context.active(&["string_concat_in_loop"])
                    && let ([target], [value]) = (targets, values)
                    && context.bindings().loops[node.get()].is_some()
                    && let NodeKind::Binary { left, operator, .. } =
                        tree.node(unwrap(tree, value.node)).kind
                    && tree.token(operator).kind == TokenKind::Symbol(Symbol::Concatenate)
                    && context.bindings().same(tree, target.node, left)
                {
                    context.warn(
                        node,
                        "string_concat_in_loop",
                        "repeated concatenation grows a string quadratically",
                    );
                }
            }
        }

        NodeKind::CompoundAssignment { operator, .. }
            if context.active(&["string_concat_in_loop"])
                && context.bindings().loops[node.get()].is_some()
                && tree.token(*operator).kind
                    == TokenKind::Symbol(Symbol::ConcatenateAssignment) =>
        {
            context.warn(
                node,
                "string_concat_in_loop",
                "repeated concatenation grows a string quadratically",
            );
        }

        NodeKind::Binary {
            left,
            operator,
            right,
        } if context.active(&[
            "ComparisonPrecedence",
            "UnknownType",
            "MisleadingAndOr",
            "DuplicateCondition",
        ]) =>
        {
            binary(context, node, *left, tree.token(*operator).kind, *right);
        }

        NodeKind::GenericFor {
            bindings,
            values,
            body,
            ..
        } if context.active(&["manual_table_clone"]) => {
            if let ([key, value], [iterator], Some(statement)) =
                (tree.list(bindings), tree.list(values), single(tree, *body))
                && let NodeKind::Call { callee, arguments } =
                    tree.node(unwrap(tree, iterator.node)).kind
                && context.bindings().global(tree, callee, "pairs")
                && arguments_list(tree, arguments).len() == 1
                && let Some((target, assigned)) = assignment(tree, statement)
                && let NodeKind::Index {
                    receiver,
                    key: indexed,
                    ..
                } = tree.node(target).kind
                && context.bindings().references[indexed.get()]
                    == context.bindings().references[key.node.get()]
                && context.bindings().references[assigned.get()]
                    == context.bindings().references[value.node.get()]
                && let Some(destination) = context.bindings().references[receiver.get()]
                && !context.bindings().locals[destination].assigned
                && let Some(initializer) = context.bindings().locals[destination].value
                && matches!(
                    &tree.node(unwrap(tree, initializer)).kind,
                    NodeKind::Table { fields, .. } if tree.list(fields).is_empty()
                )
            {
                context.warn(
                    node,
                    "manual_table_clone",
                    "table-copy loop can use table.clone",
                );
            }
        }

        NodeKind::Table { fields, .. } | NodeKind::TypeTable { fields, .. }
            if context.active(&["TableLiteral"]) =>
        {
            table(context, fields);
        }

        NodeKind::Number { .. } if context.active(&["IntegerParsing"]) => {
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

                let imprecise = if integer.is_some() {
                    parsed.is_none_or(|value| value > (1_u128 << 63))
                } else {
                    parsed.is_none_or(|value| {
                        value > u128::from(u64::MAX)
                            || (value > (1_u128 << 53)
                                && value.trailing_zeros() < 128 - value.leading_zeros() - 53)
                    })
                };

                if imprecise {
                    context.warn(
                        node,
                        "IntegerParsing",
                        "number literal exceeds its available precision or range",
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

    if context.active(&["ComparisonPrecedence"]) && comparison(operator) {
        let negated = |node| matches!(tree.node(node).kind, NodeKind::Unary { operator, .. } if tree.token(operator).kind == TokenKind::Keyword(Keyword::Not));

        if (negated(left) && !negated(right))
            || matches!(tree.node(left).kind, NodeKind::Binary { operator, .. } if comparison(tree.token(operator).kind))
        {
            context.warn(
                node,
                "ComparisonPrecedence",
                "comparison precedence is misleading; add parentheses or separate the comparisons",
            );
        }
    }

    if context.active(&["UnknownType"])
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
                let primitive = PRIMITIVES.contains(&name.as_str());
                let valid = primitive || context.symbols.types.contains(&name);

                if (context.bindings().global(tree, callee, "type") && !primitive)
                    || (context.bindings().global(tree, callee, "typeof") && !valid)
                {
                    context.warn(literal, "UnknownType", &format!("unknown type `{name}`"));
                }
            }
        }
    }

    if context.active(&["MisleadingAndOr"])
        && operator == TokenKind::Keyword(Keyword::Or)
        && let NodeKind::Binary {
            operator, right, ..
        } = tree.node(left).kind
        && tree.token(operator).kind == TokenKind::Keyword(Keyword::And)
        && matches!(
            tree.node(right).kind,
            NodeKind::Nil { .. } | NodeKind::Boolean { .. }
        )
        && truth(tree, right) == Some(false)
    {
        context.warn(
            node,
            "MisleadingAndOr",
            "and-or expression always evaluates to its second alternative",
        );
    }

    if context.active(&["DuplicateCondition"])
        && matches!(operator, TokenKind::Keyword(Keyword::And | Keyword::Or))
    {
        let parent = context.bindings().parents[node.get()];
        let in_chain = parent.is_some_and(|parent| matches!(tree.node(parent).kind, NodeKind::Binary { operator: parent_operator, .. } if tree.token(parent_operator).kind == operator));

        let ternary = parent.is_some_and(|parent| matches!(tree.node(parent).kind, NodeKind::Binary { operator: parent_operator, left: parent_left, .. } if parent_left == node && tree.token(parent_operator).kind == TokenKind::Keyword(Keyword::Or)))
            && operator == TokenKind::Keyword(Keyword::And)
            && ![left, right].iter().any(|child| matches!(tree.node(*child).kind, NodeKind::Binary { operator: child_operator, .. } if tree.token(child_operator).kind == operator));

        if !in_chain && !ternary {
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
            .any(|previous| similar(context, *previous, *condition))
        {
            context.warn(
                *condition,
                "DuplicateCondition",
                "condition has already been checked",
            );
        }
    }
}

fn similar(context: &Context<'_, '_>, left: NodeIndex, right: NodeIndex) -> bool {
    let tree = context.tree;
    let left_kind = &tree.node(left).kind;
    let right_kind = &tree.node(right).kind;

    if mem::discriminant(left_kind) != mem::discriminant(right_kind) {
        return false;
    }

    match (left_kind, right_kind) {
        (NodeKind::Name { .. }, _) => context.bindings().same(tree, left, right),

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
                same(tree, left, right)
            } else {
                left_children.len() == right_children.len()
                    && left_children
                        .iter()
                        .zip(right_children)
                        .all(|(left, right)| similar(context, *left, right))
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
        let (key, identifier, access) = match tree.node(field.node).kind {
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

        let key_value = if identifier {
            Some((true, tree.text(key).to_vec()))
        } else if let Some(value) = string(tree, key) {
            Some((true, value))
        } else if let Some(value) = number(tree, key)
            .filter(|value| (0.0..=f64::from(i32::MAX)).contains(value) && value.fract() == 0.0)
        {
            if value >= 1.0 && value <= f64::from(u32::try_from(count).unwrap_or(u32::MAX)) {
                context.warn(key, "TableLiteral", "table index duplicates a list entry");
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
                context.warn(key, "TableLiteral", "table field is a duplicate");
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

pub(super) fn comparison(operator: TokenKind) -> bool {
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

fn empty(tree: &Tree<'_>, node: NodeIndex) -> bool {
    matches!(&tree.node(body(tree, node)).kind, NodeKind::Block { statements } if tree.list(statements).is_empty())
}
