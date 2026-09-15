mod assign;
mod attributes;
mod calls;
mod comments;
mod control;
mod expressions;
mod functions;
mod locals;
mod native;

use super::{
    configuration::Rules,
    mapping::{Edit, Text},
};

use crate::{project::roblox::Environment, source::Source};
use serde_json::Value;

use std::{
    collections::BTreeMap,
    io,
    ops::Range,
    path::{Path, PathBuf},
};

use vermis::{Kind, Parts, Tree, View};

#[derive(Clone, Copy)]
pub(super) struct Project<'project> {
    pub path: &'project Path,
    pub root: &'project Path,
    pub environment: &'project Environment,
    pub files: &'project BTreeMap<PathBuf, Vec<u8>>,
}

pub(super) struct Context<'tree, 'source, 'model, 'document, 'project> {
    pub text: &'source str,
    pub tree: &'tree Tree<'source>,
    pub model: &'model Source,
    pub document: &'document Value,
    pub path: &'project Path,
    pub root: &'project Path,
    pub environment: &'project Environment,
    pub files: &'project BTreeMap<PathBuf, Vec<u8>>,
    pub parents: Vec<Option<usize>>,
}

pub(super) fn apply(
    text: &Text,
    model: &Source,
    document: &Value,
    settings: &Rules,
    project: Project<'_>,
) -> io::Result<Text> {
    let tree = vermis::parse(text.text.as_bytes().into());

    if let Some(error) = tree.diagnostics.first() {
        return Err(io::Error::other(format!(
            "syntax at byte {}: {}",
            error.span.start, error.message
        )));
    }

    let context = Context {
        text: &text.text,
        tree: &tree,
        model,
        document,
        path: project.path,
        root: project.root,
        environment: project.environment,
        files: project.files,
        parents: parents(&tree),
    };

    let mut edits = Vec::new();

    comments::apply(&context, settings, &mut edits)?;
    attributes::apply(&context, settings, &mut edits)?;
    functions::apply(&context, settings, &mut edits);
    assign::apply(&context, settings, &mut edits);
    expressions::apply(&context, settings, &mut edits);
    calls::apply(&context, settings, &mut edits);
    control::apply(&context, settings, &mut edits);
    locals::apply(&context, settings, &mut edits)?;
    native::apply(&context, settings, &mut edits);

    let transformed = text.edit(nonoverlapping(edits))?;

    let Some(interpolation) = settings
        .remove_interpolated_string
        .as_ref()
        .filter(|interpolation| interpolation.enabled())
    else {
        return Ok(transformed);
    };

    let tree = vermis::parse(transformed.text.as_bytes().into());
    let mut edits = Vec::new();

    for index in 0..tree.nodes.len() {
        if let Some(view) = tree.view(index)
            && view.kind() == Kind::Interpolation
        {
            expressions::interpolate(view, interpolation.strategy(), &mut edits);
        }
    }

    transformed.edit(nonoverlapping(edits))
}

pub(super) fn nonoverlapping(mut edits: Vec<Edit>) -> Vec<Edit> {
    edits.sort_by_key(|edit| (edit.range.start, edit.range.end));

    let mut previous = 0;

    edits.retain(|edit| {
        if edit.range.start < previous {
            false
        } else {
            previous = edit.range.end;

            true
        }
    });

    edits
}

pub(super) fn parents(tree: &Tree<'_>) -> Vec<Option<usize>> {
    let mut parents = vec![None; tree.nodes.len()];

    for (parent, node) in tree.nodes.iter().enumerate() {
        for child in tree.children.get(node.children.clone()).unwrap_or(&[]) {
            if let Some(slot) = parents.get_mut(*child) {
                *slot = Some(parent);
            }
        }
    }

    parents
}

pub(super) fn span(view: View<'_, '_>) -> Range<usize> {
    view.span().start..view.span().end
}

pub(super) fn text<'source>(view: View<'_, 'source>) -> &'source str {
    std::str::from_utf8(view.text().as_ref()).unwrap_or_default()
}

pub(super) fn replace_keep_lines(
    source: &str,
    range: Range<usize>,
    replacement: &str,
    edits: &mut Vec<Edit>,
) {
    let removed = source[range.clone()]
        .bytes()
        .filter(|byte| *byte == b'\n')
        .count();

    let added = replacement.bytes().filter(|byte| *byte == b'\n').count();

    let mut replacement = replacement.to_owned();

    if removed > added {
        replacement.push_str(&"\n".repeat(removed - added));
    }

    edits.push(Edit {
        range,
        text: replacement,
    });
}

pub(super) fn identifier(value: &str) -> bool {
    let mut bytes = value.bytes();

    let Some(first) = bytes.next() else {
        return false;
    };

    (first.is_ascii_alphabetic() || first == b'_')
        && bytes.all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
        && !matches!(
            value,
            "and"
                | "break"
                | "do"
                | "else"
                | "elseif"
                | "end"
                | "false"
                | "for"
                | "function"
                | "if"
                | "in"
                | "local"
                | "nil"
                | "not"
                | "or"
                | "repeat"
                | "return"
                | "then"
                | "true"
                | "until"
                | "while"
        )
}

pub(super) fn plain_string<'source>(view: View<'_, 'source>) -> Option<&'source str> {
    let value = text(view);
    let quote = value.as_bytes().first().copied()?;

    if !matches!(quote, b'\'' | b'"') || value.as_bytes().last().copied() != Some(quote) {
        return None;
    }

    let value = &value[1..value.len() - 1];

    (!value.contains('\\')).then_some(value)
}

pub(super) fn atomic(view: View<'_, '_>) -> bool {
    matches!(
        view.kind(),
        Kind::Name
            | Kind::Number
            | Kind::String
            | Kind::Boolean
            | Kind::Nil
            | Kind::Field
            | Kind::Index
            | Kind::Call
            | Kind::MethodCall
            | Kind::Group
    )
}

pub(super) fn reemittable(view: View<'_, '_>) -> bool {
    match view.kind() {
        Kind::Name | Kind::Number | Kind::String => true,

        Kind::Field => {
            matches!(view.parts(), Some(Parts::Field { receiver, .. }) if reemittable(receiver))
        }

        Kind::Index => {
            matches!(view.parts(), Some(Parts::Index { receiver, key }) if reemittable(receiver) && matches!(key.kind(), Kind::Name | Kind::Number | Kind::String))
        }

        Kind::Group => {
            matches!(view.parts(), Some(Parts::Group { expression }) if reemittable(expression))
        }

        _ => false,
    }
}

pub(super) fn pure(view: View<'_, '_>) -> bool {
    match view.parts() {
        Some(Parts::Unary { operand, .. }) => pure(operand),
        Some(Parts::Binary { left, right, .. }) => pure(left) && pure(right),
        Some(Parts::Group { expression }) => pure(expression),
        Some(Parts::Table { fields }) => fields.into_iter().all(pure),
        Some(Parts::TableField { key, value, .. }) => key.is_none_or(pure) && pure(value),

        _ => matches!(
            view.kind(),
            Kind::Name | Kind::Number | Kind::String | Kind::Boolean | Kind::Nil | Kind::Function
        ),
    }
}
