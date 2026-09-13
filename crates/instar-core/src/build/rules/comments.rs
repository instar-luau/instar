use super::{Context, replace_keep_lines};
use crate::build::{configuration::Rules, mapping::Edit};
use std::{fmt::Write, io};
use vermis::TokenKind;

pub(super) fn apply(
    context: &Context<'_, '_, '_, '_, '_>,
    settings: &Rules,
    edits: &mut Vec<Edit>,
) -> io::Result<()> {
    if let Some(comments) = settings
        .remove_comments
        .as_ref()
        .filter(|comments| comments.enabled())
    {
        for token in &context.tree.tokens {
            if !matches!(token.kind, TokenKind::Comment | TokenKind::BlockComment) {
                continue;
            }

            let comment = &context.text[token.span.start..token.span.end];

            let retained = comments.retains_directives() && comment.starts_with("--!")
                || comments
                    .exceptions()
                    .iter()
                    .try_fold(false, |matched, pattern| {
                        Ok::<_, io::Error>(matched || crate::luau::matches(pattern, comment)?)
                    })?;

            if retained {
                continue;
            }

            let mut start = token.span.start;

            while start > 0 && matches!(context.text.as_bytes()[start - 1], b' ' | b'\t') {
                start -= 1;
            }

            replace_keep_lines(context.text, start..token.span.end, "", edits);
        }
    }

    if let Some(directive) = &settings.add_luau_directive
        && !has_family(context.text, directive)
    {
        edits.push(Edit {
            range: 0..0,
            text: format!("--!{directive}\n"),
        });
    }

    if let Some(comment) = &settings.append_text_comment {
        let text = match (&comment.text, &comment.file) {
            (Some(text), None) => text.clone(),

            (None, Some(path)) => {
                let path = crate::build::paths::absolute(&context.root.join(path))?;

                std::str::from_utf8(
                    context
                        .files
                        .get(&path)
                        .ok_or_else(|| io::Error::other("missing appended comment file"))?,
                )
                .map_err(io::Error::other)?
                .to_owned()
            }

            _ => return Err(io::Error::other("invalid appended comment")),
        };

        let mut output = String::new();

        for line in text.lines() {
            writeln!(output, "-- {line}").expect("string writes cannot fail");
        }

        let offset = if settings
            .append_text_comment
            .as_ref()
            .is_some_and(|value| value.location == "start")
        {
            0
        } else {
            context.text.len()
        };

        let prefix = if offset == context.text.len()
            && !context.text.is_empty()
            && !context.text.ends_with('\n')
        {
            "\n"
        } else {
            ""
        };

        edits.push(Edit {
            range: offset..offset,
            text: format!("{prefix}{output}"),
        });
    }

    Ok(())
}

fn has_family(source: &str, directive: &str) -> bool {
    let selected = family(directive.split_whitespace().next().unwrap_or_default());

    source
        .lines()
        .take_while(|line| {
            let line = line.trim();

            line.is_empty() || line.starts_with("--")
        })
        .any(|line| {
            line.trim()
                .strip_prefix("--!")
                .and_then(|line| line.split_whitespace().next())
                .is_some_and(|existing| family(existing) == selected)
        })
}

fn family(value: &str) -> &str {
    match value {
        "strict" | "nonstrict" | "nocheck" => "typecheck",
        "native" => "native",
        "optimize" => "optimize",
        _ => "other",
    }
}
