use vermis::{
    token::Span,
    tree::{NodeIndex, NodeKind},
};

use super::{
    Context,
    syntax::{arguments_values, comparison, in_loop, range},
};

use crate::Rule;

impl Context<'_, '_> {
    pub(super) fn requires(&mut self) {
        for site in self.source.requires {
            if !site.constant {
                self.emit(
                    Span {
                        start: site.argument[0],
                        end: site.argument[1],
                    },
                    Rule::NonConstRequire,
                    "require argument is not statically known",
                );
            }

            if let Some(path) = &site.path
                && let Some(reason) = self
                    .source
                    .configuration
                    .restricted_module_paths
                    .paths
                    .get(path)
            {
                self.emit(
                    Span {
                        start: site.argument[0],
                        end: site.argument[1],
                    },
                    Rule::RestrictedModulePaths,
                    format!("module path {path} is restricted: {reason}"),
                );
            }
        }
    }

    pub(super) fn discarded(&mut self, node: NodeIndex) {
        let NodeKind::Call { callee, .. } = &self.tree.node(node).kind else {
            return;
        };

        if self.global(*callee, b"pcall") || self.global(*callee, b"xpcall") {
            self.finding(
                node,
                Rule::IgnoredPcallResult,
                "protected-call result is discarded",
            );
        }

        if let NodeKind::Field { receiver, name, .. } = &self.tree.node(*callee).kind {
            let pure = (self.global(*receiver, b"math")
                && matches!(
                    self.tree.text(*name),
                    b"abs"
                        | b"floor"
                        | b"ceil"
                        | b"sqrt"
                        | b"max"
                        | b"min"
                        | b"sin"
                        | b"cos"
                        | b"tan"
                        | b"log"
                        | b"exp"
                ))
                || (self.global(*receiver, b"string")
                    && matches!(
                        self.tree.text(*name),
                        b"lower" | b"upper" | b"sub" | b"len" | b"rep" | b"reverse" | b"format"
                    ))
                || (self.global(*receiver, b"table")
                    && matches!(self.tree.text(*name), b"clone" | b"concat" | b"find"));

            if pure {
                self.finding(
                    node,
                    Rule::MustUse,
                    "result of a pure function is discarded",
                );
            }
        }
    }

    pub(super) fn call(
        &mut self,
        node: NodeIndex,
        callee: NodeIndex,
        arguments: NodeIndex,
        ancestors: &[NodeIndex],
    ) {
        let values = arguments_values(self.tree, arguments);

        if (self.global(callee, b"type") || self.global(callee, b"typeof"))
            && values.len() == 1
            && comparison(self.tree, values[0])
        {
            self.finding(
                node,
                Rule::TypeCheckInsideCall,
                "comparison belongs outside the type check",
            );
        }

        if let Some(&(expected, variadic)) = self.bindings.arities.get(&callee.get()) {
            let expands = values.last().is_some_and(|value| {
                matches!(
                    self.tree.node(*value).kind,
                    NodeKind::Call { .. } | NodeKind::MethodCall { .. } | NodeKind::Variadic { .. }
                )
            });

            if (values.len() > expected && !variadic) || (values.len() < expected && !expands) {
                self.finding(
                    node,
                    Rule::MismatchedArgumentCount,
                    "call argument count differs from the local function declaration",
                );
            }
        }

        if in_loop(self.tree, node, ancestors)
            && self
                .source
                .requires
                .iter()
                .any(|site| site.call == range(self.tree, node) && site.constant)
        {
            self.finding(
                node,
                Rule::LoopInvariantCall,
                "constant require call repeated inside a loop",
            );
        }
    }
}

pub(super) fn check(context: &mut Context<'_, '_>, node: NodeIndex, ancestors: &[NodeIndex]) {
    match &context.tree.node(node).kind {
        NodeKind::CallStatement { call } => context.discarded(*call),
        NodeKind::Call { callee, arguments } => context.call(node, *callee, *arguments, ancestors),
        _ => {}
    }
}
