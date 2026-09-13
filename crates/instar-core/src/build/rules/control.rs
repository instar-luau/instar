use super::{Context, expressions::truth, replace_keep_lines, span};
use crate::build::{configuration::Rules, mapping::Edit};
use std::collections::BTreeMap;
use vermis::{Kind, Parts, View};

pub(super) fn apply(
    context: &Context<'_, '_, '_, '_, '_>,
    settings: &Rules,
    edits: &mut Vec<Edit>,
) {
    for index in 0..context.tree.nodes.len() {
        let Some(view) = context.tree.view(index) else {
            continue;
        };

        if settings.remove_unused_while && view.kind() == Kind::While {
            unused_while(view, context.text, edits);
        }

        if settings.remove_empty_do && view.kind() == Kind::Do {
            empty_do(view, context.text, edits);
        }

        if settings.filter_after_early_return && view.kind() == Kind::Block {
            after_return(view, context.text, edits);
        }

        if settings.remove_unused_if_branch && view.kind() == Kind::If {
            unused_if(view, context.text, edits);
        }
    }

    if settings.remove_continue {
        continues(context, edits);
    }
}

fn unused_while(view: View<'_, '_>, source: &str, edits: &mut Vec<Edit>) {
    let Some(Parts::While { condition, .. }) = view.parts() else {
        return;
    };

    if truth(condition) == Some(false) {
        replace_keep_lines(source, span(view), "", edits);
    }
}

fn empty_do(view: View<'_, '_>, source: &str, edits: &mut Vec<Edit>) {
    let Some(Parts::Body { body }) = view.parts() else {
        return;
    };

    let Some(Parts::Block { mut statements }) = body.parts() else {
        return;
    };

    if statements.next().is_none() {
        replace_keep_lines(source, span(view), "", edits);
    }
}

fn after_return(view: View<'_, '_>, source: &str, edits: &mut Vec<Edit>) {
    let Some(Parts::Block { statements }) = view.parts() else {
        return;
    };

    let statements = statements.collect::<Vec<_>>();

    for (index, statement) in statements.iter().enumerate() {
        let Some(Parts::Body { body }) = statement.parts() else {
            continue;
        };

        let Some(Parts::Block { statements: body }) = body.parts() else {
            continue;
        };

        let body = body.collect::<Vec<_>>();

        if statement.kind() != Kind::Do
            || body
                .last()
                .is_none_or(|statement| statement.kind() != Kind::Return)
            || index + 1 == statements.len()
            || statements[index + 1..]
                .iter()
                .any(|statement| statement.kind() == Kind::TypeAlias)
        {
            continue;
        }

        replace_keep_lines(
            source,
            statements[index + 1].span().start..statements.last().unwrap().span().end,
            "",
            edits,
        );

        break;
    }
}

fn block_declares(view: View<'_, '_>) -> bool {
    let Some(Parts::Block { statements }) = view.parts() else {
        return false;
    };

    statements.into_iter().any(|statement| {
        matches!(
            statement.kind(),
            Kind::Local | Kind::Constant | Kind::LocalFunction
        )
    })
}

