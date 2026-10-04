use vermis::{
    token::{Keyword, TokenKind},
    tree::{NodeIndex, NodeKind, Tree},
};

use super::{Context, Finding};

pub(super) fn check(
    node: NodeIndex,
    ancestors: &[NodeIndex],
    context: &Context<'_>,
    findings: &mut Vec<Finding>,
) {
    match &context.tree.node(node).kind {
        NodeKind::Binary { .. } => and_or_conditional(node, context, findings),
        NodeKind::If { .. } => collapsible_if(node, context, findings),

        NodeKind::Call { .. } | NodeKind::MethodCall { .. } => {
            deprecated(node, context, findings);
            restricted_module_path(node, ancestors, context, findings);
        }

        NodeKind::Branch { .. } | NodeKind::While { .. } | NodeKind::Repeat { .. } => {
            parenthese_conditions(node, context, findings);
        }

        NodeKind::Else { .. } => else_after_return(node, ancestors, context, findings),

        NodeKind::Conditional { .. } => {
            if_expression_assignment(node, ancestors, context, findings);
        }

        NodeKind::Unary { .. } => negated_condition(node, ancestors, context, findings),

        NodeKind::Local { .. } => {
            non_const_require(node, ancestors, context, findings);
            prefer_const(node, context, findings);
        }

        NodeKind::Name { .. } => restricted_global(node, ancestors, context, findings),
        _ => {}
    }
}

fn and_or_conditional(node: NodeIndex, context: &Context<'_>, findings: &mut Vec<Finding>) {
    if !context.enabled("and_or_conditional") {
        return;
    }

    let tree = context.tree;

    let NodeKind::Binary { left, operator, .. } = &tree.node(node).kind else {
        return;
    };

    if tree.token(*operator).kind != TokenKind::Keyword(Keyword::Or) {
        return;
    }

    let NodeKind::Binary {
        right, operator, ..
    } = &tree.node(*left).kind
    else {
        return;
    };

    if tree.token(*operator).kind == TokenKind::Keyword(Keyword::And)
        && !matches!(tree.node(*right).kind, NodeKind::Nil { .. })
        && !(matches!(tree.node(*right).kind, NodeKind::Boolean { .. })
            && tree.text(*right) == b"false")
    {
        context.emit(
            findings,
            "and_or_conditional",
            tree.node(node).span,
            "and/or conditional used as a value",
        );
    }
}

fn collapsible_if(node: NodeIndex, context: &Context<'_>, findings: &mut Vec<Finding>) {
    if !context.enabled("collapsible_if") {
        return;
    }

    let tree = context.tree;

    let NodeKind::If {
        branches,
        otherwise: None,
        ..
    } = &tree.node(node).kind
    else {
        return;
    };

    let branches = tree.list(branches);

    if branches.len() != 1 {
        return;
    }

    let NodeKind::Branch { body, .. } = &tree.node(branches[0].node).kind else {
        return;
    };

    let NodeKind::Block { statements } = &tree.node(*body).kind else {
        return;
    };

    let statements = tree.list(statements);

    if statements.len() != 1 {
        return;
    }

    let NodeKind::If {
        branches,
        otherwise: None,
        ..
    } = &tree.node(statements[0].node).kind
    else {
        return;
    };

    if tree.list(branches).len() == 1 {
        context.emit(
            findings,
            "collapsible_if",
            tree.node(node).span,
            "nested single-branch if statements can be combined",
        );
    }
}

fn else_after_return(
    node: NodeIndex,
    ancestors: &[NodeIndex],
    context: &Context<'_>,
    findings: &mut Vec<Finding>,
) {
    if !context.enabled("else_after_return") {
        return;
    }

    let tree = context.tree;

    let Some(parent) = ancestors.last() else {
        return;
    };

    let NodeKind::If { branches, .. } = &tree.node(*parent).kind else {
        return;
    };

    let exits = tree.list(branches).iter().all(|branch| {
        let NodeKind::Branch { body, .. } = &tree.node(branch.node).kind else {
            return false;
        };

        matches!(&tree.node(*body).kind, NodeKind::Block { statements }
        if tree.list(statements).last().is_some_and(|statement| matches!(
            tree.node(statement.node).kind,
            NodeKind::Return { .. } | NodeKind::Break { .. } | NodeKind::Continue { .. }
        )))
    });

    if exits {
        context.emit(
            findings,
            "else_after_return",
            tree.node(node).span,
            "else branch follows branches that exit",
        );
    }
}

