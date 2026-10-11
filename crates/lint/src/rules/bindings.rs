use std::collections::BTreeMap;

use instar_luau::BindingKind;
use instar_shared::{Knowledge, Options, Reason};

use instar_syntax::{
    Binding, Category,
    tree::{NodeIndex, NodeKind},
};

use regex::Regex;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::expressions::{builtin, path, symbol_path, unwrap};
use crate::{RuleIdentity, RuleInput, RuleResult, RuleSettings, request::Context};

/// Exemptions for unread lexical declarations.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct UnusedBindings {
    /// Include unread function parameters.
    pub parameters: bool,

    /// Include unread loop variables.
    pub loop_variables: bool,

    /// Regular expression matching exempt names.
    pub ignore_pattern: String,
}

impl Default for UnusedBindings {
    fn default() -> Self {
        Self {
            parameters: false,
            loop_variables: false,
            ignore_pattern: "^_".to_owned(),
        }
    }
}

/// Policy for declarations whose binding is unchanged.
#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct ImmutableBindings {
    /// Retain local declarations when their table value is mutated.
    pub mutated_tables_stay_local: bool,
}

/// Project-deprecated global symbol paths.
#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct DeprecatedSymbols {
    /// Symbol paths mapped to suggested replacements.
    pub paths: BTreeMap<String, String>,

    /// Include ambiguous method names matching a configured path.
    pub ambiguous_methods: bool,
}

/// Restricted globals and their explanations.
#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct RestrictedGlobals {
    /// Global names mapped to reasons.
    pub names: BTreeMap<String, String>,
}

impl Context<'_, '_> {
    pub(crate) fn declarations(
        &mut self,
        options: &Options,
        mut check: impl FnMut(&mut Self, &Binding),
    ) {
        if !self.bindings.recovery.is_empty() {
            self.unavailable(Reason::UnsupportedSyntax);

            return;
        }

        let bindings = self.bindings;

        for binding in bindings.declarations.values() {
            if let Some(completion) = options.interrupted() {
                self.result.completion = self.result.completion.clone().combine(completion);
                break;
            }

            check(self, binding);
        }
    }

    fn deprecated(&mut self, node: NodeIndex, ancestors: &[NodeIndex]) {
        let RuleSettings::Deprecated(settings) = self.parameters else {
            return;
        };

        let tree = self.tree;

        if !matches!(
            tree.node(node).kind,
            NodeKind::Name { .. } | NodeKind::Field { .. } | NodeKind::MethodCall { .. }
        ) {
            return;
        }

        if let Some((root, path)) = path(tree, node)
            && self.bindings.global(tree, root)
            && let Some(replacement) = settings.paths.get(&path)
        {
            if ancestors.last().is_some_and(|parent| {
                symbol_path(tree, *parent)
                    .is_some_and(|parent| settings.paths.contains_key(&parent))
            }) {
                return;
            }

            self.finding(
                node,
                RuleIdentity::Deprecated,
                format!("{path} is deprecated; use {replacement}"),
            );
        } else if settings.ambiguous_methods
            && let NodeKind::MethodCall { method, .. } = &tree.node(node).kind
        {
            let method = String::from_utf8_lossy(tree.text(*method));

            if let Some((_, replacement)) = settings
                .paths
                .iter()
                .find(|(path, _)| path.rsplit('.').next() == Some(method.as_ref()))
            {
                self.finding(
                    node,
                    RuleIdentity::Deprecated,
                    format!("method {method} may be deprecated; use {replacement}"),
                );
            }
        }
    }

    fn implicit(&mut self, options: &Options) {
        let parameter = self.rule == RuleIdentity::ImplicitAnyParameter;
        let local = self.rule == RuleIdentity::ImplicitAnyLocal;

        let Some(facts) = self.facts else {
            self.unavailable(Reason::UnavailableSemantics);

            return;
        };

        let mut findings = Vec::new();
        let mut unavailable = false;

        for binding in self.bindings.declarations.values() {
            if let Some(completion) = options.interrupted() {
                self.result.completion = self.result.completion.clone().combine(completion);
                break;
            }

            if parameter && binding.category != Category::Parameter
                || local && !matches!(binding.category, Category::Local | Category::Constant)
            {
                continue;
            }

            let span = self.tree.node(binding.name).span;

            let expected = if parameter {
                BindingKind::Parameter
            } else {
                BindingKind::Local
            };

            let fact = facts.facts.bindings.iter().find(|fact| fact.kind == expected
                && matches!(&fact.declaration, Knowledge::Known(Some(location)) if location.source == self.source.version && location.span.start <= span.start && span.end <= location.span.end));

            let Some(fact) = fact else {
                unavailable = true;
                continue;
            };

            if fact.annotation {
                continue;
            }

            match &fact.type_information {
                Knowledge::Known(inferred) if inferred.is_any => findings.push(binding.name),
                Knowledge::Known(_) => {}
                Knowledge::Unavailable(_) => unavailable = true,
            }
        }

        for node in findings {
            self.finding(node, self.rule, "unannotated binding has inferred any type");
        }

        if unavailable {
            self.unavailable(Reason::UnavailableSemantics);
        }
    }

