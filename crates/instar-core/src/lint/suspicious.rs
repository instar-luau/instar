use vermis::{Kind, Parts, Span, TokenKind, View};

use super::{Context, Finding};

pub(super) fn check(
    node: View<'_, '_>,
    ancestors: &[View<'_, '_>],
    context: &Context<'_>,
    findings: &mut Vec<Finding>,
) {
    match node.parts() {
        Some(Parts::Binary {
            operator, right, ..
        }) if matches!(operator.text(), b"/" | b"//" | b"%")
            && context.enabled("divide_by_zero")
            && is_zero(right) =>
        {
            context.emit(
                findings,
                "divide_by_zero",
                operator.span(),
                "division or modulo by zero",
            );
        }

        Some(Parts::If {
            branches,
            otherwise,
        }) if context.enabled("empty_if") => {
            for branch in branches {
                if let Some(Parts::Branch { body, .. }) = branch.parts() {
                    emit_empty_body(
                        body,
                        "empty_if",
                        "empty conditional branch",
                        context,
                        findings,
                    );
                }
            }

            if let Some(otherwise) = otherwise {
                emit_empty_body(
                    otherwise,
                    "empty_if",
                    "empty conditional branch",
                    context,
                    findings,
                );
            }
        }

        Some(Parts::While { body, .. } | Parts::Repeat { body, .. })
            if context.enabled("empty_loop") =>
        {
            emit_empty_body(body, "empty_loop", "empty loop body", context, findings);
        }

        Some(Parts::NumericFor { body, .. } | Parts::GenericFor { body, .. })
            if context.enabled("empty_loop") =>
        {
            emit_empty_body(body, "empty_loop", "empty loop body", context, findings);
        }

        Some(_)
            if node.kind() == Kind::Name
                && node.text() == b"_G"
                && context.enabled("global_usage") =>
        {
            if !ancestors
                .last()
                .is_some_and(|parent| parent.kind() == Kind::Binding)
                && !is_declared(b"_G", node.span().start, ancestors, context)
            {
                context.emit(findings, "global_usage", node.span(), "access to shared _G");
            }
        }

        Some(Parts::CallStatement { call })
            if context.enabled("ignored_pcall_result") && is_protected_call(call) =>
        {
            context.emit(
                findings,
                "ignored_pcall_result",
                call.span(),
                "pcall result is discarded",
            );
        }

        _ => {}
    }

    check_declarations(node, ancestors, context, findings);
    check_assignments(node, ancestors, context, findings);

    if let Some(Parts::If {
        branches,
        otherwise: Some(otherwise),
    }) = node.parts()
        && context.enabled("if_same_then_else")
    {
        let mut bodies = branches.filter_map(|branch| match branch.parts() {
            Some(Parts::Branch { body, .. }) => Some(body),
            _ => None,
        });

        if let Some(first) = bodies.next()
            && bodies.all(|body| same_body(first, body, context))
            && same_body(first, otherwise, context)
        {
            context.emit(
                findings,
                "if_same_then_else",
                node.span(),
                "if branches have identical bodies",
            );
        }
    }
}

