use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
};

use serde::Serialize;
use serde_json::{Map, Value};

use vermis::token::{Keyword, Symbol};

use super::{
    Error, content,
    generation::interrupted,
    mapping::{
        self, Argument, Class, Corrections, Dump, HIERARCHY, Member, Signature, Types, invalid,
    },
    metadata::{Capability, Metadata, Permission, Profile},
    syntax::{self, Output, Replacement},
};

use crate::{
    format::{self, Prepared},
    settings::format::Configuration,
    syntax::is_identifier,
};

struct Context<'context> {
    classes: &'context [Class],
    corrections: &'context Corrections,
    replacements: &'context BTreeMap<String, Vec<Replacement>>,
    labels: &'context [&'context str],
    is_cancelled: &'context dyn Fn() -> bool,
}

pub(super) fn render(
    dump: Value,
    source: &str,
    documentation: Value,
    builtins: &str,
    is_cancelled: &dyn Fn() -> bool,
) -> Result<BTreeMap<PathBuf, String>, Error> {
    interrupted(is_cancelled)?;

    let dump: Dump = mapping::decode(dump, "API dump")?;

    let corrections: Corrections =
        mapping::parse(include_str!("../../assets/corrections.json"), "corrections")?;

    let types: Types = mapping::parse(include_str!("../../assets/types.json"), "types")?;
    let classes = mapping::corrected(dump.classes, &corrections.classes, is_cancelled)?;
    let data_types = mapping::corrected(types.data_types, &corrections.classes, is_cancelled)?;
    let constructors = mapping::corrected(types.constructors, &corrections.classes, is_cancelled)?;
    let declarations = include_str!("../../assets/declarations.d.luau");

    let replacements =
        syntax::replacements(include_str!("../../assets/members.d.luau"), is_cancelled)?
            .into_iter()
            .collect();

    let declared = syntax::blocks(declarations, is_cancelled)?;

    let injections = syntax::injected(
        source,
        &corrections.exclusions.sections,
        &corrections.exclusions.globals,
        &corrections.exports,
        &declared,
        is_cancelled,
    )?;

    let configuration = Prepared::new(&Configuration::default())
        .map_err(|error| invalid(format!("Invalid default formatting settings. {error}")))?;

    let mut definitions = BTreeMap::new();
    let mut profiles = BTreeMap::new();

    for (name, level) in [("none", 0), ("local", 1), ("plugin", 2), ("roblox", 4)] {
        interrupted(is_cancelled)?;
        let labels = &HIERARCHY[..=level];

        let context = Context {
            classes: &classes,
            corrections: &corrections,
            replacements: &replacements,
            labels,
            is_cancelled,
        };

        let mut output = Output::default();
        output.source(declarations);
        output.newline();

        for class in &data_types {
            interrupted(is_cancelled)?;
            output.append(declaration(class, &context)?);
        }

        output.source(&injections);
        output.newline();

        for class in &classes {
            interrupted(is_cancelled)?;
            output.append(declaration(class, &context)?);
        }

        for class in &constructors {
            interrupted(is_cancelled)?;
            output.append(constructor(class, &context)?);
        }

        let ordered = syntax::ordered(&output.finish(is_cancelled)?, is_cancelled)?;
        let formatted = format::format(&ordered, &configuration)?;
        interrupted(is_cancelled)?;
        let mut capabilities = BTreeMap::new();

        for class in &classes {
            interrupted(is_cancelled)?;
            let mut properties = Vec::new();

            for member in &class.members {
                interrupted(is_cancelled)?;

                if member.kind == "Property"
                    && !is_identifier(&member.name)
                    && member.can_access(labels)?
                {
                    let (can_read, can_write) = member.permissions(labels)?;

                    properties.push(Permission {
                        name: member.name.clone(),
                        can_read,
                        can_write,
                    });
                }
            }

            let is_service = mapping::has_tag(class.tags.as_deref(), "Service");

            let is_creatable =
                !is_service && !mapping::has_tag(class.tags.as_deref(), "NotCreatable");

            if is_service || is_creatable || !properties.is_empty() {
                capabilities.insert(
                    class.name.clone(),
                    Capability {
                        name: class.name.clone(),
                        is_service,
                        is_creatable,
                        properties,
                    },
                );
            }
        }

        profiles.insert(
            name.to_owned(),
            Profile {
                classes: capabilities.into_values().collect(),
            },
        );

        definitions.insert(name.to_owned(), formatted);
    }

    let luau_roots: BTreeSet<_> = syntax::blocks(builtins, is_cancelled)?
        .into_iter()
        .map(|block| block.name)
        .collect();

    let mut roblox_roots = BTreeSet::from(["Enum".to_owned()]);

    for definition in definitions.values() {
        for block in syntax::blocks(definition, is_cancelled)? {
            if !block.is_type {
                roblox_roots.insert(block.name);
            }
        }
    }

    let canonical = canonical(documentation, &luau_roots, &roblox_roots, is_cancelled)?;

    let canonical = canonical
        .as_object()
        .ok_or_else(|| invalid("Documentation must be an object."))?;

    let mut result = BTreeMap::new();

    for namespace in ["luau", "roblox"] {
        interrupted(is_cancelled)?;
        let prefix = format!("@{namespace}/");

        let mut symbols = Map::new();

        for (symbol, value) in canonical.iter().filter(|(key, _)| key.starts_with(&prefix)) {
            interrupted(is_cancelled)?;
            symbols.insert(symbol.clone(), content::prepare(value, canonical)?);
        }

        result.insert(
            PathBuf::from(format!("{namespace}/documentation.json")),
            serialize(&symbols, "documentation")?,
        );
    }

    let metadata = Metadata {
        profiles,
        enumerations: dump
            .enumerations
            .into_iter()
            .map(|enumeration| {
                (
                    enumeration.name,
                    enumeration
                        .items
                        .into_iter()
                        .map(|item| item.name)
                        .collect(),
                )
            })
            .collect::<BTreeMap<_, _>>()
            .into(),
    };

    for (name, source) in definitions {
        result.insert(PathBuf::from(format!("roblox/{name}.d.luau")), source);
    }

    result.insert(
        PathBuf::from("roblox/metadata.json"),
        serialize(&metadata, "Roblox metadata")?,
    );

    interrupted(is_cancelled)?;

    Ok(result)
}

