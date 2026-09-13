use super::{Context, identifier, plain_string, replace_keep_lines, span, text};
use crate::build::{
    configuration::Rules,
    mapping::Edit,
    syntax::{field, kind, nodes, quote},
};
use std::collections::BTreeMap;
use vermis::{Kind, Parts, View};

pub(super) fn apply(
    context: &Context<'_, '_, '_, '_, '_>,
    settings: &Rules,
    edits: &mut Vec<Edit>,
) {
    if settings.use_get_service {
        services(context, edits);
    }

    if settings.dedupe_requires {
        requires(context, edits);
    }

    if let Some(name) = &settings.inject_module_path {
        module_path(context, name, edits);
    }

    if settings.freeze_module {
        freeze(context, edits);
    }
}

fn services(context: &Context<'_, '_, '_, '_, '_>, edits: &mut Vec<Edit>) {
    if context.tree.nodes.iter().enumerate().any(|(index, node)| {
        node.kind == Kind::Binding
            && context
                .tree
                .view(index)
                .is_some_and(|view| text(view).split(':').next() == Some("game"))
    }) {
        return;
    }

    for index in 0..context.tree.nodes.len() {
        let Some(view) = context.tree.view(index) else {
            continue;
        };

        let Some(Parts::Field { receiver, name }) = view.parts() else {
            continue;
        };

        let service = text(name);

        if receiver.kind() != Kind::Name
            || text(receiver) != "game"
            || !is_service(context, service)
            || assignment_target(index, context)
        {
            continue;
        }

        edits.push(Edit {
            range: span(view),
            text: format!("game:GetService({})", quote(service)),
        });
    }
}

fn is_service(context: &Context<'_, '_, '_, '_, '_>, name: &str) -> bool {
    context.environment.classes.iter().any(|class| {
        class
            .split_once('\0')
            .is_some_and(|(class_name, flags)| class_name == name && flags.starts_with('1'))
    })
}

fn assignment_target(mut index: usize, context: &Context<'_, '_, '_, '_, '_>) -> bool {
    while let Some(parent) = context.parents[index] {
        if matches!(
            context.tree.nodes[parent].kind,
            Kind::Assignment | Kind::CompoundAssignment
        ) {
            let Some(Parts::Assignment { targets, .. }) =
                context.tree.view(parent).and_then(View::parts)
            else {
                return false;
            };

            return targets.into_iter().any(|target| {
                target.span().start <= context.tree.nodes[index].span.start
                    && target.span().end >= context.tree.nodes[index].span.end
            });
        }

        if matches!(
            context.tree.nodes[parent].kind,
            Kind::Block | Kind::Function | Kind::LocalFunction
        ) {
            return false;
        }

        index = parent;
    }

    false
}

fn simple_require<'tree, 'source>(
    view: View<'tree, 'source>,
) -> Option<(&'source str, &'source str, View<'tree, 'source>)> {
    if view.kind() != Kind::Local {
        return None;
    }

    let Parts::Local {
        mut bindings,
        mut values,
    } = view.parts()?
    else {
        return None;
    };

    let binding = bindings.next()?;
    let value = values.next()?;

    if bindings.next().is_some()
        || values.next().is_some()
        || matches!(
            binding.parts(),
            Some(Parts::Binding {
                annotation: Some(_),
                ..
            })
        )
    {
        return None;
    }

    let Parts::Call { callee, arguments } = value.parts()? else {
        return None;
    };

    let Parts::Arguments { mut values } = arguments.parts()? else {
        return None;
    };

    let argument = values.next()?;

    if callee.kind() != Kind::Name
        || text(callee) != "require"
        || argument.kind() != Kind::String
        || values.next().is_some()
    {
        return None;
    }

    Some((
        text(binding).split(':').next()?,
        plain_string(argument)?,
        value,
    ))
}

fn requires(context: &Context<'_, '_, '_, '_, '_>, edits: &mut Vec<Edit>) {
    let Some(root) = context.tree.root_view() else {
        return;
    };

    let Some(Parts::Root { block }) = root.parts() else {
        return;
    };

    let Some(Parts::Block { statements }) = block.parts() else {
        return;
    };

    let mut seen = BTreeMap::new();

    for statement in statements {
        let Some((name, specifier, value)) = simple_require(statement) else {
            continue;
        };

        let Some(kept) = seen.get(specifier).copied() else {
            seen.insert(specifier, name);
            continue;
        };

        if kept != name {
            replace_keep_lines(context.text, span(value), kept, edits);
        }
    }
}

