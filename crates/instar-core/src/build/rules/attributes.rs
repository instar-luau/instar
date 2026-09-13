use super::{Context, replace_keep_lines, text};
use crate::build::{configuration::Rules, mapping::Edit};
use std::io;
use vermis::{Kind, Parts};

pub(super) fn apply(
    context: &Context<'_, '_, '_, '_, '_>,
    settings: &Rules,
    edits: &mut Vec<Edit>,
) -> io::Result<()> {
    let Some(rule) = settings
        .remove_attribute
        .as_ref()
        .filter(|rule| rule.enabled())
    else {
        return Ok(());
    };

    for index in 0..context.tree.nodes.len() {
        let Some(view) = context.tree.view(index) else {
            continue;
        };

        if view.kind() != Kind::Attributes {
            continue;
        }

        if rule.patterns().is_empty() {
            replace_keep_lines(context.text, super::span(view), "", edits);
            continue;
        }

        let Some(Parts::Attributes { attributes }) = view.parts() else {
            continue;
        };

        for attribute in attributes {
            let name = match attribute.parts() {
                Some(Parts::Attribute { name, .. }) => {
                    std::str::from_utf8(name.as_ref()).unwrap_or_default()
                }

                _ => text(attribute).trim_start_matches('@'),
            };

            let matched = rule.patterns().iter().try_fold(false, |matched, pattern| {
                Ok::<_, io::Error>(matched || crate::luau::matches(pattern, name)?)
            })?;

            if matched {
                replace_keep_lines(context.text, super::span(attribute), "", edits);
            }
        }
    }

    Ok(())
}