fn check_declarations(
    node: View<'_, '_>,
    ancestors: &[View<'_, '_>],
    context: &Context<'_>,
    findings: &mut Vec<Finding>,
) {
    match node.parts() {
        Some(Parts::Local {
            mut bindings,
            mut values,
        }) if context.enabled("implicit_any_local") => {
            if values.next().is_none() {
                let later_assigned = bindings.any(|binding| {
                    let Some(Parts::Binding { name, annotation }) = binding.parts() else {
                        return false;
                    };

                    annotation.is_none() && assigned_later(name, node, ancestors)
                });

                if later_assigned {
                    context.emit(
                        findings,
                        "implicit_any_local",
                        node.span(),
                        "unannotated local is assigned later",
                    );
                }
            }
        }

        Some(Parts::Parameters { parameters }) if context.enabled("implicit_any_parameter") => {
            for parameter in parameters {
                if let Some(Parts::Binding {
                    annotation: None,
                    name,
                }) = parameter.parts()
                {
                    context.emit(
                        findings,
                        "implicit_any_parameter",
                        name.span(),
                        "parameter has implicit any type",
                    );
                }
            }
        }

        Some(Parts::Table { fields }) if context.enabled("mixed_table") => {
            let mut positional = false;
            let mut keyed = false;

            for field in fields {
                if let Some(Parts::TableField { key, indexed, .. }) = field.parts() {
                    if key.is_some() || indexed {
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
                    node.span(),
                    "table mixes positional and keyed entries",
                );
            }
        }

        _ => {}
    }
}

fn check_assignments(
    node: View<'_, '_>,
    ancestors: &[View<'_, '_>],
    context: &Context<'_>,
    findings: &mut Vec<Finding>,
) {
    if let Some(Parts::Assignment {
        targets,
        values,
        operator,
    }) = node.parts()
        && operator.text() == b"="
        && context.enabled("self_assignment")
    {
        let targets: Vec<_> = targets.collect();
        let values: Vec<_> = values.collect();

        for (target, value) in targets.iter().zip(values) {
            if stable_access(*target) && target.text() == value.text() {
                context.emit(
                    findings,
                    "self_assignment",
                    target.span(),
                    "assignment leaves value unchanged",
                );
            }
        }
    }

    if let Some(Parts::Assignment { targets, .. }) = node.parts()
        && context.enabled("unscoped_variables")
    {
        for target in targets {
            if target.kind() == Kind::Name
                && target.text() != b"_G"
                && !is_declared(target.text(), target.span().start, ancestors, context)
            {
                context.emit(
                    findings,
                    "unscoped_variables",
                    target.span(),
                    "assignment creates an undeclared global",
                );
            }
        }
    }
}

pub(super) fn is_zero(node: View<'_, '_>) -> bool {
    match node.parts() {
        Some(
            Parts::Group { expression }
            | Parts::Unary {
                operator: _,
                operand: expression,
            },
        ) if node.kind() == Kind::Group || node.text().starts_with(b"-") => is_zero(expression),

        _ if node.kind() == Kind::Number => {
            let Ok(value) = std::str::from_utf8(node.text()) else {
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
    body: View<'_, '_>,
    rule: &'static str,
    message: &str,
    context: &Context<'_>,
    findings: &mut Vec<Finding>,
) {
    let block = match body.parts() {
        Some(Parts::Body { body }) => body,
        _ => body,
    };

    let Some(Parts::Block { mut statements }) = block.parts() else {
        return;
    };

    if statements.next().is_some() || has_comment(body.span(), context) {
        return;
    }

    context.emit(findings, rule, body.span(), message);
}

fn has_comment(span: Span, context: &Context<'_>) -> bool {
    context.tokens.iter().any(|token| {
        token.span.start >= span.start
            && token.span.end <= span.end
            && matches!(token.kind, TokenKind::Comment | TokenKind::BlockComment)
    })
}

fn same_body(left: View<'_, '_>, right: View<'_, '_>, context: &Context<'_>) -> bool {
    let span = |body: View<'_, '_>| match body.parts() {
        Some(Parts::Body { body }) => body.span(),
        _ => body.span(),
    };

    let tokens = |body: View<'_, '_>| {
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

fn is_protected_call(call: View<'_, '_>) -> bool {
    match call.parts() {
        Some(Parts::Call { callee, .. }) => {
            callee.kind() == Kind::Name && callee.text() == b"pcall"
                || callee.kind() == Kind::Name && callee.text() == b"xpcall"
        }

        _ => false,
    }
}

fn stable_access(node: View<'_, '_>) -> bool {
    match node.parts() {
        Some(_) if node.kind() == Kind::Name => true,
        Some(Parts::Field { receiver, .. }) => stable_access(receiver),

        Some(Parts::Index { receiver, key }) => {
            stable_access(receiver) && key.kind() == Kind::String
        }

        Some(Parts::Group { expression }) => stable_access(expression),
        _ => false,
    }
}

fn assigned_later(
    name: View<'_, '_>,
    declaration: View<'_, '_>,
    ancestors: &[View<'_, '_>],
) -> bool {
    ancestors
        .iter()
        .rev()
        .find_map(|ancestor| match ancestor.parts() {
            Some(Parts::Block { statements }) => Some(statements),
            _ => None,
        })
        .is_some_and(|statements| {
            statements
                .filter(|statement| statement.span().start >= declaration.span().end)
                .any(|statement| match statement.parts() {
                    Some(Parts::Assignment { targets, .. }) => targets
                        .into_iter()
                        .any(|target| target.kind() == Kind::Name && target.text() == name.text()),

                    _ => false,
                })
        })
}

fn is_declared(
    name: &[u8],
    before: usize,
    ancestors: &[View<'_, '_>],
    context: &Context<'_>,
) -> bool {
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

    has_local(name, before, ancestors)
}

pub(super) fn has_local(name: &[u8], before: usize, ancestors: &[View<'_, '_>]) -> bool {
    ancestors
        .iter()
        .rev()
        .any(|ancestor| match ancestor.parts() {
            Some(Parts::Function { parameters, .. }) => match parameters.parts() {
                Some(Parts::Parameters { mut parameters }) => {
                    parameters.any(|parameter| match parameter.parts() {
                        Some(Parts::Binding { name: binding, .. }) => binding.text() == name,
                        _ => false,
                    })
                }

                _ => false,
            },

            Some(Parts::NumericFor { binding, .. }) => match binding.parts() {
                Some(Parts::Binding { name: binding, .. }) => binding.text() == name,
                _ => false,
            },

            Some(Parts::GenericFor { mut bindings, .. }) => {
                bindings.any(|binding| match binding.parts() {
                    Some(Parts::Binding { name: binding, .. }) => binding.text() == name,
                    _ => false,
                })
            }

            Some(Parts::Block { statements }) => statements
                .filter(|statement| statement.span().end <= before)
                .any(|statement| match statement.parts() {
                    Some(Parts::Local { mut bindings, .. }) => {
                        bindings.any(|binding| match binding.parts() {
                            Some(Parts::Binding { name: binding, .. }) => binding.text() == name,
                            _ => false,
                        })
                    }

                    Some(Parts::Function {
                        name: Some(binding),
                        ..
                    }) if statement.kind() == Kind::LocalFunction => binding.text() == name,

                    _ => false,
                }),

            _ => false,
        })
}
