use std::{cmp::Ordering, collections::BTreeSet};

use vermis::{
    token::{Symbol, TokenKind},
    tree::{NodeIndex, NodeKind, Tree},
};

use crate::{
    lint::{
        calls::arguments_list,
        context::Context,
        expression::{self, assignment},
        layout,
        syntax::{single, truth},
    },
    syntax::{bindings::text, expression::number},
};

#[derive(Copy, Clone, Eq, Ord, PartialEq, PartialOrd)]
enum Exit {
    Unknown,
    Continue,
    Break,
    Return,
    Error,
}

pub(super) fn analyze(context: &mut Context<'_, '_>, node: NodeIndex) {
    let tree = context.tree;

    match &tree.node(node).kind {
        NodeKind::Block { statements }
            if context.active(&[
                "DuplicateFunction",
                "MultiLineStatement",
                "SameLineStatement",
                "almost_swapped",
                "UnreachableCode",
            ]) =>
        {
            let statements = tree.list(statements);
            let mut functions = BTreeSet::new();
            let mut last_line = None;
            let mut terminated = false;

            for (index, statement) in statements.iter().enumerate() {
                if context.active(&["DuplicateFunction"])
                    && let NodeKind::Function {
                        name: Some(name), ..
                    } = tree.node(statement.node).kind
                    && !functions.insert(text(tree, name).replace(':', "."))
                {
                    context.warn(
                        name,
                        "DuplicateFunction",
                        "function has already been defined in this block",
                    );
                }

                if context.active(&["MultiLineStatement"]) {
                    layout::check(context, statement.node);
                }

                if index == 0 {
                    continue;
                }

                let previous = statements[index - 1];

                if context.active(&["almost_swapped"])
                    && let (Some((first_target, first_value)), Some((second_target, second_value))) = (
                        assignment(tree, previous.node),
                        assignment(tree, statement.node),
                    )
                    && context.bindings().same(tree, first_target, second_value)
                    && context.bindings().same(tree, first_value, second_target)
                    && !context.bindings().same(tree, first_target, first_value)
                {
                    context.warn(
                        statement.node,
                        "almost_swapped",
                        "sequential assignments overwrite a value before swapping it",
                    );
                }

                let line = context
                    .active(&["SameLineStatement"])
                    .then(|| layout::line(context, tree.node(statement.node).span.start));

                if let Some(line) = line
                    && line == layout::line(context, tree.node(previous.node).span.end)
                    && last_line != Some(line)
                    && previous.separator.is_none()
                    && !(matches!(
                        tree.node(previous.node).kind,
                        NodeKind::Local { .. } | NodeKind::Constant { .. }
                    ) && matches!(tree.node(statement.node).kind, NodeKind::Do { .. }))
                {
                    context.warn(
                        statement.node,
                        "SameLineStatement",
                        "a new statement is on the same line; add a semicolon to separate statements",
                    );

                    last_line = Some(line);
                }

                let exit = if context.active(&["UnreachableCode"]) && !terminated {
                    exit(context, previous.node)
                } else {
                    Exit::Unknown
                };

                if !terminated && exit != Exit::Unknown {
                    if !(exit == Exit::Error
                        && matches!(tree.node(statement.node).kind, NodeKind::Return { .. })
                        && index + 1 == statements.len())
                    {
                        context.warn(
                            statement.node,
                            "UnreachableCode",
                            "previous statement always terminates control flow",
                        );
                    }

                    terminated = true;
                }
            }
        }

        NodeKind::If {
            branches,
            otherwise,
            ..
        } if context.active(&["DuplicateCondition", "if_expression_assignment"]) => {
            if context.active(&["DuplicateCondition"]) {
                let conditions: Vec<_> = tree
                    .list(branches)
                    .iter()
                    .filter_map(|entry| {
                        if let NodeKind::Branch { condition, .. } = tree.node(entry.node).kind {
                            Some(condition)
                        } else {
                            None
                        }
                    })
                    .collect();

                expression::duplicates(context, &conditions);
            }

            if context.active(&["if_expression_assignment"])
                && let ([branch], Some(otherwise)) = (tree.list(branches), otherwise)
                && let NodeKind::Branch { body, .. } = tree.node(branch.node).kind
                && let (Some(first), Some(second)) = (single(tree, body), single(tree, *otherwise))
                && let (Some((first, _)), Some((second, _))) =
                    (assignment(tree, first), assignment(tree, second))
                && context.bindings().references[first.get()].is_some()
                && context.bindings().same(tree, first, second)
            {
                context.warn(
                    node,
                    "if_expression_assignment",
                    "matching branch assignments can use an if expression",
                );
            }
        }

        NodeKind::Conditional { .. } if context.active(&["DuplicateCondition"]) => {
            if !context.bindings().parents[node.get()].is_some_and(|parent| {
                matches!(
                    tree.node(parent).kind,
                    NodeKind::Conditional { falsy, .. } if falsy == node
                )
            }) {
                let mut conditions = Vec::new();
                let mut branch = node;

                while let NodeKind::Conditional {
                    condition, falsy, ..
                } = tree.node(branch).kind
                {
                    conditions.push(condition);
                    branch = falsy;
                }

                expression::duplicates(context, &conditions);
            }
        }

        NodeKind::NumericFor {
            start,
            end,
            step: None,
            ..
        } if context.active(&["ForRange"]) => {
            let start_number = number(tree, *start);
            let end_number = number(tree, *end);

            let length = |node| {
                matches!(
                    tree.node(node).kind,
                    NodeKind::Unary { operator, .. }
                        if tree.token(operator).kind == TokenKind::Symbol(Symbol::Length)
                )
            };

            if (length(*start) && (end_number == Some(0.0) || end_number == Some(1.0)))
                || start_number.zip(end_number).is_some_and(|(start, end)| {
                    start > end
                        || (start + (end - start).floor()).partial_cmp(&end)
                            != Some(Ordering::Equal)
                })
                || (start_number == Some(0.0) && length(*end))
            {
                context.warn(
                    node,
                    "ForRange",
                    "numeric loop bounds require an explicit step or a one-based starting index",
                );
            }
        }

        NodeKind::Function {
            body: Some(body), ..
        } if context.active(&["ImplicitReturn"])
            && exit(context, *body) == Exit::Unknown
            && value_return(context, *body)
            && falls_through(context, *body) =>
        {
            context.warn(
                node,
                "ImplicitReturn",
                "function can implicitly return no values despite an explicit value return",
            );
        }

        _ => {}
    }
}

