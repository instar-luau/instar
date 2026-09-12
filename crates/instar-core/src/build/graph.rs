use super::{
    Diagnostic,
    configuration::{Shape, Target},
    mapping::{Edit, Text},
    paths,
    syntax::{array, field, kind, nodes, quote, range, unwrap},
};
use crate::{
    roblox::Environment,
    source::{PositionEncoding, Source},
};
use serde::Serialize;
use serde_json::Value;
use std::{
    collections::BTreeMap,
    io,
    ops::Range,
    path::{Path, PathBuf},
    sync::Arc,
};

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Classification {
    Internal,
    External,
    Dynamic,
    Unresolved,
}

#[derive(Clone, Debug, Serialize)]
pub struct Dependency {
    pub classification: Classification,
    pub specifier: Option<String>,
    pub target: Option<PathBuf>,
    pub start: usize,
    pub end: usize,

    #[serde(skip)]
    pub(super) callee: Range<usize>,

    #[serde(skip)]
    pub(super) argument: Range<usize>,
}

pub(super) struct Module {
    pub source: Arc<Source>,
    pub text: Text,
    pub document: Value,
    pub dependencies: Vec<Dependency>,
}

pub(super) fn dependencies(
    module: &Module,
    environment: &Environment,
    external: &[String],
) -> io::Result<Vec<Dependency>> {
    let mut result = Vec::new();

    for value in nodes(&module.document["root"]) {
        if kind(value) != "AstExprCall"
            || kind(unwrap(&value["func"])) != "AstExprGlobal"
            || field(unwrap(&value["func"]), "global") != "require"
        {
            continue;
        }

        let arguments = array(&value["args"]);

        if arguments.len() != 1 {
            return Err(io::Error::other(
                "build requires must have exactly one argument",
            ));
        }

        let argument = unwrap(&arguments[0]);

        let specifier = if kind(argument) == "AstExprConstantString" {
            argument["value"].as_str().map(str::to_owned)
        } else {
            None
        };

        let target = resolved(module, &arguments[0])?;

        let (classification, target) = if specifier
            .as_ref()
            .is_some_and(|specifier| external.contains(specifier))
            || (kind(argument) == "AstExprConstantNumber" && environment.enabled)
        {
            (Classification::External, None)
        } else if target.is_some() {
            (Classification::Internal, target)
        } else if specifier.is_some() {
            (Classification::Unresolved, None)
        } else {
            (Classification::Dynamic, None)
        };

        let location = range(&module.source, value)?;

        result.push(Dependency {
            classification,
            specifier,
            target,
            start: location.start,
            end: location.end,
            callee: range(&module.source, &value["func"])?,
            argument: range(&module.source, &arguments[0])?,
        });
    }

    result.sort_by_key(|dependency| dependency.start);

    Ok(result)
}

fn resolved(module: &Module, argument: &Value) -> io::Result<Option<PathBuf>> {
    let argument = range(&module.source, argument)?;

    for dependency in array(&module.document["dependencies"]) {
        let coordinates: [u32; 4] =
            serde_json::from_value(dependency["range"].clone()).map_err(io::Error::other)?;

        let start = usize::from(
            module
                .source
                .offset(
                    line_index::LineCol {
                        line: coordinates[0],
                        col: coordinates[1],
                    },
                    PositionEncoding::Utf8,
                )
                .map_err(io::Error::other)?,
        );

        if start == argument.start {
            return Ok(Some(PathBuf::from(field(dependency, "path"))));
        }
    }

    Ok(None)
}

pub(super) fn validate(
    module: &Module,
    environment: &Environment,
    shape: Shape,
    target: Target,
) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();

    for dependency in &module.dependencies {
        let message = match dependency.classification {
            Classification::Dynamic => Some((
                shape == Shape::Bundle,
                "dynamic require remains external and cannot be bundled".to_owned(),
            )),

            Classification::Unresolved => Some((
                true,
                format!(
                    "unresolved require: {}",
                    dependency.specifier.as_deref().unwrap_or_default()
                ),
            )),

            Classification::Internal => dependency
                .target
                .as_ref()
                .and_then(|destination| {
                    validate_runtime(environment, module.source.path(), destination, target).err()
                })
                .map(|error| (true, error.to_string())),

            Classification::External => None,
        };

        if let Some((error, message)) = message {
            diagnostics.push(Diagnostic {
                path: module.source.path().to_owned(),
                start: dependency.start,
                end: dependency.end,
                error,
                message,
            });
        }
    }

    if shape == Shape::Bundle {
        for value in nodes(&module.document["root"]) {
            if kind(value) != "AstExprGlobal" {
                continue;
            }

            let name = field(value, "global");

            let Ok(location) = range(&module.source, value) else {
                continue;
            };

            let internal = module.dependencies.iter().any(|dependency| {
                dependency.classification == Classification::Internal
                    && dependency.start <= location.start
                    && location.end <= dependency.end
            });

            if (name == "script" && !internal)
                || matches!(name, "getfenv" | "setfenv" | "loadstring")
                || (name == "require"
                    && !module.dependencies.iter().any(|dependency| {
                        dependency.callee.start <= location.start
                            && location.end <= dependency.callee.end
                    }))
            {
                diagnostics.push(Diagnostic { path: module.source.path().to_owned(), start: location.start, end: location.end, error: true, message: format!("bundle cannot preserve {name} identity or environment; use directory output") });
            }
        }
    }

    diagnostics
}

fn ancestry(environment: &Environment, path: &Path) -> Option<Vec<usize>> {
    let mut current = environment.node(path)?;
    let mut result = vec![current];

    while let Some(parent) = environment.nodes[current].parent {
        current = parent;
        result.push(current);
    }

    result.reverse();

    Some(result)
}

