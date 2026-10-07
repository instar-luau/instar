use instar_syntax::bindings::Category;
use vermis::tree::{NodeIndex, NodeKind};

use super::{
    Context,
    syntax::{builtin, complexity, path, symbol_path, unwrap},
};

use crate::{Level, Rule};

impl Context<'_, '_> {
    pub(super) fn deprecated(&mut self, node: NodeIndex, ancestors: &[NodeIndex]) {
        let tree = self.tree;

        if !matches!(
            tree.node(node).kind,
            NodeKind::Name { .. } | NodeKind::Field { .. } | NodeKind::MethodCall { .. }
        ) {
            return;
        }

        let options = &self.source.configuration.deprecated;

        if let Some((root, path)) = path(tree, node)
            && self.bindings.global(tree, root)
            && let Some(replacement) = options.paths.get(&path)
        {
            if ancestors.last().is_some_and(|parent| {
                symbol_path(tree, *parent).is_some_and(|parent| options.paths.contains_key(&parent))
            }) {
                return;
            }

            self.finding(
                node,
                Rule::Deprecated,
                format!("{path} is deprecated; use {replacement}"),
            );
        } else if options.ambiguous_methods
            && let NodeKind::MethodCall { method, .. } = &tree.node(node).kind
        {
            let method = String::from_utf8_lossy(tree.text(*method));

            if let Some((_, replacement)) = options
                .paths
                .iter()
                .find(|(path, _)| path.rsplit('.').next() == Some(method.as_ref()))
            {
                self.finding(
                    node,
                    Rule::Deprecated,
                    format!("method {method} may be deprecated; use {replacement}"),
                );
            }
        }
    }

    pub(super) fn declarations(&mut self) {
        let mut findings = Vec::new();

        for binding in self.bindings.declarations.values() {
            let name = String::from_utf8_lossy(self.tree.text(binding.name));

            let included = match binding.category {
                Category::Parameter => self.source.configuration.unused_variable.parameters,
                Category::Loop => self.source.configuration.unused_variable.loop_variables,
                _ => true,
            };

            if included && !binding.read && !self.ignored.is_match(&name) {
                findings.push((binding.name, Rule::UnusedVariable, "binding is never read"));
            }

            if !binding.read
                && let Some(value) = binding.value
                && let NodeKind::Call { callee, .. } =
                    &self.tree.node(unwrap(self.tree, value)).kind
                && (self.global(*callee, b"pcall") || self.global(*callee, b"xpcall"))
            {
                findings.push((
                    binding.name,
                    Rule::IgnoredPcallResult,
                    "protected-call success status is never read",
                ));
            }

            if binding.category == Category::Local && binding.value.is_some() && !binding.assigned {
                let table = binding.value.is_some_and(|value| {
                    matches!(
                        self.tree.node(unwrap(self.tree, value)).kind,
                        NodeKind::Table { .. }
                    )
                });

                if !(table
                    && binding.mutated
                    && self
                        .source
                        .configuration
                        .prefer_const
                        .mutated_tables_stay_local)
                {
                    findings.push((
                        binding.name,
                        Rule::PreferConst,
                        "unchanged local binding can be declared const",
                    ));
                }
            }
        }

        for (node, rule, message) in findings {
            self.finding(node, rule, message);
        }
    }

    pub(super) fn name(&mut self, node: NodeIndex) {
        if self.bindings.globals.contains(&node.get()) {
            self.finding(node, Rule::GlobalUsage, "access to a global binding");
            let name = String::from_utf8_lossy(self.tree.text(node));

            if let Some(reason) = self
                .source
                .configuration
                .restricted_globals
                .names
                .get(name.as_ref())
            {
                self.finding(
                    node,
                    Rule::RestrictedGlobals,
                    format!("global {name} is restricted: {reason}"),
                );
            }
        }
    }

    pub(super) fn function(&mut self, node: NodeIndex) {
        let tree = self.tree;

        let NodeKind::Function {
            body: Some(body),
            prefix,
            name,
            ..
        } = &tree.node(node).kind
        else {
            return;
        };

        if prefix.is_some_and(|prefix| tree.token(prefix).bytes(tree.source) == b"type") {
            return;
        }

        if prefix.is_none()
            && let Some(name) = name
        {
            if let NodeKind::FunctionName {
                path, method: None, ..
            } = &tree.node(*name).kind
            {
                if let [name] = tree.list(path) {
                    self.unscoped(name.node);
                }
            } else {
                self.unscoped(*name);
            }
        }

        if self.source.configuration.high_cyclomatic_complexity.level == Level::Allow {
            return;
        }

        let score = complexity(tree, *body) + 1;

        let maximum = self
            .source
            .configuration
            .high_cyclomatic_complexity
            .maximum_complexity
            .get();

        if score > maximum {
            self.finding(
                node,
                Rule::HighCyclomaticComplexity,
                format!("function complexity {score} exceeds maximum {maximum}"),
            );
        }
    }

    pub(super) fn unscoped(&mut self, node: NodeIndex) {
        if !matches!(self.tree.node(node).kind, NodeKind::Name { .. })
            || !self.bindings.globals.contains(&node.get())
        {
            return;
        }

        let name = self.tree.text(node);

        if !builtin(name)
            && !self
                .source
                .globals
                .iter()
                .any(|global| global.as_bytes() == name)
        {
            self.finding(
                node,
                Rule::UnscopedVariables,
                "assignment creates an undeclared global",
            );
        }
    }
}

pub(super) fn check(context: &mut Context<'_, '_>, node: NodeIndex, ancestors: &[NodeIndex]) {
    match &context.tree.node(node).kind {
        NodeKind::Name { .. } => context.name(node),
        NodeKind::Function { .. } => context.function(node),
        _ => {}
    }

    context.deprecated(node, ancestors);
}