fn declaration(class: &Class, context: &Context<'_>) -> Result<Output, Error> {
    let mut output = Output::default();

    if context.corrections.exclusions.types.contains(&class.name) {
        return Ok(output);
    }

    class_header(
        &mut output,
        &class.name,
        class
            .superclass
            .as_deref()
            .filter(|parent| *parent != "<<<ROOT>>>"),
    );

    let replacements = context
        .replacements
        .get(&class.name)
        .map_or(&[][..], Vec::as_slice);

    for (index, member) in class.members.iter().enumerate() {
        interrupted(context.is_cancelled)?;

        if !member.can_access(context.labels)? {
            continue;
        }

        if replacements
            .iter()
            .any(|replacement| replacement.name == member.name)
        {
            if !class.members[..index]
                .iter()
                .any(|previous| previous.name == member.name)
            {
                for replacement in replacements
                    .iter()
                    .filter(|replacement| replacement.name == member.name)
                {
                    if emit_permission(&mut output, Some(member), context.labels)? {
                        output.source(&replacement.source);
                        output.newline();
                    }
                }
            }
        } else if emit_permission(&mut output, Some(member), context.labels)? {
            output.append(signature(
                &member.name,
                member.signature(&context.corrections.types)?,
            ));

            output.newline();
        }
    }

    let mut appended = BTreeSet::new();

    for replacement in replacements {
        interrupted(context.is_cancelled)?;

        if class
            .members
            .iter()
            .any(|member| member.name == replacement.name)
            || !appended.insert(&replacement.name)
        {
            continue;
        }

        let original = inherited(class, &replacement.name, context.classes)?;

        if let Some(member) = original
            && !member.can_access(context.labels)?
        {
            continue;
        }

        for replacement in replacements
            .iter()
            .filter(|entry| entry.name == replacement.name)
        {
            if emit_permission(&mut output, original, context.labels)? {
                output.source(&replacement.source);
                output.newline();
            }
        }
    }

    output.keyword(Keyword::End);
    output.newline();
    output.newline();

    Ok(output)
}

