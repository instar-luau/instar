use vermis::tree::{NodeIndex, NodeKind};

use crate::{
    lint::{context::Context, layout},
    syntax::{bindings::Category, expression::unwrap},
};

pub(super) fn finish(context: &mut Context<'_, '_>) {
    let tree = context.tree;

    if context.is_active(&[
        "unused_variable",
        "LocalUnused",
        "FunctionUnused",
        "ImportUnused",
        "LocalShadow",
        "DuplicateLocal",
        "UninitializedLocal",
        "prefer_const",
        "ignored_pcall_result",
    ]) {
        for binding in &context.bindings().locals {
            let name = &binding.name;
            let is_used = !binding.reads.is_empty() || !binding.type_reads.is_empty();

            let is_exported = binding.category == Category::Local
                && context.bindings().is_exported(tree, binding.statement);

            let is_constant = matches!(
                tree.node(binding.statement).kind,
                NodeKind::Constant { .. }
            ) || matches!(tree.node(binding.statement).kind, NodeKind::Function { prefix, .. }
                if is_exported || prefix.is_some_and(|prefix| tree.token(prefix).bytes(tree.source) == b"const"));

            let is_function =
                matches!(tree.node(binding.statement).kind, NodeKind::Function { .. });

            let is_import = binding.value.is_some_and(|value| matches!(tree.node(unwrap(tree, value)).kind, NodeKind::Call { callee, .. } if context.bindings().is_global(tree, callee, "require")));

            if context.is_active(&["unused_variable"])
                && !is_exported
                && !is_used
                && !context.settings.ignored.is_match(&binding.name)
                && (binding.category != Category::Parameter
                    || context.settings.unused.should_check_parameters)
                && (binding.category != Category::Loop
                    || context.settings.unused.should_check_loop_variables)
            {
                let category = match binding.category {
                    Category::Local if is_function => "Function",
                    Category::Local => "Variable",
                    Category::Parameter => "Parameter",
                    Category::Loop => "Loop variable",
                };

                context.emit(
                    binding.declaration,
                    "unused_variable",
                    &format!("{category} `{name}` is never read."),
                );
            }

            if context.is_active(&["LocalUnused", "FunctionUnused", "ImportUnused"])
                && !is_used
                && binding.category == Category::Local
                && !binding.name.starts_with('_')
            {
                let category = if is_function {
                    "Local function"
                } else if is_import {
                    "Imported binding"
                } else {
                    "Local binding"
                };

                context.emit(
                    binding.declaration,
                    if is_function {
                        "FunctionUnused"
                    } else if is_import {
                        "ImportUnused"
                    } else {
                        "LocalUnused"
                    },
                    &format!("{category} `{name}` is never used; prefix its name with `_` to suppress this diagnostic."),
                );
            }

            if context.is_active(&["DuplicateLocal", "LocalShadow"])
                && let Some(shadow) = binding.shadow
            {
                let shadow = &context.bindings().locals[shadow];

                if shadow.statement == binding.statement && binding.name != "_" {
                    if context.is_active(&["DuplicateLocal"]) {
                        context.emit(
                            binding.declaration,
                            "DuplicateLocal",
                            &format!(
                                "Binding `{name}` has already been declared in this statement."
                            ),
                        );
                    }
                } else if context.is_active(&["LocalShadow"])
                    && is_used
                    && shadow.function == binding.function
                    && !(is_function
                        && matches!(tree.node(shadow.statement).kind, NodeKind::Function { .. })
                        && context.settings.enabled("DuplicateFunction").is_some())
                {
                    context.emit(
                        binding.declaration,
                        "LocalShadow",
                        &format!("Binding `{name}` shadows a previous local declaration."),
                    );
                }
            } else if context.is_active(&["LocalShadow"])
                && is_used
                && context
                    .bindings()
                    .globals
                    .get(&binding.name)
                    .is_some_and(|global| !global.references.is_empty())
                && !context.symbols.globals.contains(&binding.name)
            {
                context.emit(
                    binding.declaration,
                    "LocalShadow",
                    &format!("Binding `{name}` shadows a global referenced in this module."),
                );
            }

            if context.is_active(&["UninitializedLocal"])
                && !binding.is_initialized
                && !binding.is_assigned
                && binding.category == Category::Local
                && let Some(first) = binding.reads.first()
            {
                context.emit(
                    *first,
                    "UninitializedLocal",
                    &format!("Local binding `{name}` is never initialized or assigned; initialize it with `nil` to suppress this diagnostic."),
                );
            }

            if let Some(value) = binding.value {
                let should_keep_local = context.settings.immutable.should_keep_mutated_tables_local
                    && binding.is_mutated
                    && matches!(tree.node(unwrap(tree, value)).kind, NodeKind::Table { .. });

                if context.is_active(&["prefer_const"])
                    && !(binding.is_assigned || is_constant || should_keep_local)
                    && binding.category == Category::Local
                {
                    context.emit(
                        binding.declaration,
                        "prefer_const",
                        &format!("Local binding `{name}` is never reassigned; consider declaring it with `const`."),
                    );
                }

                if context.is_active(&["ignored_pcall_result"])
                    && binding.reads.is_empty()
                    && matches!(tree.node(unwrap(tree, value)).kind, NodeKind::Call { callee, .. } if context.bindings().is_global(tree, callee, "pcall") || context.bindings().is_global(tree, callee, "xpcall"))
                {
                    context.emit(
                        binding.declaration,
                        "ignored_pcall_result",
                        "The success result of this protected call is never read.",
                    );
                }
            }
        }
    }

    if context.is_active(&[
        "global_usage",
        "UnknownGlobal",
        "BuiltinGlobalWrite",
        "unscoped_variables",
        "GlobalUsedAsLocal",
        "FunctionUnused",
    ]) {
        for (name, global) in &context.bindings().globals {
            let is_known = context.symbols.globals.contains(name)
                || context.bindings().declared.contains(name);

            for reference in &global.references {
                if context.is_active(&["global_usage"]) {
                    context.emit(
                        *reference,
                        "global_usage",
                        &format!("Global `{name}` is accessed."),
                    );
                }

                if context.is_active(&["UnknownGlobal"]) && !is_known && global.writes.is_empty() {
                    context.emit(
                        *reference,
                        "UnknownGlobal",
                        &format!("Unknown global `{name}`."),
                    );
                }
            }

            if context.is_active(&[if is_known {
                "BuiltinGlobalWrite"
            } else {
                "unscoped_variables"
            }]) {
                for write in &global.writes {
                    context.emit(
                        *write,
                        if is_known {
                            "BuiltinGlobalWrite"
                        } else {
                            "unscoped_variables"
                        },
                        &format!(
                            "Assignment writes to {} global `{name}`.",
                            if is_known { "declared" } else { "undeclared" }
                        ),
                    );
                }
            }

            if context.is_active(&["GlobalUsedAsLocal"])
                && !is_known
                && name != "_"
                && !global.writes.is_empty()
                && (!global.functions.is_empty()
                    || (!global.is_read_before_write && !global.has_module_assignment))
                && let Some(first) = global.references.first()
            {
                context.emit(
                    *first,
                    "GlobalUsedAsLocal",
                    &format!("Global `{name}` may be intended as a local binding."),
                );
            }

            if context.is_active(&["FunctionUnused"])
                && let Some(definition) = global.definition
                && !global.is_read
                && !name.starts_with('_')
            {
                context.emit(
                    definition,
                    "FunctionUnused",
                    &format!("Function `{name}` is not read in this module; prefix its name with `_` to suppress this diagnostic."),
                );
            }
        }
    }

    if context.is_active(&["FunctionUnused"]) {
        for binding in &context.bindings().types {
            if matches!(tree.node(binding.statement).kind, NodeKind::Function { .. })
                && !binding.is_used
                && !context.bindings().is_exported(tree, binding.statement)
                && !binding.name.starts_with('_')
            {
                context.emit(
                    binding.declaration,
                    "FunctionUnused",
                    &format!("Type function `{}` is never used; prefix its name with `_` to suppress this diagnostic.", binding.name),
                );
            }
        }
    }

    if context.is_active(&["restricted_globals"]) && !context.settings.globals.is_empty() {
        for (name, global) in &context.bindings().globals {
            if let Some(reason) = context.settings.globals.get(name) {
                let message = format!("Global `{name}` is restricted.\n{reason}");

                for reference in &global.references {
                    context.emit(*reference, "restricted_globals", &message);
                }
            }
        }
    }

    if context.is_active(&["PlaceholderRead"]) {
        for index in 0..tree.nodes.len() {
            let node = NodeIndex::new(index);

            if context.bindings().is_read[index] && tree.text(node) == b"_" {
                context.emit(
                    node,
                    "PlaceholderRead",
                    "Placeholder value `_` is read; use a named variable.",
                );
            }
        }
    }

    if context.is_active(&["CommentDirective", "RedundantNativeAttribute"]) {
        layout::comments(context);
    }
}
