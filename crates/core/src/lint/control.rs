use std::{cmp::Ordering, collections::BTreeSet};

use vermis::{
    token::{Keyword, Symbol, TokenKind},
    tree::{NodeIndex, NodeKind, Tree},
};

use crate::{
    lint::{
        Edit,
        calls::arguments_list,
        context::Context,
        expression::{self, assignment},
        layout,
        syntax::{body, is_same, is_truthy, single},
    },
    syntax::{
        bindings::text,
        expression::{number, unwrap},
    },
};

#[derive(Copy, Clone, Eq, Ord, PartialEq, PartialOrd)]
enum Exit {
    Unknown,
    Continue,
    Break,
    Return,
    Error,
}

pub(super) fn check(context: &mut Context<'_, '_>, node: NodeIndex) {
    let tree = context.tree;

    match &tree.node(node).kind {
        NodeKind::Branch { condition, .. }
        | NodeKind::While { condition, .. }
        | NodeKind::Repeat { condition, .. }
        | NodeKind::Conditional { condition, .. } => self::condition(context, *condition),

        _ => {}
    }

    match &tree.node(node).kind {
        NodeKind::NumericFor {
            step: Some(step), ..
        } if context.is_active(&["zero_step_loop"]) && number(tree, *step) == Some(0.0) => {
            context.emit(*step, "zero_step_loop", "Numeric loop step is zero.");
        }

        NodeKind::While { body, .. }
        | NodeKind::Repeat { body, .. }
        | NodeKind::NumericFor { body, .. }
        | NodeKind::GenericFor { body, .. }
            if context.is_active(&["empty_loop"]) && is_empty(tree, *body) =>
        {
            context.emit(node, "empty_loop", "Loop body is empty.");
        }

        NodeKind::If {
            branches,
            otherwise,
            ..
        } if context.is_active(&[
            "empty_if",
            "if_same_then_else",
            "else_after_return",
            "negated_condition",
            "collapsible_if",
        ]) =>
        {
            for branch in tree.list(branches) {
                if context.is_active(&["empty_if"])
                    && let NodeKind::Branch { body, .. } = tree.node(branch.node).kind
                    && is_empty(tree, body)
                {
                    context.emit(branch.node, "empty_if", "Conditional branch is empty.");
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

                if context.is_active(&["empty_if"]) && is_empty(tree, *otherwise) {
                    context.emit(*otherwise, "empty_if", "Conditional branch is empty.");
                }

                if context.is_active(&["if_same_then_else"])
                    && let Some(first) = bodies.first()
                    && bodies.iter().all(|body| is_same(tree, *first, *body))
                    && is_same(tree, *first, body(tree, *otherwise))
                {
                    context.emit(
                        node,
                        "if_same_then_else",
                        "Conditional branches have identical bodies.",
                    );
                }

                if context.is_active(&["else_after_return"])
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
                        "An `else` branch follows branches that return.",
                    );
                }

                if context.is_active(&["negated_condition"])
                    && let [branch] = tree.list(branches)
                    && let NodeKind::Branch { condition, .. } = tree.node(branch.node).kind
                    && matches!(&tree.node(unwrap(tree, condition)).kind, NodeKind::Unary { operator, .. }
                        if tree.token(*operator).kind == TokenKind::Keyword(Keyword::Not))
                {
                    context.emit(
                        condition,
                        "negated_condition",
                        "Condition is negated; consider swapping the branches and removing `not`.",
                    );
                }
            } else if let [branch] = tree.list(branches)
                && context.is_active(&["collapsible_if"])
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
                    "Nested single-branch conditionals can be combined.",
                );
            }
        }

        _ => {}
    }

    match &tree.node(node).kind {
        NodeKind::Block { statements }
            if context.is_active(&[
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
            let mut is_terminated = false;

            for (index, statement) in statements.iter().enumerate() {
                if context.is_active(&["DuplicateFunction"])
                    && let NodeKind::Function {
                        name: Some(name), ..
                    } = tree.node(statement.node).kind
                    && !functions.insert(text(tree, name).replace(':', "."))
                {
                    context.emit(
                        name,
                        "DuplicateFunction",
                        "Function has already been defined in this block.",
                    );
                }

                if context.is_active(&["MultiLineStatement"]) {
                    layout::check(context, statement.node);
                }

                if index == 0 {
                    continue;
                }

                let previous = statements[index - 1];

                if context.is_active(&["almost_swapped"])
                    && let (Some((first_target, first_value)), Some((second_target, second_value))) = (
                        assignment(tree, previous.node),
                        assignment(tree, statement.node),
                    )
                    && context.bindings().is_same(tree, first_target, second_value)
                    && context.bindings().is_same(tree, first_value, second_target)
                    && !context.bindings().is_same(tree, first_target, first_value)
                {
                    context.emit(
                        statement.node,
                        "almost_swapped",
                        "Sequential assignments overwrite a value instead of swapping the values.",
                    );
                }

                let line = context
                    .is_active(&["SameLineStatement"])
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
                    context.emit(
                        statement.node,
                        "SameLineStatement",
                        "A new statement starts on the same line; add a semicolon to separate the statements.",
                    );

                    last_line = Some(line);
                }

                let exit = if context.is_active(&["UnreachableCode"]) && !is_terminated {
                    exit(context, previous.node)
                } else {
                    Exit::Unknown
                };

                if !is_terminated && exit != Exit::Unknown {
                    if !(exit == Exit::Error
                        && matches!(tree.node(statement.node).kind, NodeKind::Return { .. })
                        && index + 1 == statements.len())
                    {
                        context.emit(
                            statement.node,
                            "UnreachableCode",
                            "This statement is unreachable because the previous statement terminates control flow.",
                        );
                    }

                    is_terminated = true;
                }
            }
        }

        NodeKind::If {
            branches,
            otherwise,
            ..
        } if context.is_active(&["DuplicateCondition", "if_expression_assignment"]) => {
            if context.is_active(&["DuplicateCondition"]) {
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

            if context.is_active(&["if_expression_assignment"])
                && let ([branch], Some(otherwise)) = (tree.list(branches), otherwise)
                && let NodeKind::Branch { body, .. } = tree.node(branch.node).kind
                && let (Some(first), Some(second)) = (single(tree, body), single(tree, *otherwise))
                && let (Some((first, _)), Some((second, _))) =
                    (assignment(tree, first), assignment(tree, second))
                && context.bindings().references[first.get()].is_some()
                && context.bindings().is_same(tree, first, second)
            {
                context.emit(
                    node,
                    "if_expression_assignment",
                    "Both branches assign to the same local binding; consider an `if` expression.",
                );
            }
        }

        NodeKind::Conditional { .. } if context.is_active(&["DuplicateCondition"]) => {
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
        } if context.is_active(&["ForRange"]) => {
            let start_number = number(tree, *start);
            let end_number = number(tree, *end);

            let is_length = |node| {
                matches!(
                    tree.node(node).kind,
                    NodeKind::Unary { operator, .. }
                        if tree.token(operator).kind == TokenKind::Symbol(Symbol::Length)
                )
            };

            let message = if is_length(*start)
                && (end_number == Some(0.0) || end_number == Some(1.0))
            {
                Some(
                    "This loop starts at a length and uses the default positive step; check the direction.",
                )
            } else if let Some((start, end)) = start_number.zip(end_number) {
                if start > end {
                    Some("Loop start exceeds its end, but the default step is positive.")
                } else if (start + (end - start).floor()).partial_cmp(&end) != Some(Ordering::Equal)
                {
                    Some("The default loop step does not reach the end value exactly.")
                } else {
                    None
                }
            } else if start_number == Some(0.0) && is_length(*end) {
                Some(
                    "This loop starts at `0` and ends at a length; check whether indexing should start at `1`.",
                )
            } else {
                None
            };

            if let Some(message) = message {
                context.emit(node, "ForRange", message);
            }
        }

        NodeKind::Function {
            body: Some(body), ..
        } if context.is_active(&["ImplicitReturn"])
            && exit(context, *body) == Exit::Unknown
            && has_value_return(context, *body)
            && can_fall_through(context, *body) =>
        {
            context.emit(
                node,
                "ImplicitReturn",
                "Function can implicitly return no values despite an explicit value return.",
            );
        }

        NodeKind::GenericFor {
            bindings,
            values,
            body,
            ..
        } if context.is_active(&["manual_table_clone"]) => {
            if let ([key, value], [iterator], Some(statement)) =
                (tree.list(bindings), tree.list(values), single(tree, *body))
                && let NodeKind::Call { callee, arguments } =
                    tree.node(unwrap(tree, iterator.node)).kind
                && context.bindings().is_global(tree, callee, "pairs")
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
                && !context.bindings().locals[destination].is_assigned
                && let Some(initializer) = context.bindings().locals[destination].value
                && matches!(
                    &tree.node(unwrap(tree, initializer)).kind,
                    NodeKind::Table { fields, .. } if tree.list(fields).is_empty()
                )
            {
                context.emit(
                    node,
                    "manual_table_clone",
                    "This loop copies table entries; consider `table.clone`.",
                );
            }
        }

        _ => {}
    }
}

