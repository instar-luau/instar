use vermis::{
    token::{Keyword, Span, Symbol, TokenKind},
    tree::{NodeIndex, NodeKind, Tree},
};

use super::{Context, Finding};

pub(super) fn check(
    node: NodeIndex,
    ancestors: &[NodeIndex],
    context: &Context<'_>,
    findings: &mut Vec<Finding>,
) {
    let tree = context.tree;

    match &tree.node(node).kind {
        NodeKind::Binary {
            operator, right, ..
        } if matches!(
            tree.token(*operator).kind,
            TokenKind::Symbol(Symbol::Divide | Symbol::FloorDivide | Symbol::Modulo)
        ) && context.enabled("divide_by_zero")
            && is_zero(tree, *right) =>
        {
            context.emit(
                findings,
                "divide_by_zero",
                tree.token(*operator).span,
                "division or modulo by zero",
            );
        }

        NodeKind::If {
            branches,
            otherwise,
            ..
        } if context.enabled("empty_if") => {
            for branch in tree.list(branches) {
                if let NodeKind::Branch { body, .. } = &tree.node(branch.node).kind {
                    emit_empty_body(
                        *body,
                        "empty_if",
                        "empty conditional branch",
                        context,
                        findings,
                    );
                }
            }

            if let Some(otherwise) = otherwise {
                emit_empty_body(
                    *otherwise,
                    "empty_if",
                    "empty conditional branch",
                    context,
                    findings,
                );
            }
        }

        NodeKind::While { body, .. }
        | NodeKind::Repeat { body, .. }
        | NodeKind::NumericFor { body, .. }
        | NodeKind::GenericFor { body, .. }
            if context.enabled("empty_loop") =>
        {
            emit_empty_body(*body, "empty_loop", "empty loop body", context, findings);
        }

        NodeKind::Name { .. } if tree.text(node) == b"_G" && context.enabled("global_usage") => {
            if !ancestors
                .last()
                .is_some_and(|parent| matches!(tree.node(*parent).kind, NodeKind::Binding { .. }))
                && !is_declared(b"_G", tree.node(node).span.start, ancestors, context)
            {
                context.emit(
                    findings,
                    "global_usage",
                    tree.node(node).span,
                    "access to shared _G",
                );
            }
        }

        NodeKind::CallStatement { call }
            if context.enabled("ignored_pcall_result") && is_protected_call(tree, *call) =>
        {
            context.emit(
                findings,
                "ignored_pcall_result",
                tree.node(*call).span,
                "pcall result is discarded",
            );
        }

        _ => {}
    }

    check_declarations(node, ancestors, context, findings);
    check_assignments(node, ancestors, context, findings);
    check_identical_branches(node, context, findings);
}

fn check_identical_branches(node: NodeIndex, context: &Context<'_>, findings: &mut Vec<Finding>) {
    let tree = context.tree;

    if let NodeKind::If {
        branches,
        otherwise: Some(otherwise),
        ..
    } = &tree.node(node).kind
        && context.enabled("if_same_then_else")
    {
        let mut bodies =
            tree.list(branches)
                .iter()
                .filter_map(|branch| match &tree.node(branch.node).kind {
                    NodeKind::Branch { body, .. } => Some(*body),
                    _ => None,
                });

        if let Some(first) = bodies.next()
            && bodies.all(|body| same_body(first, body, context))
            && same_body(first, *otherwise, context)
        {
            context.emit(
                findings,
                "if_same_then_else",
                tree.node(node).span,
                "if branches have identical bodies",
            );
        }
    }
}

fn check_declarations(
    node: NodeIndex,
    ancestors: &[NodeIndex],
    context: &Context<'_>,
    findings: &mut Vec<Finding>,
) {
    let tree = context.tree;

    match &tree.node(node).kind {
        NodeKind::Local {
            bindings, values, ..
        }
        | NodeKind::Constant {
            bindings, values, ..
        } if context.enabled("implicit_any_local") => {
            if tree.list(values).is_empty() {
                let later_assigned = tree.list(bindings).iter().any(|binding| {
                    let NodeKind::Binding {
                        name, annotation, ..
                    } = &tree.node(binding.node).kind
                    else {
                        return false;
                    };

                    annotation.is_none() && assigned_later(tree, *name, node, ancestors)
                });

                if later_assigned {
                    context.emit(
                        findings,
                        "implicit_any_local",
                        tree.node(node).span,
                        "unannotated local is assigned later",
                    );
                }
            }
        }

        NodeKind::Parameters { parameters, .. } if context.enabled("implicit_any_parameter") => {
            for parameter in tree.list(parameters) {
                if let NodeKind::Binding {
                    annotation: None,
                    name,
                    ..
                } = &tree.node(parameter.node).kind
                {
                    context.emit(
                        findings,
                        "implicit_any_parameter",
                        tree.node(*name).span,
                        "parameter has implicit any type",
                    );
                }
            }
        }

        NodeKind::Table { fields, .. } if context.enabled("mixed_table") => {
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
                    findings,
                    "mixed_table",
                    tree.node(node).span,
                    "table mixes positional and keyed entries",
                );
            }
        }

        _ => {}
    }
}