fn if_expression_assignment(
    node: NodeIndex,
    ancestors: &[NodeIndex],
    context: &Context<'_>,
    findings: &mut Vec<Finding>,
) {
    if !context.enabled("if_expression_assignment") {
        return;
    }

    if !ancestors.last().is_some_and(|parent| {
        matches!(
            context.tree.node(*parent).kind,
            NodeKind::Local { .. }
                | NodeKind::Constant { .. }
                | NodeKind::Assignment { .. }
                | NodeKind::CompoundAssignment { .. }
        )
    }) {
        return;
    }

    context.emit(
        findings,
        "if_expression_assignment",
        context.tree.node(node).span,
        "if-expression is used as a value",
    );
}

fn negated_condition(
    node: NodeIndex,
    ancestors: &[NodeIndex],
    context: &Context<'_>,
    findings: &mut Vec<Finding>,
) {
    if !context.enabled("negated_condition") {
        return;
    }

    let tree = context.tree;

    let Some(branch) = ancestors.last() else {
        return;
    };

    let NodeKind::Branch { condition, .. } = &tree.node(*branch).kind else {
        return;
    };

    if *condition != node {
        return;
    }

    let Some(if_node) = ancestors
        .iter()
        .rev()
        .find(|ancestor| matches!(tree.node(**ancestor).kind, NodeKind::If { .. }))
    else {
        return;
    };

    let NodeKind::If {
        branches,
        otherwise: Some(_),
        ..
    } = &tree.node(*if_node).kind
    else {
        return;
    };

    if tree.list(branches).len() != 1 {
        return;
    }

    if matches!(&tree.node(node).kind, NodeKind::Unary { operator, .. }
        if tree.token(*operator).kind == TokenKind::Keyword(Keyword::Not))
    {
        context.emit(
            findings,
            "negated_condition",
            tree.node(node).span,
            "negated if condition could be expressed by swapping branches",
        );
    }
}

fn non_const_require(
    node: NodeIndex,
    ancestors: &[NodeIndex],
    context: &Context<'_>,
    findings: &mut Vec<Finding>,
) {
    if !context.enabled("non_const_require") {
        return;
    }

    let tree = context.tree;

    let NodeKind::Local {
        bindings, values, ..
    } = &tree.node(node).kind
    else {
        return;
    };

    for (binding, value) in tree.list(bindings).iter().zip(tree.list(values)) {
        let NodeKind::Binding { name, .. } = &tree.node(binding.node).kind else {
            continue;
        };

        if is_require(tree, value.node)
            && !super::suspicious::has_local(
                tree,
                b"require",
                tree.node(value.node).span.start,
                ancestors,
            )
            && !context.writes.assigned(tree.node(*name).span.start)
        {
            context.emit(
                findings,
                "non_const_require",
                tree.node(binding.node).span,
                "unchanged require binding can be declared const",
            );
        }
    }
}

fn prefer_const(node: NodeIndex, context: &Context<'_>, findings: &mut Vec<Finding>) {
    if !context.enabled("prefer_const") {
        return;
    }

    let tree = context.tree;

    let NodeKind::Local {
        bindings, values, ..
    } = &tree.node(node).kind
    else {
        return;
    };

    for (binding, value) in tree.list(bindings).iter().zip(tree.list(values)) {
        let NodeKind::Binding { name, .. } = &tree.node(binding.node).kind else {
            continue;
        };

        if context.writes.assigned(tree.node(*name).span.start) {
            continue;
        }

        if context.config.prefer_const.mutated_tables_stay_local()
            && matches!(tree.node(value.node).kind, NodeKind::Table { .. })
            && context.writes.mutated(tree.node(*name).span.start)
        {
            continue;
        }

        context.emit(
            findings,
            "prefer_const",
            tree.node(binding.node).span,
            "unchanged valued local can be declared const",
        );
    }
}

fn is_require(tree: &Tree<'_>, node: NodeIndex) -> bool {
    matches!(&tree.node(node).kind, NodeKind::Call { callee, .. }
        if matches!(tree.node(*callee).kind, NodeKind::Name { .. }) && tree.text(*callee) == b"require")
}

fn deprecated(node: NodeIndex, context: &Context<'_>, findings: &mut Vec<Finding>) {
    let tree = context.tree;

    if !context.enabled("deprecated") {
        return;
    }

    let options = &context.config.deprecated;

    if options.paths.is_empty() {
        return;
    }

    if let Some(path) = call_path(tree, node)
        && let Some(replacement) = options.paths.get(&path)
    {
        context.emit(
            findings,
            "deprecated",
            tree.node(node).span,
            format!("{path} is deprecated; use {replacement}"),
        );

        return;
    }

    if options.ambiguous_methods()
        && let Some(method) = method_name(tree, node)
    {
        for (path, replacement) in &options.paths {
            if path.rsplit('.').next() == Some(method.as_str()) {
                context.emit(
                    findings,
                    "deprecated",
                    tree.node(node).span,
                    format!("method {method} may be deprecated; use {replacement}"),
                );

                break;
            }
        }
    }
}

