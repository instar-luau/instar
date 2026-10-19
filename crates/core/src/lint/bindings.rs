use vermis::tree::{NodeIndex, NodeKind, Tree};

use crate::{
    lint::{context::Context, layout},
    syntax::{
        bindings::{Bindings, Category},
        expression::unwrap,
    },
};

impl Bindings {
    pub(super) fn same(&self, tree: &Tree<'_>, left: NodeIndex, right: NodeIndex) -> bool {
        let left = unwrap(tree, left);
        let right = unwrap(tree, right);

        matches!(tree.node(left).kind, NodeKind::Name { .. })
            && matches!(tree.node(right).kind, NodeKind::Name { .. })
            && self.references[left.get()] == self.references[right.get()]
            && tree.text(left) == tree.text(right)
    }
}

pub(super) fn finish(context: &mut Context<'_, '_>) {
    let tree = context.tree;

    if context.active(&[
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
            let used = !binding.reads.is_empty() || !binding.type_reads.is_empty();

            let exported = binding.category == Category::Local
                && context.bindings().exported(tree, binding.statement);

            let constant = matches!(tree.node(binding.statement).kind, NodeKind::Constant { .. })
                || matches!(tree.node(binding.statement).kind, NodeKind::Function { prefix, .. }
                if exported || prefix.is_some_and(|prefix| tree.token(prefix).bytes(tree.source) == b"const"));

            let function = matches!(tree.node(binding.statement).kind, NodeKind::Function { .. });

            let import = binding.value.is_some_and(|value| matches!(tree.node(unwrap(tree, value)).kind, NodeKind::Call { callee, .. } if context.bindings().global(tree, callee, "require")));

            if context.active(&["unused_variable"])
                && !exported
                && !used
                && !context.settings.ignored.is_match(&binding.name)
                && (binding.category != Category::Parameter || context.settings.unused.parameters)
                && (binding.category != Category::Loop || context.settings.unused.loop_variables)
            {
                context.warn(
                    binding.declaration,
                    "unused_variable",
                    "binding is never read",
                );
            }

            if context.active(&["LocalUnused", "FunctionUnused", "ImportUnused"])
                && !used
                && binding.category == Category::Local
                && !binding.name.starts_with('_')
            {
                context.warn(
                    binding.declaration,
                    if function {
                        "FunctionUnused"
                    } else if import {
                        "ImportUnused"
                    } else {
                        "LocalUnused"
                    },
                    "binding is never used; prefix with `_` to silence",
                );
            }

            if context.active(&["DuplicateLocal", "LocalShadow"])
                && let Some(shadow) = binding.shadow
            {
                let shadow = &context.bindings().locals[shadow];

                if shadow.statement == binding.statement && binding.name != "_" {
                    if context.active(&["DuplicateLocal"]) {
                        context.warn(
                            binding.declaration,
                            "DuplicateLocal",
                            "binding has already been declared in this statement",
                        );
                    }
                } else if context.active(&["LocalShadow"])
                    && used
                    && shadow.function == binding.function
                    && !(function
                        && matches!(tree.node(shadow.statement).kind, NodeKind::Function { .. })
                        && context.settings.enabled("DuplicateFunction").is_some())
                {
                    context.warn(
                        binding.declaration,
                        "LocalShadow",
                        "binding shadows a previous local declaration",
                    );
                }
            } else if context.active(&["LocalShadow"])
                && used
                && context
                    .bindings()
                    .globals
                    .get(&binding.name)
                    .is_some_and(|global| !global.references.is_empty())
                && !context.symbols.globals.contains(&binding.name)
            {
                context.warn(
                    binding.declaration,
                    "LocalShadow",
                    "binding shadows a global used in this module",
                );
            }

            if context.active(&["UninitializedLocal"])
                && !binding.initialized
                && !binding.assigned
                && binding.category == Category::Local
                && let Some(first) = binding.reads.first()
            {
                context.warn(
                    *first,
                    "UninitializedLocal",
                    "binding is never initialized or assigned; initialize with nil to silence",
                );
            }

            if let Some(value) = binding.value {
                let keep_local = context.settings.immutable.mutated_tables_stay_local
                    && binding.mutated
                    && matches!(tree.node(unwrap(tree, value)).kind, NodeKind::Table { .. });

                if context.active(&["prefer_const"])
                    && !(binding.assigned || constant || keep_local)
                    && binding.category == Category::Local
                {
                    context.warn(
                        binding.declaration,
                        "prefer_const",
                        "unchanged local binding can be declared const",
                    );
                }

                if context.active(&["ignored_pcall_result"])
                    && binding.reads.is_empty()
                    && matches!(tree.node(unwrap(tree, value)).kind, NodeKind::Call { callee, .. } if context.bindings().global(tree, callee, "pcall") || context.bindings().global(tree, callee, "xpcall"))
                {
                    context.warn(
                        binding.declaration,
                        "ignored_pcall_result",
                        "protected-call success status is never read",
                    );
                }
            }
        }
    }

    if context.active(&[
        "global_usage",
        "UnknownGlobal",
        "BuiltinGlobalWrite",
        "unscoped_variables",
        "GlobalUsedAsLocal",
        "FunctionUnused",
    ]) {
        for (name, global) in &context.bindings().globals {
            let known = context.symbols.globals.contains(name)
                || context.bindings().declared.contains(name);

            for reference in &global.references {
                if context.active(&["global_usage"]) {
                    context.warn(*reference, "global_usage", "access to a global binding");
                }

                if context.active(&["UnknownGlobal"]) && !known && global.writes.is_empty() {
                    context.warn(
                        *reference,
                        "UnknownGlobal",
                        "unknown global; consider assigning to it first",
                    );
                }
            }

            if context.active(&[if known {
                "BuiltinGlobalWrite"
            } else {
                "unscoped_variables"
            }]) {
                for write in &global.writes {
                    context.warn(
                        *write,
                        if known {
                            "BuiltinGlobalWrite"
                        } else {
                            "unscoped_variables"
                        },
                        if known {
                            "built-in global is overwritten"
                        } else {
                            "assignment creates an undeclared global"
                        },
                    );
                }
            }

            if context.active(&["GlobalUsedAsLocal"])
                && !known
                && name != "_"
                && !global.writes.is_empty()
                && (!global.functions.is_empty()
                    || (!global.read_before_write && !global.module_assignment))
                && let Some(first) = global.references.first()
            {
                context.warn(
                    *first,
                    "GlobalUsedAsLocal",
                    "global is only used locally; consider declaring a local",
                );
            }

            if context.active(&["FunctionUnused"])
                && let Some(definition) = global.definition
                && !global.read
                && !name.starts_with('_')
            {
                context.warn(
                    definition,
                    "FunctionUnused",
                    "function is never used; prefix with `_` to silence",
                );
            }
        }
    }

    if context.active(&["FunctionUnused"]) {
        for binding in &context.bindings().types {
            if matches!(tree.node(binding.statement).kind, NodeKind::Function { .. })
                && !binding.used
                && !context.bindings().exported(tree, binding.statement)
                && !binding.name.starts_with('_')
            {
                context.warn(
                    binding.declaration,
                    "FunctionUnused",
                    "function is never used; prefix with `_` to silence",
                );
            }
        }
    }

    if context.active(&["restricted_globals"]) && !context.settings.globals.is_empty() {
        for (name, global) in &context.bindings().globals {
            if let Some(reason) = context.settings.globals.get(name) {
                let message = format!("global `{name}` is restricted: {reason}");

                for reference in &global.references {
                    context.warn(*reference, "restricted_globals", &message);
                }
            }
        }
    }

    if context.active(&["PlaceholderRead"]) {
        for index in 0..tree.nodes.len() {
            let node = NodeIndex::new(index);

            if context.bindings().reads[index] && tree.text(node) == b"_" {
                context.warn(
                    node,
                    "PlaceholderRead",
                    "placeholder value `_` is read; use a named variable",
                );
            }
        }
    }

    if context.active(&["CommentDirective", "RedundantNativeAttribute"]) {
        layout::comments(context);
    }
}
