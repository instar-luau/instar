use super::{Context, identifier, replace_keep_lines};

use crate::build::{
    configuration::Rules,
    mapping::Edit,
    syntax::{array, field, kind, nodes, range, unwrap},
};

use serde_json::Value;

use std::{
    collections::{BTreeMap, BTreeSet},
    io,
    ops::Range,
};

use vermis::{Kind, Parts};

pub(super) fn apply(
    context: &Context<'_, '_, '_, '_, '_>,
    settings: &Rules,
    edits: &mut Vec<Edit>,
) -> io::Result<()> {
    let (reads, writes) = if settings.const_requires || settings.remove_unused_variable {
        uses(context.document)
    } else {
        (BTreeMap::new(), BTreeSet::new())
    };

    if settings.const_requires {
        const_requires(context, &writes, edits)?;
    }

    if settings.remove_unused_variable {
        remove_unused(context, &reads, edits)?;
    }

    if settings.rename_variables {
        rename(context, edits)?;
    }

    Ok(())
}

fn uses(document: &Value) -> (BTreeMap<String, usize>, BTreeSet<String>) {
    let mut reads = BTreeMap::<String, usize>::new();
    let mut writes = BTreeSet::new();

    for node in nodes(&document["root"]) {
        if kind(node) == "AstExprLocal" {
            *reads
                .entry(field(&node["local"], "location").to_owned())
                .or_default() += 1;
        } else if kind(node) == "AstTypeReference"
            && let Some(identity) = node["prefixLocal"]["location"].as_str()
        {
            *reads.entry(identity.to_owned()).or_default() += 1;
        }

        let targets = match kind(node) {
            "AstStatAssign" => array(&node["vars"]).iter().collect::<Vec<_>>(),
            "AstStatCompoundAssign" => vec![&node["var"]],
            _ => Vec::new(),
        };

        for target in targets {
            if kind(target) == "AstExprLocal" {
                writes.insert(field(&target["local"], "location").to_owned());
            }
        }
    }

    (reads, writes)
}

fn const_requires(
    context: &Context<'_, '_, '_, '_, '_>,
    writes: &BTreeSet<String>,
    edits: &mut Vec<Edit>,
) -> io::Result<()> {
    for node in nodes(&context.document["root"]) {
        if kind(node) != "AstStatLocal" {
            continue;
        }

        let bindings = array(&node["vars"]);
        let values = array(&node["values"]);
        let value = values.first().map(unwrap);

        if bindings.len() != 1
            || values.len() != 1
            || value.is_none_or(|value| kind(value) != "AstExprCall")
            || value.is_none_or(|value| kind(&value["func"]) != "AstExprGlobal")
            || value.is_none_or(|value| field(&value["func"], "global") != "require")
            || writes.contains(field(&bindings[0], "location"))
            || !bindings[0]["annotation"].is_null()
        {
            continue;
        }

        let statement = range(context.model, node)?;

        if static_require(context, &statement)
            && context.text.get(statement.start..statement.start + 5) == Some("local")
        {
            edits.push(Edit {
                range: statement.start..statement.start + 5,
                text: "const".into(),
            });
        }
    }

    Ok(())
}

fn static_require(context: &Context<'_, '_, '_, '_, '_>, statement: &Range<usize>) -> bool {
    context.tree.nodes.iter().enumerate().any(|(index, node)| {
        if node.kind != Kind::Local
            || node.span.start != statement.start
            || node.span.end != statement.end
        {
            return false;
        }

        let Some(Parts::Local {
            mut bindings,
            mut values,
        }) = context.tree.view(index).and_then(vermis::View::parts)
        else {
            return false;
        };

        let (Some(_), Some(value)) = (bindings.next(), values.next()) else {
            return false;
        };

        if bindings.next().is_some() || values.next().is_some() {
            return false;
        }

        let Some(Parts::Call { callee, arguments }) = value.parts() else {
            return false;
        };

        let Some(Parts::Arguments { mut values }) = arguments.parts() else {
            return false;
        };

        callee.kind() == Kind::Name
            && super::text(callee) == "require"
            && values
                .next()
                .is_some_and(|argument| argument.kind() == Kind::String)
            && values.next().is_none()
    })
}