fn unused_if(view: View<'_, '_>, source: &str, edits: &mut Vec<Edit>) {
    let Some(Parts::If {
        branches,
        otherwise,
    }) = view.parts()
    else {
        return;
    };

    let branches = branches.collect::<Vec<_>>();
    let mut states = Vec::new();
    let mut truthy = None;

    for (index, branch) in branches.iter().enumerate() {
        let Some(Parts::Branch { condition, .. }) = branch.parts() else {
            return;
        };

        let state = truth(condition);
        states.push(state);

        if state == Some(true) {
            truthy = Some(index);
            break;
        }
    }

    states.resize(branches.len(), None);

    let survivors = branches
        .iter()
        .enumerate()
        .filter(|(index, _)| {
            states[*index] != Some(false) && truthy.is_none_or(|truthy| *index <= truthy)
        })
        .map(|(index, _)| index)
        .collect::<Vec<_>>();

    if survivors.len() == branches.len() && truthy.is_none() {
        return;
    }

    if survivors.is_empty() {
        if let Some(otherwise) = otherwise
            && let Some(Parts::Body { body }) = otherwise.parts()
        {
            unwrap(view, body, source, edits);
        } else {
            replace_keep_lines(source, span(view), "", edits);
        }

        return;
    }

    if let Some(truthy) = truthy
        && survivors == [truthy]
        && let Some(Parts::Branch { body, .. }) = branches[truthy].parts()
    {
        unwrap(view, body, source, edits);

        return;
    }

    for (index, branch) in branches.iter().enumerate() {
        if survivors.contains(&index) {
            continue;
        }

        let Some(Parts::Branch { condition, body }) = branch.parts() else {
            continue;
        };

        let start = branch_keyword(source, view.span().start, condition.span().start);
        replace_keep_lines(source, start..body.span().end, "", edits);
    }

    if truthy.is_some()
        && let Some(otherwise) = otherwise
    {
        replace_keep_lines(source, span(otherwise), "", edits);
    }

    if let Some(first) = survivors.first().copied()
        && first > 0
        && let Some(Parts::Branch { condition, .. }) = branches[first].parts()
    {
        let start = branch_keyword(source, view.span().start, condition.span().start);
        let end = start + "elseif".len();

        if source.get(start..end) == Some("elseif") {
            edits.push(Edit {
                range: start..end,
                text: "if".into(),
            });
        }
    }
}

fn branch_keyword(source: &str, start: usize, condition: usize) -> usize {
    let prefix = &source[start..condition];

    prefix
        .rfind("elseif")
        .map(|offset| start + offset)
        .or_else(|| prefix.rfind("if").map(|offset| start + offset))
        .unwrap_or(start)
}

fn unwrap(view: View<'_, '_>, body: View<'_, '_>, source: &str, edits: &mut Vec<Edit>) {
    let scoped = block_declares(body);

    replace_keep_lines(
        source,
        view.span().start..body.span().start,
        if scoped { "do" } else { "" },
        edits,
    );

    replace_keep_lines(
        source,
        body.span().end..view.span().end,
        if scoped { "end" } else { "" },
        edits,
    );
}

fn continues(context: &Context<'_, '_, '_, '_, '_>, edits: &mut Vec<Edit>) {
    let mut loops = BTreeMap::<usize, Vec<View<'_, '_>>>::new();

    for index in 0..context.tree.nodes.len() {
        let Some(view) = context.tree.view(index) else {
            continue;
        };

        if view.kind() == Kind::Continue
            && let Some(owner) = loop_owner(index, context)
            && context.tree.nodes[owner].kind != Kind::Repeat
        {
            loops.entry(owner).or_default().push(view);
        }
    }

    for (owner, statements) in loops {
        if owns_break(owner, context) {
            continue;
        }

        let Some(loop_view) = context.tree.view(owner) else {
            continue;
        };

        let Some(
            Parts::While { body, .. }
            | Parts::NumericFor { body, .. }
            | Parts::GenericFor { body, .. },
        ) = loop_view.parts()
        else {
            continue;
        };

        edits.push(Edit {
            range: body.span().start..body.span().start,
            text: "repeat ".into(),
        });

        edits.push(Edit {
            range: body.span().end..body.span().end,
            text: " until true".into(),
        });

        for statement in statements {
            edits.push(Edit {
                range: span(statement),
                text: "break".into(),
            });
        }
    }
}

fn loop_owner(mut index: usize, context: &Context<'_, '_, '_, '_, '_>) -> Option<usize> {
    while let Some(parent) = context.parents[index] {
        if matches!(
            context.tree.nodes[parent].kind,
            Kind::While | Kind::Repeat | Kind::NumericFor | Kind::GenericFor
        ) {
            return Some(parent);
        }

        index = parent;
    }

    None
}

fn owns_break(owner: usize, context: &Context<'_, '_, '_, '_, '_>) -> bool {
    context
        .tree
        .nodes
        .iter()
        .enumerate()
        .any(|(index, node)| node.kind == Kind::Break && loop_owner(index, context) == Some(owner))
}