fn check_assignments(
    node: NodeIndex,
    ancestors: &[NodeIndex],
    context: &Context<'_>,
    findings: &mut Vec<Finding>,
) {
    let tree = context.tree;

    if let NodeKind::Assignment {
        targets, values, ..
    } = &tree.node(node).kind
        && context.enabled("self_assignment")
    {
        for (target, value) in tree.list(targets).iter().zip(tree.list(values)) {
            if stable_access(tree, target.node) && tree.text(target.node) == tree.text(value.node) {
                context.emit(
                    findings,
                    "self_assignment",
                    tree.node(target.node).span,
                    "assignment leaves value unchanged",
                );
            }
        }
    }

    if context.enabled("unscoped_variables") {
        let mut check_target = |target| {
            if matches!(tree.node(target).kind, NodeKind::Name { .. })
                && tree.text(target) != b"_G"
                && !is_declared(
                    tree.text(target),
                    tree.node(target).span.start,
                    ancestors,
                    context,
                )
            {
                context.emit(
                    findings,
                    "unscoped_variables",
                    tree.node(target).span,
                    "assignment creates an undeclared global",
                );
            }
        };

        match &tree.node(node).kind {
            NodeKind::Assignment { targets, .. } => {
                for target in tree.list(targets) {
                    check_target(target.node);
                }
            }

            NodeKind::CompoundAssignment { target, .. } => check_target(*target),
            _ => {}
        }
    }
}

pub(super) fn is_zero(tree: &Tree<'_>, node: NodeIndex) -> bool {
    match &tree.node(node).kind {
        NodeKind::Group { expression, .. } => is_zero(tree, *expression),

        NodeKind::Unary { operator, operand }
            if tree.token(*operator).kind == TokenKind::Symbol(Symbol::Subtract) =>
        {
            is_zero(tree, *operand)
        }

        NodeKind::Number { .. } => {
            let Ok(value) = std::str::from_utf8(tree.text(node)) else {
                return false;
            };

            if let Some(digits) = value
                .strip_prefix("0x")
                .or_else(|| value.strip_prefix("0X"))
                .or_else(|| value.strip_prefix("0b"))
                .or_else(|| value.strip_prefix("0B"))
            {
                !digits.is_empty() && digits.bytes().all(|digit| digit == b'0')
            } else {
                value.parse::<f64>().is_ok_and(|number| number == 0.0)
            }
        }

        _ => false,
    }
}

fn emit_empty_body(
    body: NodeIndex,
    rule: &'static str,
    message: &str,
    context: &Context<'_>,
    findings: &mut Vec<Finding>,
) {
    let tree = context.tree;

    let block = match &tree.node(body).kind {
        NodeKind::Else { body, .. } => *body,
        _ => body,
    };

    let NodeKind::Block { statements } = &tree.node(block).kind else {
        return;
    };

    if !tree.list(statements).is_empty() || has_comment(tree.node(body).span, context) {
        return;
    }

    context.emit(findings, rule, tree.node(body).span, message);
}

fn has_comment(span: Span, context: &Context<'_>) -> bool {
    context.tokens.iter().any(|token| {
        token.span.start >= span.start
            && token.span.end <= span.end
            && matches!(token.kind, TokenKind::Comment | TokenKind::BlockComment)
    })
}

fn same_body(left: NodeIndex, right: NodeIndex, context: &Context<'_>) -> bool {
    let tree = context.tree;

    let span = |body: NodeIndex| match &tree.node(body).kind {
        NodeKind::Else { body, .. } => tree.node(*body).span,
        _ => tree.node(body).span,
    };

    let tokens = |body: NodeIndex| {
        let range = span(body);

        context
            .tokens
            .iter()
            .filter(move |token| {
                token.span.start >= range.start
                    && token.span.end <= range.end
                    && token.kind != TokenKind::Whitespace
            })
            .map(|token| token.bytes(context.source.as_bytes()))
    };

    tokens(left).eq(tokens(right))
}