fn inert(value: &Value) -> bool {
    match kind(value) {
        "AstExprGroup" | "AstExprTypeAssertion" | "AstExprUnary" => inert(&value["expr"]),
        "AstExprBinary" => inert(&value["left"]) && inert(&value["right"]),

        "AstExprTable" => array(&value["items"])
            .iter()
            .all(|item| inert(&item["key"]) && inert(&item["value"])),

        "AstExprConstantNil"
        | "AstExprConstantBool"
        | "AstExprConstantNumber"
        | "AstExprConstantString"
        | "AstExprFunction"
        | "" => true,

        _ => false,
    }
}

fn remove_unused(
    context: &Context<'_, '_, '_, '_, '_>,
    reads: &BTreeMap<String, usize>,
    edits: &mut Vec<Edit>,
) -> io::Result<()> {
    for node in nodes(&context.document["root"]) {
        let removable = match kind(node) {
            "AstStatLocal" => {
                let bindings = array(&node["vars"]);

                !bindings.is_empty()
                    && bindings.iter().all(|binding| {
                        reads
                            .get(field(binding, "location"))
                            .copied()
                            .unwrap_or_default()
                            == 0
                    })
                    && array(&node["values"]).iter().all(inert)
            }

            "AstStatLocalFunction" => {
                reads
                    .get(field(&node["name"], "location"))
                    .copied()
                    .unwrap_or_default()
                    == 0
            }

            _ => false,
        };

        if removable {
            replace_keep_lines(context.text, range(context.model, node)?, "", edits);
        }
    }

    Ok(())
}

fn rename(context: &Context<'_, '_, '_, '_, '_>, edits: &mut Vec<Edit>) -> io::Result<()> {
    let syntax_nodes = nodes(&context.document["root"]);
    let mut taken = BTreeSet::new();
    let mut type_uses = BTreeSet::new();

    for node in &syntax_nodes {
        match kind(node) {
            "AstExprGlobal" => {
                taken.insert(field(node, "global").to_owned());
            }

            "AstLocal" => {
                taken.insert(field(node, "name").to_owned());
            }

            "AstTypeReference" => {
                if let Some(identity) = node["prefixLocal"]["location"].as_str() {
                    type_uses.insert(identity.to_owned());
                }
            }

            _ => {}
        }
    }

    let mut replacements = BTreeMap::new();
    let mut counter = 0;

    for node in &syntax_nodes {
        if kind(node) != "AstLocal" {
            continue;
        }

        let identity = field(node, "location");
        let original = field(node, "name");

        if original == "self" || type_uses.contains(identity) || replacements.contains_key(identity)
        {
            continue;
        }

        let replacement = loop {
            let candidate = short_name(counter);
            counter += 1;

            if identifier(&candidate) && !taken.contains(&candidate) {
                break candidate;
            }
        };

        taken.insert(replacement.clone());
        replacements.insert(identity.to_owned(), replacement);
    }

    for node in syntax_nodes {
        let replacement = match kind(node) {
            "AstLocal" => replacements.get(field(node, "location")),
            "AstExprLocal" => replacements.get(field(&node["local"], "location")),
            _ => None,
        };

        if let Some(replacement) = replacement {
            edits.push(Edit {
                range: range(context.model, node)?,
                text: replacement.clone(),
            });
        }
    }

    Ok(())
}

fn short_name(mut index: usize) -> String {
    const ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ";
    let mut name = String::new();

    loop {
        name.push(char::from(ALPHABET[index % ALPHABET.len()]));
        index /= ALPHABET.len();

        if index == 0 {
            return name;
        }

        index -= 1;
    }
}
