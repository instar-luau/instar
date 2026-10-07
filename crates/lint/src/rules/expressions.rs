use vermis::{
    token::{Keyword, Symbol, TokenKind},
    tree::{NodeIndex, NodeKind},
};

use super::{
    Context,
    syntax::{assignment, in_loop, nan, number, same, truth, unwrap},
};

use crate::Rule;

impl Context<'_, '_> {
    pub(super) fn binary(&mut self, node: NodeIndex, ancestors: &[NodeIndex]) {
        let tree = self.tree;

        let NodeKind::Binary {
            left,
            operator,
            right,
        } = &tree.node(node).kind
        else {
            return;
        };

        let operator = tree.token(*operator).kind;

        if matches!(
            operator,
            TokenKind::Symbol(Symbol::Divide | Symbol::FloorDivide | Symbol::Modulo)
        ) && number(tree, *right) == Some(0.0)
        {
            self.finding(*right, Rule::DivideByZero, "division or modulo by zero");
        }

        if matches!(
            operator,
            TokenKind::Symbol(Symbol::Equal | Symbol::NotEqual)
        ) {
            if nan(tree, *left) || nan(tree, *right) {
                self.finding(
                    node,
                    Rule::CompareNan,
                    "comparison with NaN has a fixed result",
                );
            }

            if matches!(tree.node(unwrap(tree, *left)).kind, NodeKind::Table { .. })
                || matches!(tree.node(unwrap(tree, *right)).kind, NodeKind::Table { .. })
            {
                self.finding(
                    node,
                    Rule::ConstantTableComparison,
                    "fresh tables are compared by identity",
                );
            }
        }

        if operator == TokenKind::Keyword(Keyword::Or)
            && let NodeKind::Binary {
                operator, right, ..
            } = &tree.node(unwrap(tree, *left)).kind
            && tree.token(*operator).kind == TokenKind::Keyword(Keyword::And)
            && truth(tree, *right) != Some(false)
        {
            self.finding(
                node,
                Rule::AndOrConditional,
                "use an if expression instead of an and/or conditional",
            );
        }

        if operator == TokenKind::Symbol(Symbol::Concatenate)
            && in_loop(tree, node, ancestors)
            && let Some(parent) = ancestors.last()
            && let Some((target, value)) = assignment(tree, *parent)
            && value == node
            && same(tree, target, *left)
        {
            self.finding(
                node,
                Rule::StringConcatInLoop,
                "repeated concatenation grows a string quadratically",
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
                Rule::MixedTable,
                "table mixes positional and keyed entries",
            );
        }
    }

    pub(super) fn condition(&mut self, condition: NodeIndex) {
        if matches!(self.tree.node(condition).kind, NodeKind::Group { .. }) {
            self.finding(
                condition,
                Rule::ParenthesizedConditions,
                "unnecessary parentheses around condition",
            );
        }

        let node = unwrap(self.tree, condition);

        if truth(self.tree, node).is_some() {
            self.finding(
                node,
                Rule::ConstantCondition,
                "condition has a fixed truth value",
            );
        }

        if let NodeKind::Unary { operator, .. } = &self.tree.node(node).kind
            && self.tree.token(*operator).kind == TokenKind::Symbol(Symbol::Length)
        {
            self.finding(
                node,
                Rule::LengthAsCondition,
                "zero is truthy in Luau; compare the length explicitly",
            );
        }
    }
}

pub(super) fn check(context: &mut Context<'_, '_>, node: NodeIndex, ancestors: &[NodeIndex]) {
    match &context.tree.node(node).kind {
        NodeKind::Binary { .. } => context.binary(node, ancestors),
        NodeKind::Table { .. } => context.table(node),

        NodeKind::Branch { condition, .. }
        | NodeKind::While { condition, .. }
        | NodeKind::Repeat { condition, .. }
        | NodeKind::Conditional { condition, .. } => context.condition(*condition),

        NodeKind::String { token }
            if matches!(
                context.tree.token(*token).kind,
                TokenKind::QuotedString
                    | TokenKind::RawString
                    | TokenKind::InterpolatedStringSimple
            ) && instar_syntax::literal::bytes(context.tree, node).is_none() =>
        {
            context.finding(
                node,
                Rule::BadStringEscape,
                "string contains an invalid escape",
            );
        }

        _ => {}
    }
}