fn condition(context: &mut Context<'_, '_>, condition: NodeIndex) {
    let tree = context.tree;
    let unwrapped = unwrap(tree, condition);

    if context.is_active(&["constant_condition"]) && is_truthy(tree, unwrapped).is_some() {
        context.emit(
            condition,
            "constant_condition",
            "Condition has a fixed truth value.",
        );
    }

    if context.is_active(&["length_as_condition"])
        && matches!(&tree.node(unwrapped).kind, NodeKind::Unary { operator, .. }
            if tree.token(*operator).kind == TokenKind::Symbol(Symbol::Length))
    {
        context.emit(
            condition,
            "length_as_condition",
            "Zero is truthy in Luau; compare the length explicitly.",
        );
    }

    if context.is_active(&["parenthesized_conditions"])
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

        context.emit_with_edits(
            condition,
            "parenthesized_conditions",
            "Condition has unnecessary parentheses.",
            edits,
        );
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
                && (context.bindings().is_global(tree, callee, "error")
                    || (context.bindings().is_global(tree, callee, "assert")
                        && arguments_list(tree, arguments)
                            .first()
                            .is_some_and(|argument| is_truthy(tree, *argument) == Some(false))))
            {
                return Exit::Error;
            }

            Exit::Unknown
        }

        _ => Exit::Unknown,
    }
}