    fn name(&mut self, node: NodeIndex) {
        if !self.bindings.globals.contains(&node.get()) {
            return;
        }

        if let RuleSettings::RestrictedGlobals(settings) = self.parameters {
            let name = String::from_utf8_lossy(self.tree.text(node));

            if let Some(reason) = settings.names.get(name.as_ref()) {
                self.finding(
                    node,
                    RuleIdentity::RestrictedGlobals,
                    format!("global {name} is restricted: {reason}"),
                );
            }
        }
    }

    fn function(&mut self, node: NodeIndex) {
        let tree = self.tree;

        let NodeKind::Function {
            prefix: None,
            name: Some(name),
            ..
        } = &tree.node(node).kind
        else {
            return;
        };

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

    pub(crate) fn unscoped(&mut self, node: NodeIndex) {
        if !matches!(self.tree.node(node).kind, NodeKind::Name { .. })
            || !self.bindings.globals.contains(&node.get())
        {
            return;
        }

        let name = self.tree.text(node);

        if !builtin(name)
            && !self
                .policy
                .globals
                .iter()
                .any(|global| global.as_bytes() == name)
        {
            self.finding(
                node,
                RuleIdentity::UnscopedVariables,
                "assignment creates an undeclared global",
            );
        }
    }
}

pub(crate) fn evaluate(
    input: &RuleInput<'_, '_>,
    settings: &RuleSettings,
    options: &Options,
) -> RuleResult {
    let mut context = Context::new(input, settings);

    match settings {
        RuleSettings::UnusedVariable(parameters) => {
            let ignored = Regex::new(&parameters.ignore_pattern).ok();

            context.declarations(options, |context, binding| {
                let included = match binding.category {
                    Category::Parameter => parameters.parameters,
                    Category::Loop => parameters.loop_variables,
                    _ => true,
                };

                if included
                    && !binding.read
                    && ignored.as_ref().is_some_and(|ignored| {
                        !ignored.is_match(&String::from_utf8_lossy(context.tree.text(binding.name)))
                    })
                {
                    context.finding(
                        binding.name,
                        RuleIdentity::UnusedVariable,
                        "binding is never read",
                    );
                }
            });
        }

        RuleSettings::PreferConst(parameters) => {
            context.declarations(options, |context, binding| {
                if binding.category == Category::Local
                    && binding.value.is_some()
                    && !binding.assigned
                {
                    let table = binding.value.is_some_and(|value| {
                        matches!(
                            context.tree.node(unwrap(context.tree, value)).kind,
                            NodeKind::Table { .. }
                        )
                    });

                    if !(table && binding.mutated && parameters.mutated_tables_stay_local) {
                        context.finding(
                            binding.name,
                            RuleIdentity::PreferConst,
                            "unchanged local binding can be declared const",
                        );
                    }
                }
            });
        }

        RuleSettings::ImplicitAnyLocal(_) | RuleSettings::ImplicitAnyParameter(_) => {
            context.implicit(options);
        }

        RuleSettings::Deprecated(_) => context.walk(options, |context, node, ancestors| {
            context.deprecated(node, ancestors);
        }),

        RuleSettings::GlobalUsage(_) => context.walk(options, |context, node, _| {
            if matches!(context.tree.node(node).kind, NodeKind::Name { .. })
                && context.bindings.globals.contains(&node.get())
            {
                context.finding(
                    node,
                    RuleIdentity::GlobalUsage,
                    "access to a global binding",
                );
            }
        }),

        RuleSettings::RestrictedGlobals(_) => context.walk(options, |context, node, _| {
            if matches!(context.tree.node(node).kind, NodeKind::Name { .. }) {
                context.name(node);
            }
        }),

        RuleSettings::UnscopedVariables(_) => context.walk(options, unscoped),
        _ => unreachable!("binding producer received another rule"),
    }

    context.finish(options)
}

fn unscoped(context: &mut Context<'_, '_>, node: NodeIndex, _: &[NodeIndex]) {
    match &context.tree.node(node).kind {
        NodeKind::Function { .. } => context.function(node),

        NodeKind::Assignment { targets, .. } => {
            for target in context.tree.list(targets) {
                context.unscoped(target.node);
            }
        }

        NodeKind::CompoundAssignment { target, .. } => context.unscoped(*target),
        _ => {}
    }
}
