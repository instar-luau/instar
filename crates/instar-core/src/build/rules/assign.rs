use super::{Context, atomic, pure, reemittable, replace_keep_lines, span, text};
use crate::build::{configuration::Rules, mapping::Edit};
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

        if view.kind() == Kind::CompoundAssignment {
            if settings.remove_compound_assignment {
                compound(view, settings.remove_floor_division, false, edits);
            } else if settings.remove_floor_division {
                compound(view, true, true, edits);
            }
        }

        if settings.remove_floor_division && view.kind() == Kind::Binary {
            floor(view, edits);
        }

        if settings.remove_nil_declaration && view.kind() == Kind::Local {
            nils(view, context.text, edits);
        }

        if settings.make_assignment_local && view.kind() == Kind::Constant {
            keyword(view, context, "const", "local", edits);
        }

        if settings.group_local_assignment && view.kind() == Kind::Block {
            group(view, context.text, edits);
        }
    }
}

fn keyword(
    view: View<'_, '_>,
    context: &Context<'_, '_, '_, '_, '_>,
    from: &str,
    to: &str,
    edits: &mut Vec<Edit>,
) {
    if let Some(token) = context.tree.tokens.iter().find(|token| {
        token.span.start >= view.span().start
            && token.span.end <= view.span().end
            && token.utf8(context.tree.source).ok() == Some(from)
    }) {
        edits.push(Edit {
            range: token.span.start..token.span.end,
            text: to.into(),
        });
    }
}

fn compound(view: View<'_, '_>, floor: bool, only_floor: bool, edits: &mut Vec<Edit>) {
    let Some(Parts::Assignment {
        mut targets,
        operator,
        mut values,
    }) = view.parts()
    else {
        return;
    };

    let (Some(target), Some(value)) = (targets.next(), values.next()) else {
        return;
    };

    if targets.next().is_some() || values.next().is_some() || !reemittable(target) {
        return;
    }

    let operator_text = text(operator);

    if only_floor && operator_text != "//=" {
        return;
    }

    let plain = match operator_text {
        "+=" => "+",
        "-=" => "-",
        "*=" => "*",
        "/=" => "/",
        "%=" => "%",
        "^=" => "^",
        "..=" => "..",
        "//=" => "//",
        _ => return,
    };

    let target_text = text(target);

    edits.push(Edit {
        range: span(operator),
        text: if operator_text == "//=" && floor {
            format!("= math.floor({target_text} /")
        } else {
            format!("= {target_text} {plain}")
        },
    });

    if !atomic(value) {
        edits.push(Edit {
            range: value.span().start..value.span().start,
            text: "(".into(),
        });

        edits.push(Edit {
            range: value.span().end..value.span().end,
            text: ")".into(),
        });
    }

    if operator_text == "//=" && floor {
        edits.push(Edit {
            range: value.span().end..value.span().end,
            text: ")".into(),
        });
    }
}

fn floor(view: View<'_, '_>, edits: &mut Vec<Edit>) {
    let Some(Parts::Binary {
        left,
        operator,
        right,
    }) = view.parts()
    else {
        return;
    };

    if text(operator) != "//" {
        return;
    }

    edits.push(Edit {
        range: left.span().start..left.span().start,
        text: "math.floor(".into(),
    });

    edits.push(Edit {
        range: span(operator),
        text: "/".into(),
    });

    edits.push(Edit {
        range: right.span().end..right.span().end,
        text: ")".into(),
    });
}

fn nils(view: View<'_, '_>, source: &str, edits: &mut Vec<Edit>) {
    let Some(Parts::Local {
        mut bindings,
        values,
    }) = view.parts()
    else {
        return;
    };

    let bindings = bindings.by_ref().collect::<Vec<_>>();
    let values = values.collect::<Vec<_>>();

    if bindings.is_empty() || values.is_empty() {
        return;
    }

    let keep = values
        .iter()
        .rposition(|value| value.kind() != Kind::Nil)
        .map_or(0, |index| index + 1);

    if keep == values.len() {
        return;
    }

    let end = values
        .last()
        .map_or(view.span().end, |value| value.span().end);

    let start = if keep == 0 {
        source[bindings.last().unwrap().span().end..values[0].span().start]
            .find('=')
            .map(|offset| bindings.last().unwrap().span().end + offset)
    } else {
        source[values[keep - 1].span().end..values[keep].span().start]
            .find(',')
            .map(|offset| values[keep - 1].span().end + offset)
    };

    if let Some(mut start) = start {
        while start > 0 && matches!(source.as_bytes()[start - 1], b' ' | b'\t') {
            start -= 1;
        }

        replace_keep_lines(source, start..end, "", edits);
    }
}

fn reads_any(view: View<'_, '_>, names: &[String]) -> bool {
    if view.kind() == Kind::Name {
        return names.iter().any(|name| name == text(view));
    }

    match view.parts() {
        Some(Parts::Field { receiver, .. }) => reads_any(receiver, names),

        Some(Parts::TableField {
            key,
            value,
            indexed,
        }) => indexed && key.is_some_and(|key| reads_any(key, names)) || reads_any(value, names),

        _ => view.children().any(|child| reads_any(child, names)),
    }
}

fn group(view: View<'_, '_>, source: &str, edits: &mut Vec<Edit>) {
    let Some(Parts::Block { statements }) = view.parts() else {
        return;
    };

    let statements = statements.collect::<Vec<_>>();
    let mut index = 0;

    while index < statements.len() {
        let mut declarations = Vec::new();
        let mut names = Vec::<String>::new();
        let mut cursor = index;

        while let Some(statement) = statements.get(cursor).copied() {
            let Some(Parts::Local { bindings, values }) = statement.parts() else {
                break;
            };

            let bindings = bindings.collect::<Vec<_>>();
            let values = values.collect::<Vec<_>>();

            if statement.kind() != Kind::Local
                || bindings.is_empty()
                || bindings.len() != values.len()
                || bindings.iter().any(|binding| {
                    matches!(
                        binding.parts(),
                        Some(Parts::Binding {
                            annotation: Some(_),
                            ..
                        })
                    )
                })
                || values.iter().any(|value| !pure(*value))
                || values.iter().any(|value| reads_any(*value, &names))
                || bindings.iter().any(|binding| {
                    names
                        .iter()
                        .any(|name| name == text(*binding).split(':').next().unwrap_or_default())
                })
                || declarations.last().is_some_and(|previous: &View<'_, '_>| {
                    let gap = &source[previous.span().end..statement.span().start];

                    gap.bytes().filter(|byte| *byte == b'\n').count() != 1 || gap.contains("--")
                })
            {
                break;
            }

            names.extend(bindings.iter().map(|binding| {
                text(*binding)
                    .split(':')
                    .next()
                    .unwrap_or_default()
                    .to_owned()
            }));

            declarations.push(statement);
            cursor += 1;
        }

        if declarations.len() > 1 {
            let mut names = Vec::new();
            let mut values = Vec::new();

            for declaration in &declarations {
                if let Some(Parts::Local {
                    bindings,
                    values: declaration_values,
                }) = declaration.parts()
                {
                    names.extend(bindings.map(text));
                    values.extend(declaration_values.map(text));
                }
            }

            replace_keep_lines(
                source,
                declarations[0].span().start..declarations.last().unwrap().span().end,
                &format!("local {} = {}", names.join(", "), values.join(", ")),
                edits,
            );
        }

        index = cursor.max(index + 1);
    }
}
