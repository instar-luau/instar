use vermis::{Kind, Parts, TokenKind, View};

use super::{Context, Finding};

pub(super) fn check(
    node: View<'_, '_>,
    ancestors: &[View<'_, '_>],
    context: &Context<'_>,
    findings: &mut Vec<Finding>,
) {
    match node.kind() {
        Kind::Binary => and_or_conditional(node, context, findings),
        Kind::If => collapsible_if(node, context, findings),

        Kind::Call | Kind::MethodCall => {
            deprecated(node, context, findings);
            restricted_module_path(node, ancestors, context, findings);
        }

        Kind::Branch | Kind::While | Kind::Repeat => parenthese_conditions(node, context, findings),
        Kind::Else => else_after_return(node, ancestors, context, findings),
        Kind::Conditional => if_expression_assignment(node, ancestors, context, findings),
        Kind::Unary => negated_condition(node, ancestors, context, findings),

        Kind::Local => {
            non_const_require(node, ancestors, context, findings);
            prefer_const(node, context, findings);
        }

        Kind::Name => restricted_global(node, ancestors, context, findings),
        _ => {}
    }
}

fn and_or_conditional(node: View<'_, '_>, context: &Context<'_>, findings: &mut Vec<Finding>) {
    if !context.enabled("and_or_conditional") {
        return;
    }

    let Some(Parts::Binary { left, operator, .. }) = node.parts() else {
        return;
    };

    if operator.text() != b"or" {
        return;
    }

    let Some(Parts::Binary {
        right, operator, ..
    }) = left.parts()
    else {
        return;
    };

    if operator.text() == b"and"
        && right.kind() != Kind::Nil
        && !(right.kind() == Kind::Boolean && right.text() == b"false")
    {
        context.emit(
            findings,
            "and_or_conditional",
            node.span(),
            "and/or conditional used as a value",
        );
    }
}

fn collapsible_if(node: View<'_, '_>, context: &Context<'_>, findings: &mut Vec<Finding>) {
    if !context.enabled("collapsible_if") {
        return;
    }

    let Some(Parts::If {
        branches,
        otherwise: None,
    }) = node.parts()
    else {
        return;
    };

    let mut branches = branches.into_iter();

    let Some(branch) = branches.next() else {
        return;
    };

    if branches.next().is_some() {
        return;
    }

    let Some(Parts::Branch { body, .. }) = branch.parts() else {
        return;
    };

    let Some(Parts::Block { mut statements }) = body.parts() else {
        return;
    };

    let Some(statement) = statements.next() else {
        return;
    };

    if statements.next().is_some() || statement.kind() != Kind::If {
        return;
    }

    let Some(Parts::If {
        branches,
        otherwise: None,
    }) = statement.parts()
    else {
        return;
    };

    if branches.into_iter().count() == 1 {
        context.emit(
            findings,
            "collapsible_if",
            node.span(),
            "nested single-branch if statements can be combined",
        );
    }
}

fn else_after_return(
    node: View<'_, '_>,
    ancestors: &[View<'_, '_>],
    context: &Context<'_>,
    findings: &mut Vec<Finding>,
) {
    if !context.enabled("else_after_return") {
        return;
    }

    let Some(parent) = ancestors.last() else {
        return;
    };

    if parent.kind() != Kind::If {
        return;
    }

    let Some(Parts::If { branches, .. }) = parent.parts() else {
        return;
    };

    let exits = branches.into_iter().all(|branch| {
        let Some(Parts::Branch { body, .. }) = branch.parts() else { return false };

        matches!(body.parts(), Some(Parts::Block { statements })
            if statements.clone().next_back().is_some_and(|statement| matches!(statement.kind(), Kind::Return | Kind::Break | Kind::Continue)))
    });

    if exits {
        context.emit(
            findings,
            "else_after_return",
            node.span(),
            "else branch follows branches that exit",
        );
    }
}

fn if_expression_assignment(
    node: View<'_, '_>,
    ancestors: &[View<'_, '_>],
    context: &Context<'_>,
    findings: &mut Vec<Finding>,
) {
    if !context.enabled("if_expression_assignment") {
        return;
    }

    if !matches!(
        ancestors.last().map(|parent| parent.kind()),
        Some(Kind::Local | Kind::Constant | Kind::Assignment)
    ) {
        return;
    }

    context.emit(
        findings,
        "if_expression_assignment",
        node.span(),
        "if-expression is used as a value",
    );
}

