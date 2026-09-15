use super::{Options, sorting};
use crate::format::scope::Names;
use crate::project::configuration::format::{Binding, Declaration, Unused};
use std::{io, ops::Range};
use vermis::{Kind, Parts, Tree, View};

pub(in crate::format) fn prepare(
    source: &str,
    tree: &Tree<'_>,
    options: &Options,
    held: &[Range<usize>],
) -> io::Result<String> {
    let mut original_comments = crate::format::comments(tree, source.as_bytes());
    original_comments.sort_unstable();
    let source = bindings(source, tree, options, held)?;
    let tree = vermis::parse(source.as_bytes().into());
    let held = crate::format::regions::held(&source, &tree);
    let output = sorting::sort(&source, &tree, options, &held)?;
    let parsed = vermis::parse(output.as_bytes().into());

    if !parsed.diagnostics.is_empty() {
        return Err(io::Error::other(
            "format rewrite produced invalid syntax; source was not written",
        ));
    }

    let mut output_comments = crate::format::comments(&parsed, output.as_bytes());
    output_comments.sort_unstable();

    if original_comments != output_comments {
        return Err(io::Error::other(
            "format rewrite changed comments; source was not written",
        ));
    }

    Ok(output)
}

fn required<'tree, 'source>(view: View<'tree, 'source>) -> Option<View<'tree, 'source>> {
    let Parts::Local { bindings, values } = view.parts()? else {
        return None;
    };

    if bindings.clone().count() != 1 || values.clone().count() != 1 {
        return None;
    }

    let Parts::Binding {
        name,
        annotation: None,
    } = bindings.clone().next()?.parts()?
    else {
        return None;
    };

    let Parts::Call { callee, .. } = values.clone().next()?.parts()? else {
        return None;
    };

    (callee.kind() == Kind::Name && callee.text() == "require").then_some(name)
}

fn removed(source: &str, tree: &Tree<'_>, view: View<'_, '_>) -> (Range<usize>, String) {
    let span = view.span();

    let comments = tree
        .tokens
        .iter()
        .filter(|token| {
            token.span.start >= span.start
                && token.span.end <= span.end
                && matches!(
                    token.kind,
                    vermis::TokenKind::Comment | vermis::TokenKind::BlockComment
                )
        })
        .map(|token| &source[token.span.start..token.span.end])
        .collect::<Vec<_>>()
        .join("\n");

    let replacement = if comments.is_empty() {
        comments
    } else {
        format!("{comments}\n")
    };

    let mut end = span.end;

    if source.as_bytes().get(end) == Some(&b';') {
        end += 1;
    }

    let line_start = source[..span.start]
        .rfind('\n')
        .map_or(0, |index| index + 1);

    if source[line_start..span.start].trim().is_empty() {
        let after = &source[end..];

        if let Some(newline) = after.find('\n')
            && after[..newline].trim().is_empty()
        {
            return (line_start..end + newline + 1, replacement);
        }
    }

    (span.start..end, replacement)
}

fn function(
    source: &str,
    tree: &Tree<'_>,
    view: View<'_, '_>,
    names: &Names,
    declaration: Declaration,
) -> Option<(Range<usize>, String)> {
    let Parts::Function {
        name: Some(name), ..
    } = view.parts()?
    else {
        return None;
    };

    let name = match name.parts() {
        Some(Parts::FunctionName { path, method: None }) if path.clone().count() == 1 => {
            path.clone().next().expect("one name")
        }

        Some(Parts::Leaf) if name.kind() == Kind::Name => name,
        _ => return None,
    };

    if !matches!(view.kind(), Kind::LocalFunction | Kind::Function) {
        return None;
    }

    let local = view.kind() == Kind::LocalFunction;

    let mutable = if local {
        names
            .bindings
            .get(&name.span().start)
            .is_none_or(|binding| binding.writes > 0)
    } else {
        names
            .globals
            .get(&name.text().to_string())
            .is_some_and(|count| *count > 1)
    };

    let keyword = match declaration {
        Declaration::Const if !mutable => "const ",
        Declaration::Local => "local ",
        Declaration::Global => "",
        _ => return None,
    };

    let span = view.span();

    let token = tree.tokens.iter().find(|token| {
        token.span.start >= span.start
            && token.span.end <= name.span().start
            && token.kind == vermis::TokenKind::Keyword(vermis::Keyword::Function)
    })?;

    let start = if local {
        tree.tokens
            .iter()
            .rfind(|previous| {
                previous.span.start >= span.start
                    && previous.span.end <= token.span.start
                    && matches!(
                        &source[previous.span.start..previous.span.end],
                        "local" | "const"
                    )
            })
            .map_or(token.span.start, |previous| previous.span.start)
    } else {
        token.span.start
    };

    Some((start..token.span.start, keyword.to_owned()))
}

fn bindings(
    source: &str,
    tree: &Tree<'_>,
    options: &Options,
    held: &[Range<usize>],
) -> io::Result<String> {
    if options.imports.binding == Binding::Preserve
        && !options.bindings.prefer_constant
        && options.imports.unused == Unused::Ignore
        && options.functions.binding == Declaration::Preserve
    {
        return Ok(source.to_owned());
    }

    let root = tree
        .root_view()
        .ok_or_else(|| io::Error::other("missing syntax root"))?;

    let names = Names::resolve(root);

    let top: Vec<_> = root
        .children()
        .flat_map(View::children)
        .map(View::index)
        .collect();

    let mut changes: Vec<(Range<usize>, String)> = Vec::new();

    for index in 0..tree.nodes.len() {
        let view = tree.view(index).expect("node exists");
        let span = view.span();

        if held
            .iter()
            .any(|range| range.start < span.end && range.end > span.start)
        {
            continue;
        }

        if let Some(Parts::Local { bindings, values }) = view.parts() {
            let mutable = bindings.clone().any(|binding| {
                let Some(Parts::Binding { name, .. }) = binding.parts() else {
                    return true;
                };

                names
                    .bindings
                    .get(&name.span().start)
                    .is_none_or(|binding| {
                        binding.writes > 0
                            || options.bindings.preserve_mutated_tables && binding.mutated
                    })
            });

            let mut keyword = None;

            if options.bindings.prefer_constant && values.clone().next().is_some() && !mutable {
                keyword = Some("const");
            }

            if let Some(name) = required(view) {
                let binding = names.bindings.get(&name.span().start);

                match options.imports.binding {
                    Binding::Local => keyword = Some("local"),

                    Binding::Const if binding.is_some_and(|binding| binding.writes == 0) => {
                        keyword = Some("const");
                    }

                    _ => {}
                }

                if !name.text().to_string().starts_with('_')
                    && binding.is_some_and(|binding| binding.reads == 0 && binding.writes == 0)
                {
                    match options.imports.unused {
                        Unused::Underscore => changes.push((
                            name.span().start..name.span().end,
                            format!("_{}", name.text()),
                        )),

                        Unused::Remove if keyword.is_none() => {
                            changes.push(removed(source, tree, view));
                            continue;
                        }

                        _ => {}
                    }
                }
            }

            if let Some(keyword) = keyword {
                let length = if view.kind() == Kind::Constant {
                    "const".len()
                } else {
                    "local".len()
                };

                changes.push((span.start..span.start + length, keyword.to_owned()));
            }
        } else if top.contains(&index)
            && options.functions.binding != Declaration::Preserve
            && let Some(change) = function(source, tree, view, &names, options.functions.binding)
        {
            changes.push(change);
        }
    }

    crate::emit::apply(
        source,
        changes
            .into_iter()
            .map(|(range, text)| crate::emit::Edit { range, text })
            .collect(),
    )
}