fn emit_permission(
    output: &mut Output,
    member: Option<&Member>,
    labels: &[&str],
) -> Result<bool, Error> {
    let Some(member) = member.filter(|member| member.kind == "Property") else {
        return Ok(true);
    };

    let (can_read, can_write) = member.permissions(labels)?;

    let is_allowed =
        if (can_read || can_write) && !is_identifier(&member.name) || (can_read && can_write) {
            true
        } else if can_read {
            output.source("read");

            true
        } else if can_write {
            output.source("write");

            true
        } else {
            false
        };

    Ok(is_allowed)
}

fn signature(name: &str, signature: &Signature) -> Output {
    let mut output = Output::default();

    match signature {
        Signature::Property { value } => {
            output.property(name);
            output.symbol(Symbol::Colon);
            output.source(value);
        }

        Signature::Function {
            parameters: values,
            returns: result,
        } => {
            output.keyword(Keyword::Function);
            output.property(name);
            output.symbol(Symbol::LeftParenthesis);
            output.source("self");

            if !values.is_empty() {
                output.symbol(Symbol::Comma);
            }

            parameters(&mut output, values, false);
            output.symbol(Symbol::RightParenthesis);
            output.symbol(Symbol::Colon);
            returns(&mut output, result);
        }

        Signature::Callback {
            parameters: values,
            returns: result,
        } => {
            output.property(name);
            output.symbol(Symbol::Colon);
            output.symbol(Symbol::LeftParenthesis);
            parameters(&mut output, values, true);
            output.symbol(Symbol::RightParenthesis);
            output.symbol(Symbol::Arrow);
            returns(&mut output, result);
        }

        Signature::Event { arguments } => {
            output.property(name);
            output.symbol(Symbol::Colon);
            output.source("RBXScriptSignal");
            output.symbol(Symbol::LessThan);
            returns(&mut output, arguments);
            output.symbol(Symbol::GreaterThan);
        }
    }

    output
}

fn parameters(output: &mut Output, values: &[Argument], is_callback: bool) {
    for (index, parameter) in values.iter().enumerate() {
        if index > 0 {
            output.symbol(Symbol::Comma);
        }

        let value = parameter.value.as_str();
        let is_variadic = value.starts_with("...") || parameter.name == "...";

        if is_variadic && index + 1 == values.len() {
            output.symbol(Symbol::Ellipsis);

            if !is_callback {
                output.symbol(Symbol::Colon);
            }

            output.source(value.strip_prefix("...").unwrap_or(value));
            continue;
        }

        if is_identifier(&parameter.name) && parameter.name != "self" {
            output.source(&parameter.name);
        } else {
            output.source(format!("argument{index}"));
        }

        output.symbol(Symbol::Colon);

        let value = if is_variadic { "any" } else { value };

        output.source(value);

        if parameter.has_default && !value.ends_with('?') {
            output.symbol(Symbol::QuestionMark);
        }
    }
}

fn returns(output: &mut Output, values: &[String]) {
    if values.len() != 1 {
        output.symbol(Symbol::LeftParenthesis);
    }

    for (index, value) in values.iter().enumerate() {
        if index > 0 {
            output.symbol(Symbol::Comma);
        }

        output.source(if value.starts_with("...") && index + 1 < values.len() {
            "any"
        } else {
            value.as_str()
        });
    }

    if values.len() != 1 {
        output.symbol(Symbol::RightParenthesis);
    }
}

