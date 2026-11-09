use std::{collections::BTreeMap, ops::Range, path::Path};

use instar_core::{
    analysis::{self, is_identifier},
    format::Insertion,
    lint::Symbols,
    workspace::{ImportStyle, Purpose},
};

use serde::{Deserialize, Serialize};

use tower_lsp_server::{
    gen_lsp_types::{
        CompletionItem, CompletionItemKind, CompletionItemLabelDetails, InsertTextFormat, Position,
        TextEdit,
    },
    jsonrpc::Result,
};

use crate::{configuration::Require, document::Document, error::failure, project::Project};

use super::completion::{can_resolve, text_edit, word};

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) enum Import {
    Module(String),
    Service(String),
    Member { module: String, name: String },
    Type { module: String, name: String },
}

pub(super) struct ImportContext<'source> {
    pub(super) document: &'source Document,
    pub(super) position: Position,
    pub(super) symbols: &'source Symbols,
    pub(super) insertion: Insertion<'source>,
    pub(super) is_resolved: bool,
    pub(super) imports: BTreeMap<String, Option<String>>,
}

impl Import {
    pub(super) fn name(&self) -> Option<&str> {
        let name = match self {
            Self::Module(target) => {
                let path = Path::new(target);

                path.file_stem()
                    .and_then(|name| name.to_str())
                    .filter(|name| *name != "init")
                    .or_else(|| {
                        path.parent()
                            .and_then(Path::file_name)
                            .and_then(|name| name.to_str())
                    })
            }

            Self::Service(name) | Self::Member { name, .. } | Self::Type { name, .. } => {
                Some(name.as_str())
            }
        };

        name.filter(|name| is_identifier(name))
    }
}

pub(super) fn imported(
    project: &mut Project,
    module: &str,
    target: &Import,
    quality: u8,
    context: &ImportContext<'_>,
) -> Result<Option<CompletionItem>> {
    let &ImportContext {
        document,
        position,
        symbols,
        ref insertion,
        is_resolved,
        ref imports,
    } = context;

    let Some(name) = target.name() else {
        return Ok(None);
    };

    let is_type = matches!(target, Import::Type { .. });

    if !project.configuration.imports.is_enabled
        || if is_type {
            symbols.types.contains(name) || insertion.is_type_bound(name)
        } else {
            symbols.globals.contains(name) || insertion.is_bound(name)
        }
    {
        return Ok(None);
    }

    let offset = document.offset(position, project.encoding)?;
    let replacement = word(&document.text, offset);
    let is_combined = insertion.range.is_empty() && insertion.range.start == replacement.start;
    let should_defer = !is_resolved && !is_combined && can_resolve(project, "additionalTextEdits");

    let binding = match target {
        Import::Member { module, .. } | Import::Type { module, .. } => {
            imports.get(module).and_then(Option::as_deref)
        }

        Import::Module(_) | Import::Service(_) => None,
    };

    let expression = match target {
        Import::Module(target)
        | Import::Member { module: target, .. }
        | Import::Type { module: target, .. } => {
            if target == module
                || !project
                    .workspace
                    .imports(target)
                    .map_err(failure)?
                    .can_import(Path::new(target))
                || project
                    .workspace
                    .is_excluded(target, Purpose::Index)
                    .map_err(failure)?
                || binding.is_none() && insertion.is_bound("require")
            {
                return Ok(None);
            }

            if let Some(binding) = binding {
                binding.to_owned()
            } else {
                let is_instance = project.configuration.imports.require == Require::Instance;

                let style = if is_instance {
                    ImportStyle::Instance
                } else {
                    ImportStyle::String
                };

                let key = (module.to_owned(), target.clone(), is_instance);

                let expression = if should_defer && let Some(expression) = project.imports.get(&key)
                {
                    expression.clone()
                } else {
                    let expression = if project
                        .workspace
                        .can_import(module, target)
                        .map_err(failure)?
                    {
                        project
                            .workspace
                            .import(module, target, style)
                            .map_err(failure)?
                    } else {
                        None
                    };

                    if should_defer
                        && project
                            .has_capability("/workspace/didChangeWatchedFiles/dynamicRegistration")
                    {
                        project.imports.insert(key, expression.clone());
                    }

                    expression
                };

                let Some(expression) = expression else {
                    return Ok(None);
                };

                expression
            }
        }

        Import::Service(_) => {
            if !symbols.globals.contains("game") {
                return Ok(None);
            }

            format!("game:GetService(\"{name}\")")
        }
    };

    let root = expression.split(['.', ':', '[']).next().unwrap_or_default();

    if binding.is_none()
        && !expression.starts_with(['\'', '"'])
        && ((!is_type && root == name) || insertion.is_bound(root))
    {
        return Ok(None);
    }

    let expression = match target {
        Import::Module(_) | Import::Member { .. } | Import::Type { .. } if binding.is_none() => {
            format!("require({expression})")
        }

        Import::Member { .. } | Import::Type { .. } | Import::Module(_) | Import::Service(_) => {
            expression
        }
    };

    let value = if matches!(target, Import::Member { .. } | Import::Type { .. }) {
        format!("{expression}.{name}")
    } else {
        expression.clone()
    };

    let keyword = project.configuration.imports.binding.keyword();

    let (declaration_keyword, parameters, arguments) = if let Import::Type { module, name } = target
    {
        let Some(alias) = project
            .exports
            .get(module)
            .and_then(|exports| exports.types.iter().find(|alias| alias.name == *name))
        else {
            return Ok(None);
        };

        ("type", alias.parameters.as_str(), alias.arguments.as_str())
    } else {
        (keyword, "", "")
    };

    let declaration = match target {
        Import::Member { module, name } | Import::Type { module, name } if binding.is_none() => {
            let base = Import::Module(module.clone())
                .name()
                .unwrap_or("Module")
                .to_owned();

            let mut binding = base.clone();
            let mut suffix = 2;

            while binding == *name
                || symbols.globals.contains(&binding)
                || insertion.is_used(&binding)
            {
                binding = format!("{base}{suffix}");
                suffix += 1;
            }

            format!(
                "{keyword} {binding} = {expression}\n{declaration_keyword} {name}{parameters} = {binding}.{name}{arguments}"
            )
        }

        Import::Member { .. } | Import::Type { .. } | Import::Module(_) | Import::Service(_) => {
            format!("{declaration_keyword} {name}{parameters} = {value}{arguments}")
        }
    };

    if insertion.range.end > replacement.start || analysis::validate(&declaration).is_err() {
        return Ok(None);
    }

    let mut import_edit = None;

    if !should_defer {
        let settings = project.workspace.formatting(module).map_err(failure)?;
        let edit = insertion.format(&declaration, &settings).map_err(failure)?;
        let mut proposed = document.text.to_string();
        proposed.replace_range(replacement.clone(), name);
        proposed.replace_range(edit.range.clone(), &edit.replacement);

        if analysis::validate(&proposed).is_err() && analysis::validate(&document.text).is_ok() {
            return Ok(None);
        }

        import_edit = Some(edit);
    }

    let replacement = document.range(replacement, project.encoding)?;

    Ok(Some(CompletionItem {
        label: name.to_owned(),
        label_details: if let Import::Member { module, .. }
        | Import::Type { module, .. }
        | Import::Module(module) = target
        {
            Some(CompletionItemLabelDetails {
                description: Some(
                    Path::new(module)
                        .strip_prefix(project.workspace.directory())
                        .unwrap_or_else(|_| Path::new(module))
                        .display()
                        .to_string(),
                ),
                ..CompletionItemLabelDetails::default()
            })
        } else {
            None
        },
        kind: Some(match target {
            Import::Module(_) => CompletionItemKind::Module,
            Import::Service(_) => CompletionItemKind::Variable,
            Import::Type { .. } => CompletionItemKind::TypeParameter,

            Import::Member { module, name } => {
                if project.exports.get(module).is_some_and(|exports| {
                    exports
                        .members
                        .iter()
                        .any(|member| member.name == *name && member.is_function)
                }) {
                    CompletionItemKind::Function
                } else {
                    CompletionItemKind::Variable
                }
            }
        }),
        detail: Some(value),
        text_edit: Some(text_edit(
            project,
            replacement,
            position,
            if let Some(edit) = &import_edit
                && is_combined
            {
                format!("{}{name}", edit.replacement)
            } else {
                name.to_owned()
            },
        )),
        insert_text_format: Some(InsertTextFormat::PlainText),
        additional_text_edits: if is_combined {
            None
        } else {
            import_edit
                .map(|edit| -> Result<_> {
                    Ok(vec![TextEdit {
                        range: document.range(edit.range, project.encoding)?,
                        new_text: edit.replacement,
                    }])
                })
                .transpose()?
        },
        sort_text: Some(ranking(project, module, target, quality, context)?),
        ..CompletionItem::default()
    }))
}

