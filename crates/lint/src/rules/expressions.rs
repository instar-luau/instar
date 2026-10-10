use std::cmp::Ordering::{Equal, Greater, Less};

use instar_shared::Options;
use instar_syntax::literal;

use instar_syntax::{
    token::{Keyword, Symbol, TokenKind},
    tree::{NodeIndex, NodeKind, Tree},
};

use crate::{RuleIdentity, RuleInput, RuleResult, RuleSettings, request::Context};

impl Context<'_, '_> {
    fn cloning(&mut self, node: NodeIndex) {
        let tree = self.tree;

        let NodeKind::GenericFor {
            bindings,
            values,
            body,
            ..
        } = &tree.node(node).kind
        else {
            return;
        };

        let bindings = tree.list(bindings);
        let values = tree.list(values);

        if bindings.len() != 2 || values.len() != 1 {
            return;
        }

        let (NodeKind::Binding { name: key, .. }, NodeKind::Binding { name: value, .. }) = (
            &tree.node(bindings[0].node).kind,
            &tree.node(bindings[1].node).kind,
        ) else {
            return;
        };

        let NodeKind::Call { callee, arguments } = &tree.node(values[0].node).kind else {
            return;
        };

        if !self.global(*callee, b"pairs") || arguments_values(tree, *arguments).len() != 1 {
            return;
        }

        let Some(statement) = single(tree, *body) else {
            return;
        };

        let Some((target, assigned)) = assignment(tree, statement) else {
            return;
        };

        let NodeKind::Index {
            receiver,
            key: assigned_key,
            ..
        } = &tree.node(target).kind
        else {
            return;
        };

        let Some(destination) = self.bindings.declaration(*receiver) else {
            return;
        };

        if destination.assigned
            || !destination
                .value
                .is_some_and(|value| empty_table(tree, value))
        {
            return;
        }

        if matches!(tree.node(assigned).kind, NodeKind::Name { .. })
            && same(tree, assigned, *value)
            && same(tree, *assigned_key, *key)
            && self.bindings.references.get(&assigned.get()) == Some(&value.get())
            && self.bindings.references.get(&assigned_key.get()) == Some(&key.get())
        {
            self.finding(
                node,
                RuleIdentity::ManualTableClone,
                "table-copy loop can use table.clone",
            );
        }
    }

    fn divide(&mut self, node: NodeIndex) {
        let Some((_, operator, right)) = self.operands(node) else {
            return;
        };

        if matches!(
            operator,
            TokenKind::Symbol(Symbol::Divide | Symbol::FloorDivide | Symbol::Modulo)
        ) && number(self.tree, right) == Some(0.0)
        {
            self.finding(
                right,
                RuleIdentity::DivideByZero,
                "division or modulo by zero",
            );
        }
    }

    fn compare_not_a_number(&mut self, node: NodeIndex) {
        let Some((left, operator, right)) = self.operands(node) else {
            return;
        };

        if matches!(
            operator,
            TokenKind::Symbol(Symbol::Equal | Symbol::NotEqual)
        ) && (nan(self.tree, left) || nan(self.tree, right))
        {
            self.finding(
                node,
                RuleIdentity::CompareNan,
                "comparison with NaN has a fixed result",
            );
        }
    }

    fn compare_table(&mut self, node: NodeIndex) {
        let Some((left, operator, right)) = self.operands(node) else {
            return;
        };

        if matches!(
            operator,
            TokenKind::Symbol(Symbol::Equal | Symbol::NotEqual)
        ) && (matches!(
            self.tree.node(unwrap(self.tree, left)).kind,
            NodeKind::Table { .. }
        ) || matches!(
            self.tree.node(unwrap(self.tree, right)).kind,
            NodeKind::Table { .. }
        )) {
            self.finding(
                node,
                RuleIdentity::ConstantTableComparison,
                "fresh tables are compared by identity",
            );
        }
    }

    fn conditional_expression(&mut self, node: NodeIndex) {
        let Some((left, operator, _)) = self.operands(node) else {
            return;
        };

        if operator == TokenKind::Keyword(Keyword::Or)
            && let NodeKind::Binary {
                operator, right, ..
            } = &self.tree.node(unwrap(self.tree, left)).kind
            && self.tree.token(*operator).kind == TokenKind::Keyword(Keyword::And)
            && truth(self.tree, *right) != Some(false)
        {
            self.finding(
                node,
                RuleIdentity::AndOrConditional,
                "use an if expression instead of an and/or conditional",
            );
        }
    }

    pub(super) fn table(&mut self, node: NodeIndex) {
        let tree = self.tree;

        let NodeKind::Table { fields, .. } = &tree.node(node).kind else {
            return;
        };

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

        if keyed && positional {
            self.finding(
                node,
                RuleIdentity::MixedTable,
                "table mixes positional and keyed entries",
            );
        }
    }

    fn operands(&self, node: NodeIndex) -> Option<(NodeIndex, TokenKind, NodeIndex)> {
        let NodeKind::Binary {
            left,
            operator,
            right,
        } = &self.tree.node(node).kind
        else {
            return None;
        };

        Some((*left, self.tree.token(*operator).kind, *right))
    }
}

pub(crate) fn evaluate(
    input: &RuleInput<'_, '_>,
    settings: &RuleSettings,
    options: &Options,
) -> RuleResult {
    let mut context = Context::new(input, settings);

    let check: fn(&mut Context<'_, '_>, NodeIndex, &[NodeIndex]) = match context.rule {
        RuleIdentity::DivideByZero => |context, node, _| {
            context.require_number(node);
            context.divide(node);
        },

        RuleIdentity::CompareNan => |context, node, _| {
            context.require_number(node);
            context.compare_not_a_number(node);
        },

        RuleIdentity::ConstantTableComparison => |context, node, _| context.compare_table(node),
        RuleIdentity::AndOrConditional => |context, node, _| context.conditional_expression(node),
        RuleIdentity::MixedTable => |context, node, _| context.table(node),
        RuleIdentity::ManualTableClone => |context, node, _| context.cloning(node),
        RuleIdentity::BadStringEscape => string_escape,
        _ => unreachable!("expression producer received another rule"),
    };

    context.walk(options, check);

    context.finish(options)
}

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

    match literal::number(tree, node)? {
        literal::NumberValue::Float(bits) => Some(f64::from_bits(bits)),
        literal::NumberValue::Integer(_) => None,
    }
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

        NodeKind::Number { .. }
        | NodeKind::String { .. }
        | NodeKind::Table { .. }
        | NodeKind::Function { .. } => Some(true),

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

fn string_escape(context: &mut Context<'_, '_>, node: NodeIndex, _: &[NodeIndex]) {
    match &context.tree.node(node).kind {
        NodeKind::String { token }
            if matches!(
                context.tree.token(*token).kind,
                TokenKind::QuotedString
                    | TokenKind::RawString
                    | TokenKind::InterpolatedStringSimple
            ) && literal::bytes(context.tree, node).is_none() =>
        {
            context.finding(
                node,
                RuleIdentity::BadStringEscape,
                "string contains an invalid escape",
            );
        }

        _ => {}
    }
}