fn method_name(tree: &Tree<'_>, node: NodeIndex) -> Option<String> {
    match &tree.node(node).kind {
        NodeKind::MethodCall { method, .. } => Some(text(tree.text(*method))),
        _ => None,
    }
}

fn call_path(tree: &Tree<'_>, node: NodeIndex) -> Option<String> {
    match &tree.node(node).kind {
        NodeKind::Call { callee, .. } => expression_path(tree, *callee),

        NodeKind::MethodCall {
            receiver, method, ..
        } => Some(format!(
            "{}.{}",
            expression_path(tree, *receiver)?,
            text(tree.text(*method))
        )),

        _ => None,
    }
}

fn expression_path(tree: &Tree<'_>, node: NodeIndex) -> Option<String> {
    match &tree.node(node).kind {
        NodeKind::Name { .. } => Some(text(tree.text(node))),

        NodeKind::Field { receiver, name, .. } => Some(format!(
            "{}.{}",
            expression_path(tree, *receiver)?,
            text(tree.text(*name))
        )),

        NodeKind::Group { expression, .. } => expression_path(tree, *expression),
        _ => None,
    }
}

fn restricted_global(
    node: NodeIndex,
    ancestors: &[NodeIndex],
    context: &Context<'_>,
    findings: &mut Vec<Finding>,
) {
    let restricted = &context.config.restricted_globals.names;

    if !context.enabled("restricted_globals") || restricted.is_empty() {
        return;
    }

    let tree = context.tree;

    if ancestors.iter().any(|parent| {
        matches!(
            tree.node(*parent).kind,
            NodeKind::Binding { .. } | NodeKind::FunctionName { .. } | NodeKind::TypeName { .. }
        )
    }) {
        return;
    }

    if ancestors
        .last()
        .is_some_and(|parent| match &tree.node(*parent).kind {
            NodeKind::Field { name, .. } => *name == node,
            NodeKind::MethodCall { method, .. } => *method == node,
            _ => false,
        })
    {
        return;
    }

    let Ok(name) = std::str::from_utf8(tree.text(node)) else {
        return;
    };

    let Some(reason) = restricted.get(name) else {
        return;
    };

    if super::suspicious::has_local(tree, tree.text(node), tree.node(node).span.start, ancestors) {
        return;
    }

    context.emit(
        findings,
        "restricted_globals",
        tree.node(node).span,
        format!("global {name} is restricted: {reason}"),
    );
}

fn restricted_module_path(
    node: NodeIndex,
    ancestors: &[NodeIndex],
    context: &Context<'_>,
    findings: &mut Vec<Finding>,
) {
    let paths = &context.config.restricted_module_paths.paths;
    let tree = context.tree;

    if !context.enabled("restricted_module_paths")
        || paths.is_empty()
        || !is_require(tree, node)
        || super::suspicious::has_local(tree, b"require", tree.node(node).span.start, ancestors)
    {
        return;
    }

    let NodeKind::Call { arguments, .. } = &tree.node(node).kind else {
        return;
    };

    let NodeKind::Arguments { values, .. } = &tree.node(*arguments).kind else {
        return;
    };

    let Some(argument) = tree.list(values).first() else {
        return;
    };

    if !matches!(tree.node(argument.node).kind, NodeKind::String { .. }) {
        return;
    }

    let Ok(path) = crate::string_value(tree.text(argument.node)) else {
        return;
    };

    if let Some(reason) = paths.get(&path) {
        context.emit(
            findings,
            "restricted_module_paths",
            tree.node(argument.node).span,
            format!("module path {path} is restricted: {reason}"),
        );
    }
}

fn parenthese_conditions(node: NodeIndex, context: &Context<'_>, findings: &mut Vec<Finding>) {
    if !context.enabled("parenthese_conditions") {
        return;
    }

    let tree = context.tree;

    let (NodeKind::Branch { condition, .. }
    | NodeKind::While { condition, .. }
    | NodeKind::Repeat { condition, .. }) = &tree.node(node).kind
    else {
        return;
    };

    if matches!(tree.node(*condition).kind, NodeKind::Group { .. }) {
        context.emit(
            findings,
            "parenthese_conditions",
            tree.node(*condition).span,
            "unnecessary parentheses around condition",
        );
    }
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}