fn is_protected_call(tree: &Tree<'_>, call: NodeIndex) -> bool {
    matches!(&tree.node(call).kind, NodeKind::Call { callee, .. }
        if matches!(tree.node(*callee).kind, NodeKind::Name { .. })
            && matches!(tree.text(*callee), b"pcall" | b"xpcall"))
}

fn stable_access(tree: &Tree<'_>, node: NodeIndex) -> bool {
    match &tree.node(node).kind {
        NodeKind::Name { .. } => true,
        NodeKind::Field { receiver, .. } => stable_access(tree, *receiver),

        NodeKind::Index { receiver, key, .. } => {
            stable_access(tree, *receiver)
                && matches!(tree.node(*key).kind, NodeKind::String { .. })
        }

        NodeKind::Group { expression, .. } => stable_access(tree, *expression),
        _ => false,
    }
}

fn assigned_later(
    tree: &Tree<'_>,
    name: NodeIndex,
    declaration: NodeIndex,
    ancestors: &[NodeIndex],
) -> bool {
    ancestors
        .iter()
        .rev()
        .find_map(|ancestor| match &tree.node(*ancestor).kind {
            NodeKind::Block { statements } => Some(statements),
            _ => None,
        })
        .is_some_and(|statements| {
            tree.list(statements)
                .iter()
                .filter(|statement| {
                    tree.node(statement.node).span.start >= tree.node(declaration).span.end
                })
                .any(|statement| match &tree.node(statement.node).kind {
                    NodeKind::Assignment { targets, .. } => {
                        tree.list(targets).iter().any(|target| {
                            matches!(tree.node(target.node).kind, NodeKind::Name { .. })
                                && tree.text(target.node) == tree.text(name)
                        })
                    }

                    NodeKind::CompoundAssignment { target, .. } => {
                        matches!(tree.node(*target).kind, NodeKind::Name { .. })
                            && tree.text(*target) == tree.text(name)
                    }

                    _ => false,
                })
        })
}

fn is_declared(name: &[u8], before: usize, ancestors: &[NodeIndex], context: &Context<'_>) -> bool {
    const BUILTINS: &[&[u8]] = &[
        b"assert",
        b"bit32",
        b"buffer",
        b"coroutine",
        b"debug",
        b"error",
        b"game",
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
        b"shared",
        b"string",
        b"table",
        b"task",
        b"tonumber",
        b"tostring",
        b"type",
        b"typeof",
        b"utf8",
        b"warn",
        b"workspace",
        b"xpcall",
    ];

    if name != b"_G"
        && (BUILTINS.contains(&name)
            || context
                .globals
                .iter()
                .any(|global| global.as_bytes() == name))
    {
        return true;
    }

    has_local(context.tree, name, before, ancestors)
}

pub(super) fn has_local(
    tree: &Tree<'_>,
    name: &[u8],
    before: usize,
    ancestors: &[NodeIndex],
) -> bool {
    ancestors.iter().rev().any(|ancestor| match &tree.node(*ancestor).kind {
        NodeKind::Function { parameters, .. } => match &tree.node(*parameters).kind {
            NodeKind::Parameters { parameters, .. } => tree.list(parameters).iter().any(|parameter| {
                matches!(&tree.node(parameter.node).kind, NodeKind::Binding { name: binding, .. }
                    if tree.text(*binding) == name)
            }),

            _ => false,
        },

        NodeKind::NumericFor { binding, .. } => {
            matches!(&tree.node(*binding).kind, NodeKind::Binding { name: binding, .. }
                if tree.text(*binding) == name)
        }

        NodeKind::GenericFor { bindings, .. } => tree.list(bindings).iter().any(|binding| {
            matches!(&tree.node(binding.node).kind, NodeKind::Binding { name: binding, .. }
                if tree.text(*binding) == name)
        }),

        NodeKind::Block { statements } => tree.list(statements)
            .iter()
            .filter(|statement| tree.node(statement.node).span.end <= before)
            .any(|statement| match &tree.node(statement.node).kind {
                NodeKind::Local { bindings, .. } | NodeKind::Constant { bindings, .. } => tree.list(bindings).iter().any(|binding| {
                    matches!(&tree.node(binding.node).kind, NodeKind::Binding { name: binding, .. }
                        if tree.text(*binding) == name)
                }),

                NodeKind::Function { name: Some(binding), prefix: Some(prefix), .. }
                    if tree.token(*prefix).kind == TokenKind::Keyword(Keyword::Local) => tree.text(*binding) == name,

                _ => false,
            }),

        _ => false,
    })
}