fn exit(context: &Context<'_, '_>, node: NodeIndex) -> Exit {
    let tree = context.tree;

    match &tree.node(node).kind {
        NodeKind::Block { statements } => tree
            .list(statements)
            .iter()
            .map(|entry| exit(context, entry.node))
            .find(|exit| *exit != Exit::Unknown)
            .unwrap_or(Exit::Unknown),

        NodeKind::Do { body, .. } | NodeKind::Else { body, .. } | NodeKind::Branch { body, .. } => {
            exit(context, *body)
        }

        NodeKind::If {
            branches,
            otherwise: Some(otherwise),
            ..
        } => tree
            .list(branches)
            .iter()
            .map(|entry| exit(context, entry.node))
            .chain([exit(context, *otherwise)])
            .min()
            .unwrap_or(Exit::Unknown),

        NodeKind::Return { .. } => Exit::Return,
        NodeKind::Break { .. } => Exit::Break,
        NodeKind::Continue { .. } => Exit::Continue,

        NodeKind::CallStatement { call } => {
            if let NodeKind::Call { callee, arguments } = tree.node(*call).kind
                && (context.bindings().global(tree, callee, "error")
                    || (context.bindings().global(tree, callee, "assert")
                        && arguments_list(tree, arguments)
                            .first()
                            .is_some_and(|argument| truth(tree, *argument) == Some(false))))
            {
                return Exit::Error;
            }

            Exit::Unknown
        }

        _ => Exit::Unknown,
    }
}

fn value_return(context: &Context<'_, '_>, node: NodeIndex) -> bool {
    match &context.tree.node(node).kind {
        NodeKind::Function { .. } => false,
        NodeKind::Return { values, .. } => !context.tree.list(values).is_empty(),

        _ => context
            .tree
            .children(node)
            .iter()
            .any(|child| value_return(context, *child)),
    }
}

fn falls_through(context: &Context<'_, '_>, node: NodeIndex) -> bool {
    match &context.tree.node(node).kind {
        NodeKind::Block { statements } => context
            .tree
            .list(statements)
            .iter()
            .all(|entry| falls_through(context, entry.node)),

        NodeKind::While {
            condition, body, ..
        } if truth(context.tree, *condition) == Some(true) => contains_break(context.tree, *body),

        NodeKind::If {
            branches,
            otherwise: Some(otherwise),
            ..
        } => {
            context
                .tree
                .list(branches)
                .iter()
                .any(|entry| falls_through(context, entry.node))
                || falls_through(context, *otherwise)
        }

        NodeKind::Do { body, .. } | NodeKind::Else { body, .. } | NodeKind::Branch { body, .. } => {
            falls_through(context, *body)
        }

        _ => exit(context, node) == Exit::Unknown,
    }
}

fn contains_break(tree: &Tree<'_>, node: NodeIndex) -> bool {
    match tree.node(node).kind {
        NodeKind::Break { .. } => true,

        NodeKind::Function { .. }
        | NodeKind::While { .. }
        | NodeKind::Repeat { .. }
        | NodeKind::NumericFor { .. }
        | NodeKind::GenericFor { .. } => false,

        _ => tree
            .children(node)
            .iter()
            .any(|child| contains_break(tree, *child)),
    }
}