fn ranking(
    project: &Project,
    source: &str,
    target: &Import,
    quality: u8,
    context: &ImportContext<'_>,
) -> Result<String> {
    let name = target.name().unwrap_or_default();

    let (priority, distance, route) = match target {
        Import::Service(name) => (1, 0, name.as_str()),

        Import::Module(module) | Import::Member { module, .. } | Import::Type { module, .. } => {
            let imported = matches!(target, Import::Member { .. } | Import::Type { .. })
                && context.imports.get(module).is_some_and(Option::is_some);

            let dependency = project
                .workspace
                .imports(module)
                .map_err(failure)?
                .dependency_root(Path::new(module))
                .is_some();

            let priority = if imported {
                0
            } else if matches!(target, Import::Member { .. } | Import::Type { .. }) {
                4
            } else if dependency {
                3
            } else {
                2
            };

            let source = Path::new(source)
                .parent()
                .unwrap_or_else(|| Path::new(source));

            let target = Path::new(module)
                .parent()
                .unwrap_or_else(|| Path::new(module));

            let common = source
                .components()
                .zip(target.components())
                .take_while(|(left, right)| left == right)
                .count();

            let distance = source.components().count() + target.components().count() - 2 * common;

            (priority, distance, module.as_str())
        }
    };

    Ok(format!("~{priority}{quality}{distance:08}:{name}:{route}"))
}

pub(super) fn module_imports(
    module: &str,
    document: &Document,
    imports: &[instar_bridge::Import],
) -> Result<BTreeMap<String, Vec<Range<usize>>>> {
    let mut modules: BTreeMap<_, Vec<_>> = BTreeMap::new();

    for import in imports
        .iter()
        .filter(|import| import.location.module == module)
    {
        let ranges = modules.entry(import.target.clone()).or_default();

        if !imports
            .iter()
            .any(|other| other.location == import.location && other.target != import.target)
        {
            ranges.push(document.native_bytes(import.location.range)?);
        }
    }

    Ok(modules)
}

pub(super) fn module_bindings(
    imports: BTreeMap<String, Vec<Range<usize>>>,
    insertion: &Insertion<'_>,
) -> BTreeMap<String, Option<String>> {
    imports
        .into_iter()
        .map(|(module, ranges)| {
            (
                module,
                ranges.iter().find_map(|range| insertion.binding(range)),
            )
        })
        .collect()
}