fn negated_condition(
    node: View<'_, '_>,
    ancestors: &[View<'_, '_>],
    context: &Context<'_>,
    findings: &mut Vec<Finding>,
) {
    if !context.enabled("negated_condition") {
        return;
    }

    let Some(branch) = ancestors.last() else {
        return;
    };

    if branch.kind() != Kind::Branch {
        return;
    }

    let Some(Parts::Branch { condition, .. }) = branch.parts() else {
        return;
    };

    if condition.span() != node.span() {
        return;
    }

    let Some(if_node) = ancestors
        .iter()
        .rev()
        .find(|ancestor| ancestor.kind() == Kind::If)
    else {
        return;
    };

    let Some(Parts::If {
        branches,
        otherwise: Some(_),
    }) = if_node.parts()
    else {
        return;
    };

    if branches.into_iter().count() != 1 {
        return;
    }

    if matches!(node.parts(), Some(Parts::Unary { operator, .. }) if operator.text() == b"not") {
        context.emit(
            findings,
            "negated_condition",
            node.span(),
            "negated if condition could be expressed by swapping branches",
        );
    }
}

fn non_const_require(
    node: View<'_, '_>,
    ancestors: &[View<'_, '_>],
    context: &Context<'_>,
    findings: &mut Vec<Finding>,
) {
    if !context.enabled("non_const_require") {
        return;
    }

    let Some(Parts::Local { bindings, values }) = node.parts() else {
        return;
    };

    for (binding, value) in bindings.zip(values) {
        let Some(Parts::Binding { name, .. }) = binding.parts() else {
            continue;
        };

        if is_require(value)
            && !super::suspicious::has_local(b"require", value.span().start, ancestors)
            && !assigned_again(context, name)
        {
            context.emit(
                findings,
                "non_const_require",
                binding.span(),
                "unchanged require binding can be declared const",
            );
        }
    }
}

fn prefer_const(node: View<'_, '_>, context: &Context<'_>, findings: &mut Vec<Finding>) {
    if !context.enabled("prefer_const") {
        return;
    }

    let Some(Parts::Local { bindings, values }) = node.parts() else {
        return;
    };

    for (binding, value) in bindings.zip(values) {
        let Some(Parts::Binding { name, .. }) = binding.parts() else {
            continue;
        };

        if assigned_again(context, name) {
            continue;
        }

        if context
            .config
            .options
            .prefer_const
            .mutated_tables_stay_local()
            && value.kind() == Kind::Table
            && mutated_table(context, name)
        {
            continue;
        }

        context.emit(
            findings,
            "prefer_const",
            binding.span(),
            "unchanged valued local can be declared const",
        );
    }
}

fn significant(kind: TokenKind) -> bool {
    !matches!(
        kind,
        TokenKind::Whitespace | TokenKind::Comment | TokenKind::BlockComment
    )
}

fn next_token(context: &Context<'_>, after: usize) -> Option<vermis::Token> {
    context
        .tokens
        .iter()
        .copied()
        .find(|token| token.span.start >= after && significant(token.kind))
}

fn assigned_again(context: &Context<'_>, name: View<'_, '_>) -> bool {
    let mut previous = None;

    for token in context
        .tokens
        .iter()
        .copied()
        .filter(|token| significant(token.kind))
    {
        if token.span.start <= name.span().start {
            previous = Some(token.kind);
            continue;
        }

        if token.bytes(context.source.as_bytes()) != name.text() {
            previous = Some(token.kind);
            continue;
        }

        if matches!(previous, Some(TokenKind::Keyword(vermis::Keyword::Local))) {
            return true;
        }

        let next = next_token(context, token.span.end).map(|token| token.kind);

        let assignment = matches!(
            next,
            Some(
                TokenKind::Byte(b'=')
                    | TokenKind::Operator(
                        vermis::Operator::AddAssign
                            | vermis::Operator::SubtractAssign
                            | vermis::Operator::MultiplyAssign
                            | vermis::Operator::DivideAssign
                            | vermis::Operator::FloorDivideAssign
                            | vermis::Operator::ModuloAssign
                            | vermis::Operator::PowerAssign
                            | vermis::Operator::ConcatAssign
                    )
            )
        );

        if assignment && !matches!(previous, Some(TokenKind::Byte(b'.' | b':'))) {
            return true;
        }

        previous = Some(token.kind);
    }

    false
}

fn mutated_table(context: &Context<'_>, name: View<'_, '_>) -> bool {
    for (index, token) in context.tokens.iter().enumerate() {
        if token.span.start <= name.span().start
            || token.kind != TokenKind::Name
            || token.bytes(context.source.as_bytes()) != name.text()
        {
            continue;
        }

        let Some(access) = next_token(context, token.span.end) else {
            continue;
        };

        if access.kind == TokenKind::Byte(b'.') {
            if next_token(context, access.span.end)
                .and_then(|field| next_token(context, field.span.end))
                .is_some_and(|operator| operator.kind == TokenKind::Byte(b'='))
            {
                return true;
            }
        } else if access.kind == TokenKind::Byte(b'[') {
            let mut depth = 0usize;

            for candidate in context.tokens[index + 1..]
                .iter()
                .filter(|candidate| significant(candidate.kind))
            {
                match candidate.kind {
                    TokenKind::Byte(b'[') => depth += 1,

                    TokenKind::Byte(b']') => {
                        depth -= 1;

                        if depth == 0 {
                            return next_token(context, candidate.span.end)
                                .is_some_and(|next| next.kind == TokenKind::Byte(b'='));
                        }
                    }

                    _ => {}
                }
            }
        }
    }

    false
}