fn constructor(class: &Class, context: &Context<'_>) -> Result<Output, Error> {
    let mut output = Output::default();

    if context.corrections.exclusions.types.contains(&class.name) {
        return Ok(output);
    }

    output.source("declare");
    output.source(&class.name);
    output.symbol(Symbol::Colon);
    output.symbol(Symbol::LeftBrace);
    output.newline();
    let mut fields = 0;

    for member in &class.members {
        interrupted(context.is_cancelled)?;

        if member.kind == "Property" {
            if fields > 0 {
                output.symbol(Symbol::Comma);
                output.newline();
            }

            fields += 1;

            output.append(signature(
                &member.name,
                member.signature(&context.corrections.types)?,
            ));
        }
    }

    let mut emitted = BTreeSet::new();

    for member in &class.members {
        interrupted(context.is_cancelled)?;

        if member.kind != "Function" || !emitted.insert(&member.name) {
            continue;
        }

        if fields > 0 {
            output.symbol(Symbol::Comma);
            output.newline();
        }

        fields += 1;
        output.property(&member.name);
        output.symbol(Symbol::Colon);

        for (index, overload) in class
            .members
            .iter()
            .filter(|value| value.kind == "Function" && value.name == member.name)
            .enumerate()
        {
            interrupted(context.is_cancelled)?;

            if index > 0 {
                output.symbol(Symbol::Ampersand);
            }

            output.symbol(Symbol::LeftParenthesis);
            output.symbol(Symbol::LeftParenthesis);

            let Signature::Function {
                parameters: values,
                returns: result,
            } = overload.signature(&context.corrections.types)?
            else {
                return Err(invalid("Invalid constructor function signature."));
            };

            parameters(&mut output, values, true);

            output.symbol(Symbol::RightParenthesis);
            output.symbol(Symbol::Arrow);

            returns(&mut output, result);

            output.symbol(Symbol::RightParenthesis);
        }
    }

    output.newline();
    output.symbol(Symbol::RightBrace);
    output.newline();
    output.newline();

    Ok(output)
}

fn inherited<'classes>(
    class: &'classes Class,
    name: &str,
    classes: &'classes [Class],
) -> Result<Option<&'classes Member>, Error> {
    let mut current = class;
    let mut visited = BTreeSet::new();

    loop {
        if let Some(member) = current.members.iter().find(|member| member.name == name) {
            return Ok(Some(member));
        }

        if !visited.insert(&current.name) {
            return Err(invalid(format!(
                "Cyclic class inheritance at `{}`.",
                current.name
            )));
        }

        let Some(parent) = current
            .superclass
            .as_deref()
            .and_then(|parent| classes.iter().find(|class| class.name == parent))
        else {
            return Ok(None);
        };

        current = parent;
    }
}

fn class_header(output: &mut Output, name: &str, parent: Option<&str>) {
    output.source("declare");
    output.source("extern");
    output.source("type");
    output.source(name);

    if let Some(parent) = parent {
        output.source("extends");
        output.source(parent);
    }

    output.source("with");
    output.newline();
}

fn canonical(
    value: Value,
    luau: &BTreeSet<String>,
    roblox: &BTreeSet<String>,
    is_cancelled: &dyn Fn() -> bool,
) -> Result<Value, Error> {
    interrupted(is_cancelled)?;

    match value {
        Value::Object(values) => {
            let keys: BTreeSet<_> = values.keys().cloned().collect();
            let mut result = Map::new();

            for (key, value) in values {
                interrupted(is_cancelled)?;
                let replacement = canonical_symbol(&key, luau, roblox);

                if replacement != key && keys.contains(&replacement) {
                    continue;
                }

                result.insert(replacement, canonical(value, luau, roblox, is_cancelled)?);
            }

            Ok(Value::Object(result))
        }

        Value::Array(values) => {
            let values: Result<Vec<_>, _> = values
                .into_iter()
                .map(|value| canonical(value, luau, roblox, is_cancelled))
                .collect();

            Ok(Value::Array(values?))
        }

        Value::String(value) => Ok(Value::String(canonical_symbol(&value, luau, roblox))),
        value => Ok(value),
    }
}

fn canonical_symbol(value: &str, luau: &BTreeSet<String>, roblox: &BTreeSet<String>) -> String {
    if let Some(symbol) = value.strip_prefix("@roblox/global/") {
        let boundary = symbol.find(['.', '/']).unwrap_or(symbol.len());
        let (root, suffix) = symbol.split_at(boundary);

        if is_identifier(root) && luau.contains(root) && !roblox.contains(root) {
            return format!("@luau/global/{root}{suffix}");
        }
    }

    value.to_owned()
}

fn serialize(value: &impl Serialize, input: &str) -> Result<String, Error> {
    serde_json::to_string(value).map_err(|source| Error::Data {
        input: input.to_owned(),
        source,
    })
}
