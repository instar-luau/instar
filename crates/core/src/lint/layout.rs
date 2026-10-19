use vermis::{
    token::TokenKind,
    tree::{NodeIndex, NodeKind},
};

use crate::{
    lint::{Finding, context::Context},
    syntax::indentation as width,
};

enum Layout {
    Node(NodeIndex),
    End(NodeIndex),
}

pub(super) fn line(context: &Context<'_, '_>, offset: usize) -> usize {
    context
        .lines()
        .partition_point(|start| *start <= offset)
        .saturating_sub(1)
}

pub(super) fn check(context: &mut Context<'_, '_>, statement: NodeIndex) {
    let tree = context.tree;

    match &tree.node(statement).kind {
        NodeKind::Repeat { .. } => return,

        NodeKind::If { branches, .. } => {
            for branch in tree.list(branches) {
                check(context, branch.node);
            }

            return;
        }

        NodeKind::Export { declaration, .. } | NodeKind::Declaration { declaration, .. } => {
            check(context, *declaration);

            return;
        }

        NodeKind::Class {
            extends, members, ..
        } => {
            for header in extends
                .iter()
                .copied()
                .chain(tree.list(members).iter().map(|entry| entry.node))
            {
                check(context, header);
            }

            return;
        }

        _ => {}
    }

    let start = match tree.node(statement).kind {
        NodeKind::Function {
            keyword: Some(keyword),
            ..
        } => tree.token(keyword).span.start,

        _ => tree.node(statement).span.start,
    };

    let mut line = self::line(context, start);
    let column = indentation(context, line);
    let mut pending = vec![Layout::Node(statement)];

    while let Some(step) = pending.pop() {
        let node = match step {
            Layout::Node(node) => node,

            Layout::End(node) => {
                line = line.max(self::line(
                    context,
                    tree.node(node).span.end.saturating_sub(1),
                ));

                continue;
            }
        };

        let kind = &tree.node(node).kind;
        pending.push(Layout::End(node));

        match kind {
            NodeKind::Block { .. }
            | NodeKind::Table { .. }
            | NodeKind::TypeTable { .. }
            | NodeKind::Attributes { .. }
            | NodeKind::FunctionName { .. } => continue,

            NodeKind::Binding { annotation, .. }
            | NodeKind::Generic {
                default: annotation,
                ..
            } => {
                pending.extend(annotation.map(Layout::Node));

                continue;
            }

            NodeKind::Parameters { parameters, .. } | NodeKind::Generics { parameters, .. } => {
                line = line.max(self::line(context, tree.node(node).span.start));

                pending.extend(tree.list(parameters).iter().rev().filter_map(|entry| {
                    match tree.node(entry.node).kind {
                        NodeKind::Binding { annotation, .. }
                        | NodeKind::Variadic { annotation, .. }
                        | NodeKind::Generic {
                            default: annotation,
                            ..
                        } => annotation.map(Layout::Node),

                        _ => None,
                    }
                }));

                continue;
            }

            NodeKind::TypeParameter { annotation, .. } => {
                pending.push(Layout::Node(*annotation));

                continue;
            }

            _ => {}
        }

        if matches!(
            kind,
            NodeKind::Name { .. }
                | NodeKind::Number { .. }
                | NodeKind::String { .. }
                | NodeKind::Boolean { .. }
                | NodeKind::Nil { .. }
                | NodeKind::Variadic { .. }
                | NodeKind::Unary { .. }
                | NodeKind::Binary { .. }
                | NodeKind::Call { .. }
                | NodeKind::MethodCall { .. }
                | NodeKind::Field { .. }
                | NodeKind::Index { .. }
                | NodeKind::Instantiate { .. }
                | NodeKind::Group { .. }
                | NodeKind::Assertion { .. }
                | NodeKind::Conditional { .. }
                | NodeKind::Interpolation { .. }
                | NodeKind::Function { .. }
                | NodeKind::TypeName { .. }
                | NodeKind::TypeGroup { .. }
                | NodeKind::TypePack { .. }
                | NodeKind::GenericPack { .. }
                | NodeKind::VariadicType { .. }
                | NodeKind::TypeUnion { .. }
                | NodeKind::TypeIntersection { .. }
                | NodeKind::TypeOptional { .. }
                | NodeKind::TypeOf { .. }
                | NodeKind::TypeFunction { .. }
        ) {
            let current = self::line(context, tree.node(node).span.start);

            if current > line {
                line = current;

                if indentation(context, current) <= column {
                    context.warn(
                        node,
                        "MultiLineStatement",
                        "statement spans multiple lines; indent its continuation",
                    );

                    break;
                }
            }
        }

        pending.extend(tree.children(node).into_iter().rev().map(Layout::Node));
    }
}

fn indentation(context: &Context<'_, '_>, line: usize) -> usize {
    let source = &context.source.as_bytes()[context.lines()[line]..];

    let length = source
        .iter()
        .take_while(|byte| matches!(byte, b' ' | b'\t'))
        .count();

    width(&source[..length], context.settings.indent_width.get())
}

pub(super) fn comments(context: &mut Context<'_, '_>) {
    let mut leading = true;
    let mut mode = false;
    let mut native = false;

    for token in &context.tree.tokens {
        if matches!(token.kind, TokenKind::Whitespace | TokenKind::EndOfFile) {
            continue;
        }

        if !matches!(token.kind, TokenKind::Comment | TokenKind::BlockComment) {
            leading = false;
            continue;
        }

        let Some(directive) = token.bytes(context.tree.source).strip_prefix(b"--!") else {
            continue;
        };

        if directive.is_empty() || directive.first().is_some_and(u8::is_ascii_whitespace) {
            continue;
        }

        let directive = String::from_utf8_lossy(directive);
        let words: Vec<_> = directive.split_whitespace().collect();

        let message = if leading {
            match words.as_slice() {
                ["nolint", ..] | ["optimize", "0" | "1" | "2"] => None,

                ["strict" | "nonstrict" | "nocheck"] => {
                    if mode {
                        Some("type checking mode was already specified")
                    } else {
                        mode = true;

                        None
                    }
                }

                ["strict" | "nonstrict" | "nocheck", ..] => {
                    Some("type checking directive has extra symbols")
                }

                ["optimize", ..] => Some("optimize directive requires a level from 0 to 2"),

                ["native"] => {
                    native = true;

                    None
                }

                ["native", ..] => Some("native directive has extra symbols"),
                _ => Some("unknown comment directive"),
            }
        } else {
            Some("comment directive appears after the first non-comment token")
        };

        if let Some(message) = message
            && let Some(severity) = context.settings.enabled("CommentDirective")
        {
            context.findings.push(Finding {
                rule: "CommentDirective".to_owned(),
                severity,
                range: token.span.start..token.span.end,
                message: message.to_owned(),
                edits: Vec::new(),
            });
        }
    }

    if native && context.active(&["RedundantNativeAttribute"]) {
        for index in 0..context.tree.nodes.len() {
            let node = NodeIndex::new(index);

            if let NodeKind::Attribute { name, .. } = context.tree.node(node).kind
                && matches!(context.tree.text(name), b"native" | b"@native")
            {
                context.warn(
                    node,
                    "RedundantNativeAttribute",
                    "native attribute is redundant in a native module",
                );
            }
        }
    }
}