fn clone_root(environment: &Environment, ancestors: &[usize]) -> Option<usize> {
    ancestors.iter().copied().find(|index| {
        let node = &environment.nodes[*index];

        matches!(
            node.class_name.as_str(),
            "StarterPlayerScripts" | "StarterCharacterScripts" | "StarterGui" | "StarterPack"
        ) || node.parent.is_some_and(|parent| {
            (environment.nodes[parent].class_name == "DataModel"
                && matches!(node.name.as_str(), "StarterGui" | "StarterPack"))
                || (environment.nodes[parent].name == "StarterPlayer"
                    && matches!(
                        node.name.as_str(),
                        "StarterPlayerScripts" | "StarterCharacterScripts"
                    ))
        })
    })
}

fn server(environment: &Environment, ancestors: &[usize]) -> bool {
    ancestors.iter().any(|index| {
        let node = &environment.nodes[*index];

        matches!(
            node.class_name.as_str(),
            "ServerScriptService" | "ServerStorage"
        ) || (node.parent == Some(0)
            && matches!(node.name.as_str(), "ServerScriptService" | "ServerStorage"))
    })
}

fn validate_runtime(
    environment: &Environment,
    from: &Path,
    to: &Path,
    target: Target,
) -> io::Result<()> {
    if target == Target::Path {
        return Ok(());
    }

    let from = ancestry(environment, from)
        .ok_or_else(|| io::Error::other("Roblox build source has no instance mapping"))?;

    let to = ancestry(environment, to)
        .ok_or_else(|| io::Error::other("Roblox require target has no instance mapping"))?;

    if environment.nodes[*to
        .last()
        .ok_or_else(|| io::Error::other("missing instance"))?]
    .class_name
        != "ModuleScript"
    {
        return Err(io::Error::other("require target is not a ModuleScript"));
    }

    let source_server = server(environment, &from);
    let target_server = server(environment, &to);

    if !source_server && target_server {
        return Err(io::Error::other(
            "require from a client-accessible location reaches a server-only module",
        ));
    }

    if clone_root(environment, &to).is_some()
        && clone_root(environment, &to) != clone_root(environment, &from)
    {
        return Err(io::Error::other(
            "require reaches a cloned template rather than the executing player's module",
        ));
    }

    Ok(())
}

pub(super) fn rewrite(
    module: &Module,
    destinations: &BTreeMap<PathBuf, PathBuf>,
    environment: &Environment,
    target: Target,
    bundle: Option<(&str, &Path)>,
) -> io::Result<Text> {
    let mut edits = Vec::new();

    for dependency in &module.dependencies {
        let Some(destination) = &dependency.target else {
            continue;
        };

        if dependency.classification != Classification::Internal {
            continue;
        }

        let replacement = if let Some((prefix, root)) = bundle {
            edits.push(Edit {
                range: dependency.callee.clone(),
                text: format!("{prefix}require"),
            });

            quote(
                &destination
                    .strip_prefix(root)
                    .map_err(io::Error::other)?
                    .to_string_lossy()
                    .replace('\\', "/"),
            )
        } else if target == Target::Path {
            quote(&paths::specifier(
                destinations
                    .get(module.source.path())
                    .ok_or_else(|| io::Error::other("source destination missing"))?,
                destinations
                    .get(destination)
                    .ok_or_else(|| io::Error::other("dependency destination missing"))?,
            )?)
        } else {
            roblox_path(environment, module.source.path(), destination, target)?
        };

        let original = &module.text.text[dependency.argument.clone()];

        let comments = vermis::tokenize(original.as_bytes().into())
            .iter()
            .filter(|token| {
                matches!(
                    token.kind,
                    vermis::TokenKind::Comment | vermis::TokenKind::BlockComment
                )
            })
            .map(|token| &original[token.span.start..token.span.end])
            .collect::<Vec<_>>();

        let replacement = if comments.is_empty() {
            replacement
        } else {
            format!("\n{}\n{replacement}", comments.join("\n"))
        };

        edits.push(Edit {
            range: dependency.argument.clone(),
            text: replacement,
        });
    }

    module.text.edit(edits)
}

fn roblox_path(
    environment: &Environment,
    from: &Path,
    to: &Path,
    target: Target,
) -> io::Result<String> {
    validate_runtime(environment, from, to, target)?;

    let from =
        ancestry(environment, from).ok_or_else(|| io::Error::other("source mapping missing"))?;

    let to = ancestry(environment, to).ok_or_else(|| io::Error::other("target mapping missing"))?;

    let common = from
        .iter()
        .zip(&to)
        .take_while(|(left, right)| left == right)
        .count();

    let relative = clone_root(environment, &to).is_some();

    if target == Target::RobloxString {
        let names = if relative {
            let mut names = vec!["..".to_owned(); from.len() - common];

            names.extend(
                to[common..]
                    .iter()
                    .map(|index| environment.nodes[*index].name.clone()),
            );

            names
        } else {
            to.iter()
                .skip(1)
                .map(|index| environment.nodes[*index].name.clone())
                .collect()
        };

        if names.iter().any(|name| {
            name.contains('/') || name.contains('\\') || name == "." || (name == ".." && !relative)
        }) {
            return Err(io::Error::other(
                "instance names cannot be represented by a require string",
            ));
        }

        return Ok(quote(&format!(
            "{}/{}",
            if relative { "@self" } else { "@game" },
            names.join("/")
        )));
    }

    let mut result = if relative {
        format!("script{}", ".Parent".repeat(from.len() - common))
    } else {
        "game".into()
    };

    for index in if relative { &to[common..] } else { &to[1..] } {
        result.push_str(":FindFirstChild(");
        result.push_str(&quote(&environment.nodes[*index].name));
        result.push(')');
    }

    Ok(result)
}