fn has_value_return(context: &Context<'_, '_>, node: NodeIndex) -> bool {
    match &context.tree.node(node).kind {
        NodeKind::Function { .. } => false,
        NodeKind::Return { values, .. } => !context.tree.list(values).is_empty(),

        _ => context
            .tree
            .children(node)
            .iter()
            .any(|child| has_value_return(context, *child)),
    }
}

fn can_fall_through(context: &Context<'_, '_>, node: NodeIndex) -> bool {
    match &context.tree.node(node).kind {
        NodeKind::Block { statements } => context
            .tree
            .list(statements)
            .iter()
            .all(|entry| can_fall_through(context, entry.node)),

        NodeKind::While {
            condition, body, ..
        } if is_truthy(context.tree, *condition) == Some(true) => has_break(context.tree, *body),

        NodeKind::If {
            branches,
            otherwise: Some(otherwise),
            ..
        } => {
            context
                .tree
                .list(branches)
                .iter()
                .any(|entry| can_fall_through(context, entry.node))
                || can_fall_through(context, *otherwise)
        }

        NodeKind::Do { body, .. } | NodeKind::Else { body, .. } | NodeKind::Branch { body, .. } => {
            can_fall_through(context, *body)
        }

        _ => exit(context, node) == Exit::Unknown,
    }
}

fn has_break(tree: &Tree<'_>, node: NodeIndex) -> bool {
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
            .any(|child| has_break(tree, *child)),
    }
}

fn is_empty(tree: &Tree<'_>, node: NodeIndex) -> bool {
    matches!(&tree.node(body(tree, node)).kind, NodeKind::Block { statements } if tree.list(statements).is_empty())
}