fn is_require(node: View<'_, '_>) -> bool {
    matches!(node.parts(), Some(Parts::Call { callee, .. }) if callee.kind() == Kind::Name && callee.text() == b"require")
}

fn deprecated(node: View<'_, '_>, context: &Context<'_>, findings: &mut Vec<Finding>) {
    if !context.enabled("deprecated") {
        return;
    }

    let options = &context.config.options.deprecated;

    if options.additional.is_empty() {
        return;
    }

    if let Some(path) = call_path(node)
        && let Some(replacement) = options.additional.get(&path)
    {
        context.emit(
            findings,
            "deprecated",
            node.span(),
            format!("{path} is deprecated; use {replacement}"),
        );

        return;
    }

    if options.ambiguous_methods()
        && let Some(method) = method_name(node)
    {
        for (path, replacement) in &options.additional {
            if path.rsplit('.').next() == Some(method.as_str()) {
                context.emit(
                    findings,
                    "deprecated",
                    node.span(),
                    format!("method {method} may be deprecated; use {replacement}"),
                );

                break;
            }
        }
    }
}

fn method_name(node: View<'_, '_>) -> Option<String> {
    match node.parts()? {
        Parts::MethodCall { method, .. } => Some(text(method.text())),
        _ => None,
    }
}

fn call_path(node: View<'_, '_>) -> Option<String> {
    match node.parts()? {
        Parts::Call { callee, .. } => expression_path(callee),

        Parts::MethodCall {
            receiver, method, ..
        } => Some(format!(
            "{}.{}",
            expression_path(receiver)?,
            text(method.text())
        )),

        _ => None,
    }
}

fn expression_path(node: View<'_, '_>) -> Option<String> {
    match node.parts()? {
        Parts::Leaf if node.kind() == Kind::Name => Some(text(node.text())),

        Parts::Field { receiver, name } => Some(format!(
            "{}.{}",
            expression_path(receiver)?,
            text(name.text())
        )),

        Parts::Group { expression } => expression_path(expression),
        _ => None,
    }
}

fn restricted_global(
    node: View<'_, '_>,
    ancestors: &[View<'_, '_>],
    context: &Context<'_>,
    findings: &mut Vec<Finding>,
) {
    let restricted = &context.config.options.restricted_globals;

    if !context.enabled("restricted_globals") || restricted.is_empty() {
        return;
    }

    if ancestors.iter().any(|parent| {
        matches!(
            parent.kind(),
            Kind::Binding | Kind::FunctionName | Kind::TypeName
        )
    }) {
        return;
    }

    if ancestors.last().is_some_and(|parent| match parent.parts() {
        Some(Parts::Field { name, .. }) => name.span() == node.span(),
        Some(Parts::MethodCall { method, .. }) => method.span() == node.span(),
        _ => false,
    }) {
        return;
    }

    let Ok(name) = std::str::from_utf8(node.text()) else {
        return;
    };

    let Some(reason) = restricted.get(name) else {
        return;
    };

    if super::suspicious::has_local(node.text(), node.span().start, ancestors) {
        return;
    }

    context.emit(
        findings,
        "restricted_globals",
        node.span(),
        format!("global {name} is restricted: {reason}"),
    );
}

fn restricted_module_path(
    node: View<'_, '_>,
    ancestors: &[View<'_, '_>],
    context: &Context<'_>,
    findings: &mut Vec<Finding>,
) {
    let paths = &context.config.options.restricted_module_paths.paths;

    if !context.enabled("restricted_module_paths")
        || paths.is_empty()
        || !is_require(node)
        || super::suspicious::has_local(b"require", node.span().start, ancestors)
    {
        return;
    }

    let Some(Parts::Call { arguments, .. }) = node.parts() else {
        return;
    };

    let Some(argument) = arguments.children().next() else {
        return;
    };

    if argument.kind() != Kind::String {
        return;
    }

    let Ok(path) = crate::string_value(argument.text()) else {
        return;
    };

    if let Some(reason) = paths.get(&path) {
        context.emit(
            findings,
            "restricted_module_paths",
            argument.span(),
            format!("module path {path} is restricted: {reason}"),
        );
    }
}

fn parenthese_conditions(node: View<'_, '_>, context: &Context<'_>, findings: &mut Vec<Finding>) {
    if !context.enabled("parenthese_conditions") {
        return;
    }

    let Some(
        Parts::Branch { condition, .. }
        | Parts::While { condition, .. }
        | Parts::Repeat { condition, .. },
    ) = node.parts()
    else {
        return;
    };

    if condition.kind() == Kind::Group {
        context.emit(
            findings,
            "parenthese_conditions",
            condition.span(),
            "unnecessary parentheses around condition",
        );
    }
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}