fn module_path(context: &Context<'_, '_, '_, '_, '_>, name: &str, edits: &mut Vec<Edit>) {
    let mut referenced = false;
    let mut bound = false;

    for node in nodes(&context.document["root"]) {
        if kind(node) == "AstExprGlobal" && field(node, "global") == name {
            referenced = true;
        } else if kind(node) == "AstLocal" && field(node, "name") == name {
            bound = true;
        }
    }

    if !referenced || bound || !identifier(name) {
        return;
    }

    let Some(mut index) = context.environment.node(context.path) else {
        return;
    };

    let mut names = Vec::new();

    loop {
        let node = &context.environment.nodes[index];

        let Some(parent) = node.parent else {
            break;
        };

        names.push(node.name.as_str());
        index = parent;
    }

    names.reverse();

    let path = if names.is_empty() {
        "@game".to_owned()
    } else {
        format!("@game/{}", names.join("/"))
    };

    let offset = context
        .tree
        .tokens
        .iter()
        .find(|token| {
            !matches!(
                token.kind,
                vermis::TokenKind::Whitespace
                    | vermis::TokenKind::Comment
                    | vermis::TokenKind::BlockComment
                    | vermis::TokenKind::Eof
            )
        })
        .map_or(context.text.len(), |token| token.span.start);

    let line_start = context.text[..offset]
        .rfind('\n')
        .map_or(0, |line| line + 1);

    let offset = if context.text[line_start..offset]
        .bytes()
        .all(|byte| matches!(byte, b' ' | b'\t'))
    {
        line_start
    } else {
        offset
    };

    edits.push(Edit {
        range: offset..offset,
        text: format!("local {name} = {}\n", quote(&path)),
    });
}

fn freeze(context: &Context<'_, '_, '_, '_, '_>, edits: &mut Vec<Edit>) {
    let Some(root) = context.tree.root_view() else {
        return;
    };

    let Some(Parts::Root { block }) = root.parts() else {
        return;
    };

    let Some(Parts::Block { statements }) = block.parts() else {
        return;
    };

    let statements = statements.collect::<Vec<_>>();

    let Some(returned) = statements.last().copied() else {
        return;
    };

    let Some(Parts::Return { mut values }) = returned.parts() else {
        return;
    };

    let Some(value) = values.next() else {
        return;
    };

    if values.next().is_some() {
        return;
    }

    if value.kind() == Kind::Table {
        wrap(value, edits);

        return;
    }

    if value.kind() != Kind::Name {
        return;
    }

    let name = text(value);

    let mut defined = false;

    for statement in &statements {
        let Some(Parts::Local { bindings, values }) = statement.parts() else {
            continue;
        };

        let bindings = bindings.collect::<Vec<_>>();

        if !bindings
            .iter()
            .any(|binding| text(*binding).split(':').next() == Some(name))
        {
            continue;
        }

        let values = values.collect::<Vec<_>>();

        if defined || bindings.len() != 1 || values.len() != 1 || values[0].kind() != Kind::Table {
            return;
        }

        defined = true;
    }

    if !defined || touched(name, context) {
        return;
    }

    wrap(value, edits);
}

fn wrap(view: View<'_, '_>, edits: &mut Vec<Edit>) {
    edits.push(Edit {
        range: view.span().start..view.span().start,
        text: "table.freeze(".into(),
    });

    edits.push(Edit {
        range: view.span().end..view.span().end,
        text: ")".into(),
    });
}

fn rooted(view: View<'_, '_>, name: &str) -> bool {
    match view.kind() {
        Kind::Name => text(view) == name,

        Kind::Field => {
            matches!(view.parts(), Some(Parts::Field { receiver, .. }) if rooted(receiver, name))
        }

        Kind::Index => {
            matches!(view.parts(), Some(Parts::Index { receiver, .. }) if rooted(receiver, name))
        }

        Kind::Group => {
            matches!(view.parts(), Some(Parts::Group { expression }) if rooted(expression, name))
        }

        _ => false,
    }
}

fn touched(name: &str, context: &Context<'_, '_, '_, '_, '_>) -> bool {
    for index in 0..context.tree.nodes.len() {
        let Some(view) = context.tree.view(index) else {
            continue;
        };

        if let Some(Parts::Assignment { targets, .. }) = view.parts()
            && targets.into_iter().any(|target| rooted(target, name))
        {
            return true;
        }

        if view.kind() == Kind::Function
            && let Some(Parts::Function {
                name: Some(function),
                ..
            }) = view.parts()
            && text(function)
                .split(['.', ':'])
                .next()
                .is_some_and(|root| root == name)
        {
            return true;
        }

        if let Some(Parts::Call { callee, arguments }) = view.parts()
            && callee.kind() == Kind::Name
            && text(callee) == "setmetatable"
            && arguments.children().any(|argument| rooted(argument, name))
        {
            return true;
        }
    }

    false
}
