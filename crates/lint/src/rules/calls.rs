use std::collections::BTreeMap;

use instar_luau::{ControlFlow, Determinism, EffectFact, FactsResult, SemanticIdentity};
use instar_shared::{Knowledge, Options, Reason};

use instar_syntax::{
    literal,
    token::{Span, Symbol, TokenKind},
    tree::{NodeIndex, NodeKind, Tree},
};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::expressions::{arguments_values, assignment, comparison, in_loop, range, same, unwrap};
use crate::{RuleIdentity, RuleInput, RuleResult, RuleSettings, request::Context};

/// Restricted decoded import requests and their reasons.
#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct RestrictedImports {
    /// Exact requested paths mapped to reasons.
    pub paths: BTreeMap<String, String>,
}

impl Context<'_, '_> {
    fn requires(&mut self, options: &Options) {
        for site in self.imports {
            if let Some(completion) = options.interrupted() {
                self.result.completion = self.result.completion.clone().combine(completion);
                break;
            }

            if self.rule == RuleIdentity::NonConstRequire && !site.constant {
                self.emit(
                    Span {
                        start: site.argument.span.start,
                        end: site.argument.span.end,
                    },
                    RuleIdentity::NonConstRequire,
                    "require argument is not statically known",
                );
            }

            if let RuleSettings::RestrictedModulePaths(settings) = self.parameters {
                match &site.requested {
                    Knowledge::Known(Some(path)) => {
                        if let Some(reason) = settings.paths.get(path) {
                            self.emit(
                                Span {
                                    start: site.argument.span.start,
                                    end: site.argument.span.end,
                                },
                                RuleIdentity::RestrictedModulePaths,
                                format!("module path {path} is restricted: {reason}"),
                            );
                        }
                    }

                    Knowledge::Unavailable(reason) => self.unavailable(*reason),
                    Knowledge::Known(None) => {}
                }
            }
        }
    }

    fn protected_result(&mut self, node: NodeIndex) {
        let NodeKind::Call { callee, .. } = &self.tree.node(node).kind else {
            return;
        };

        if self.global(*callee, b"pcall") || self.global(*callee, b"xpcall") {
            self.finding(
                node,
                RuleIdentity::IgnoredPcallResult,
                "protected-call result is discarded",
            );
        }
    }

    fn pure_result(&mut self, node: NodeIndex) {
        let NodeKind::Call { callee, .. } = &self.tree.node(node).kind else {
            return;
        };

        if let NodeKind::Field { receiver, name, .. } = &self.tree.node(*callee).kind {
            let required = (self.global(*receiver, b"math")
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

            if required {
                self.finding(
                    node,
                    RuleIdentity::MustUse,
                    "result of a pure function is discarded",
                );
            }
        }
    }

    fn type_comparison(&mut self, node: NodeIndex) {
        let NodeKind::Call { callee, arguments } = &self.tree.node(node).kind else {
            return;
        };

        let callee = *callee;
        let arguments = *arguments;
        let values = arguments_values(self.tree, arguments);

        if (self.global(callee, b"type") || self.global(callee, b"typeof"))
            && values.len() == 1
            && comparison(self.tree, values[0])
        {
            self.finding(
                node,
                RuleIdentity::TypeCheckInsideCall,
                "comparison belongs outside the type check",
            );
        }
    }

    fn argument_count(&mut self, node: NodeIndex) {
        let NodeKind::Call { callee, arguments } = &self.tree.node(node).kind else {
            if let NodeKind::MethodCall { arguments, .. } = &self.tree.node(node).kind {
                self.semantic_count(node, &arguments_values(self.tree, *arguments));
            }

            return;
        };

        let callee = *callee;
        let values = arguments_values(self.tree, *arguments);

        let semantic = self.facts.is_some_and(|facts| {
            facts.facts.calls.iter().any(|call| call.location.source == self.source.version
                && [call.location.span.start, call.location.span.end] == range(self.tree, node)
                && matches!(&call.signatures, Knowledge::Known(signatures) if !signatures.is_empty()))
        });

        if semantic {
            self.semantic_count(node, &values);
        } else if let Some(&(expected, variadic)) = self.bindings.arities.get(&callee.get()) {
            let expands = values.last().is_some_and(|value| {
                matches!(
                    self.tree.node(*value).kind,
                    NodeKind::Call { .. } | NodeKind::MethodCall { .. } | NodeKind::Variadic { .. }
                )
            });

            if values.len() - usize::from(expands) > expected && !variadic
                || values.len() < expected && !expands
            {
                self.finding(
                    node,
                    RuleIdentity::MismatchedArgumentCount,
                    "call argument count differs from the local function declaration",
                );
            }
        } else {
            self.semantic_count(node, &values);
        }
    }

    fn require_call(&mut self, node: NodeIndex) {
        let NodeKind::Call { callee, arguments } = &self.tree.node(node).kind else {
            return;
        };

        let callee = *callee;
        let values = arguments_values(self.tree, *arguments);

        if self.global(callee, b"require")
            && !self
                .imports
                .iter()
                .any(|site| [site.call.span.start, site.call.span.end] == range(self.tree, node))
        {
            if values.len() == 1 {
                let argument = unwrap(self.tree, values[0]);

                if let Some(path) = literal::string(self.tree, argument) {
                    if let RuleSettings::RestrictedModulePaths(settings) = self.parameters
                        && let Some(reason) = settings.paths.get(&path)
                    {
                        self.finding(
                            argument,
                            RuleIdentity::RestrictedModulePaths,
                            format!("module path {path} is restricted: {reason}"),
                        );
                    }
                } else {
                    self.unavailable(Reason::FailedPrerequisite);
                }
            } else if self.rule == RuleIdentity::NonConstRequire {
                self.finding(
                    node,
                    RuleIdentity::NonConstRequire,
                    "require argument is not statically known",
                );
            }
        }
    }

    fn semantic_count(&mut self, node: NodeIndex, values: &[NodeIndex]) {
        let Some(facts) = self.facts else {
            self.unavailable(Reason::UnavailableSemantics);

            return;
        };

        let Some(call) = facts.facts.calls.iter().find(|call| {
            call.location.source == self.source.version
                && [call.location.span.start, call.location.span.end] == range(self.tree, node)
        }) else {
            self.unavailable(Reason::UnavailableSemantics);

            return;
        };

        let Knowledge::Known(signatures) = &call.signatures else {
            self.unavailable(Reason::UnavailableSemantics);

            return;
        };

        if signatures.is_empty() {
            return;
        }

        let expands = values.last().is_some_and(|value| {
            matches!(
                self.tree.node(*value).kind,
                NodeKind::Call { .. } | NodeKind::MethodCall { .. } | NodeKind::Variadic { .. }
            )
        });

        let count = values.len() - usize::from(expands);
        let mut unavailable = false;

        for signature in signatures {
            if !signature.variadic && count > signature.parameters.len() {
                continue;
            }

            let Knowledge::Known(minimum) = &signature.minimum_arguments else {
                unavailable = true;
                continue;
            };

            if !expands && count < *minimum {
                continue;
            }

            if expands && (!signature.variadic || count < *minimum) {
                unavailable = true;
                continue;
            }

            return;
        }

        if unavailable {
            self.unavailable(Reason::UnavailableSemantics);
        } else {
            self.finding(
                node,
                RuleIdentity::MismatchedArgumentCount,
                "call argument count is incompatible with every established signature",
            );
        }
    }

    fn concatenation(&mut self, node: NodeIndex, ancestors: &[NodeIndex]) {
        if !in_loop(self.tree, node, ancestors) {
            return;
        }

        let repeated = match &self.tree.node(node).kind {
            NodeKind::CompoundAssignment { operator, .. } => {
                self.tree.token(*operator).kind == TokenKind::Symbol(Symbol::ConcatenateAssignment)
            }

            NodeKind::Binary { left, operator, .. } => {
                self.tree.token(*operator).kind == TokenKind::Symbol(Symbol::Concatenate)
                    && ancestors
                        .last()
                        .and_then(|parent| assignment(self.tree, *parent))
                        .is_some_and(|(target, value)| {
                            value == node && same(self.tree, target, *left)
                        })
            }

            _ => false,
        };

        if repeated {
            self.finding(
                node,
                RuleIdentity::StringConcatInLoop,
                "repeated concatenation grows a string quadratically",
            );
        }
    }

    fn invariant(&mut self, node: NodeIndex, ancestors: &[NodeIndex], options: &Options) {
        if !matches!(
            self.tree.node(node).kind,
            NodeKind::Call { .. } | NodeKind::MethodCall { .. }
        ) {
            return;
        }

        if !self.bindings.recovery.is_empty() {
            self.unavailable(Reason::UnsupportedSyntax);

            return;
        }

        let Some(loop_node) = enclosing_loop(self.tree, node, ancestors) else {
            return;
        };

        let Some(facts) = self.facts else {
            self.unavailable(Reason::UnavailableSemantics);

            return;
        };

        let call_span = range(self.tree, node);

        let Some(call) = facts.facts.calls.iter().find(|call| {
            call.location.source == self.source.version
                && [call.location.span.start, call.location.span.end] == call_span
        }) else {
            self.unavailable(Reason::UnavailableSemantics);

            return;
        };

        let Knowledge::Known(effect) = &call.effects else {
            self.unavailable(Reason::UnavailableSemantics);

            return;
        };

        if !hoistable(effect) {
            return;
        }

        let Knowledge::Known(targets) = &call.targets else {
            self.unavailable(Reason::UnavailableSemantics);

            return;
        };

        if targets.is_empty() {
            return;
        }

        let Some(expression) = facts
            .facts
            .expressions
            .iter()
            .find(|expression| expression.location == call.location)
        else {
            self.unavailable(Reason::UnavailableSemantics);

            return;
        };

        let Knowledge::Known(dependencies) = &expression.dependencies else {
            self.unavailable(Reason::UnavailableSemantics);

            return;
        };

        let Knowledge::Known(expression_effect) = &expression.effects else {
            self.unavailable(Reason::UnavailableSemantics);

            return;
        };

        if !hoistable(expression_effect) {
            return;
        }

        let loop_span = self.tree.node(loop_node).span;

        let dependencies = dependencies
            .iter()
            .chain(&effect.reads)
            .chain(&expression_effect.reads)
            .chain(targets)
            .collect::<Vec<_>>();

        if dependencies.iter().any(|identity| matches!(identity, SemanticIdentity::Source { location, .. } if location.source == self.source.version && loop_span.start <= location.span.start && location.span.end <= loop_span.end)) { return; }

        let Some(writes) = self.loop_writes(loop_node, facts, options) else {
            return;
        };

        if dependencies
            .iter()
            .any(|dependency| writes.contains(dependency))
        {
            return;
        }

        self.finding(
            node,
            RuleIdentity::LoopInvariantCall,
            "call is proven deterministic, effect-free, and independent of loop writes",
        );
    }

    fn loop_writes<'facts>(
        &mut self,
        loop_node: NodeIndex,
        facts: &'facts FactsResult,
        options: &Options,
    ) -> Option<Vec<&'facts SemanticIdentity>> {
        let loop_span = self.tree.node(loop_node).span;
        let mut writes = Vec::new();

        for index in &self.bindings.writes {
            if let Some(completion) = options.interrupted() {
                self.result.completion = self.result.completion.clone().combine(completion);

                return None;
            }

            let Some(node) = self.tree.nodes.get(*index) else {
                self.unavailable(Reason::FailedPrerequisite);

                return None;
            };

            if loop_span.start <= node.span.start && node.span.end <= loop_span.end
                && !facts.facts.occurrences.iter().any(|occurrence| occurrence.location.source == self.source.version
                    && occurrence.location.span.start == node.span.start && occurrence.location.span.end == node.span.end
                    && occurrence.access.writes() && matches!(&occurrence.targets, Knowledge::Known(targets) if !targets.is_empty())) {
                self.unavailable(Reason::UnavailableSemantics);

                return None;
            }
        }

        for occurrence in &facts.facts.occurrences {
            if let Some(completion) = options.interrupted() {
                self.result.completion = self.result.completion.clone().combine(completion);

                return None;
            }

            if occurrence.access.writes()
                && occurrence.location.source == self.source.version
                && loop_span.start <= occurrence.location.span.start
                && occurrence.location.span.end <= loop_span.end
            {
                let Knowledge::Known(targets) = &occurrence.targets else {
                    self.unavailable(Reason::UnavailableSemantics);

                    return None;
                };

                writes.extend(targets.iter());
            }
        }

        let mut pending = vec![loop_node];

        while let Some(candidate) = pending.pop() {
            if let Some(completion) = options.interrupted() {
                self.result.completion = self.result.completion.clone().combine(completion);

                return None;
            }

            if candidate != loop_node
                && matches!(self.tree.node(candidate).kind, NodeKind::Function { .. })
            {
                continue;
            }

            if requires_effect(&self.tree.node(candidate).kind)
                && (!matches!(self.tree.node(candidate).kind, NodeKind::Name { .. })
                    || self.bindings.references.contains_key(&candidate.get())
                    || self.bindings.globals.contains(&candidate.get()))
            {
                let span = range(self.tree, candidate);

                let Some(expression) = facts.facts.expressions.iter().find(|expression| {
                    expression.location.source == self.source.version
                        && [expression.location.span.start, expression.location.span.end] == span
                }) else {
                    self.unavailable(Reason::UnavailableSemantics);

                    return None;
                };

                let Knowledge::Known(effect) = &expression.effects else {
                    self.unavailable(Reason::UnavailableSemantics);

                    return None;
                };

                if effect.control_flow != ControlFlow::Preserved || effect.may_yield {
                    return None;
                }

                writes.extend(effect.writes.iter());
            }

            pending.extend(self.tree.children(candidate));
        }

        Some(writes)
    }
}

pub(crate) fn evaluate(
    input: &RuleInput<'_, '_>,
    settings: &RuleSettings,
    options: &Options,
) -> RuleResult {
    let mut context = Context::new(input, settings);

    if context.rule == RuleIdentity::IgnoredPcallResult {
        context.declarations(options, |context, binding| {
            if !binding.read
                && let Some(value) = binding.value
                && let NodeKind::Call { callee, .. } =
                    &context.tree.node(unwrap(context.tree, value)).kind
                && (context.global(*callee, b"pcall") || context.global(*callee, b"xpcall"))
            {
                context.finding(
                    binding.name,
                    RuleIdentity::IgnoredPcallResult,
                    "protected-call success status is never read",
                );
            }
        });
    }

    if matches!(
        context.rule,
        RuleIdentity::NonConstRequire | RuleIdentity::RestrictedModulePaths
    ) {
        context.requires(options);
    }

    let check: fn(&mut Context<'_, '_>, NodeIndex, &[NodeIndex], &Options) = match context.rule {
        RuleIdentity::IgnoredPcallResult => |context, node, _, _| {
            if let NodeKind::CallStatement { call } = &context.tree.node(node).kind {
                context.protected_result(*call);
            }
        },

        RuleIdentity::MustUse => |context, node, _, _| {
            if let NodeKind::CallStatement { call } = &context.tree.node(node).kind {
                context.pure_result(*call);
            }
        },

        RuleIdentity::MismatchedArgumentCount => |context, node, _, _| context.argument_count(node),
        RuleIdentity::TypeCheckInsideCall => |context, node, _, _| context.type_comparison(node),

        RuleIdentity::NonConstRequire | RuleIdentity::RestrictedModulePaths => {
            |context, node, _, _| context.require_call(node)
        }

        RuleIdentity::StringConcatInLoop => {
            |context, node, ancestors, _| context.concatenation(node, ancestors)
        }

        RuleIdentity::LoopInvariantCall => {
            |context, node, ancestors, options| context.invariant(node, ancestors, options)
        }

        _ => unreachable!("call producer received another rule"),
    };

    context.walk(options, |context, node, ancestors| {
        check(context, node, ancestors, options)
    });

    context.finish(options)
}

fn hoistable(effect: &EffectFact) -> bool {
    effect.writes.is_empty()
        && !effect.may_throw
        && !effect.may_yield
        && !effect.may_allocate
        && effect.determinism == Determinism::Deterministic
        && effect.control_flow == ControlFlow::Preserved
}

fn enclosing_loop(tree: &Tree<'_>, node: NodeIndex, ancestors: &[NodeIndex]) -> Option<NodeIndex> {
    for ancestor in ancestors.iter().rev() {
        match &tree.node(*ancestor).kind {
            NodeKind::Function { .. } => return None,

            NodeKind::While { body, .. }
            | NodeKind::Repeat { body, .. }
            | NodeKind::NumericFor { body, .. }
            | NodeKind::GenericFor { body, .. } => {
                let span = tree.node(node).span;
                let body = tree.node(*body).span;

                return (body.start <= span.start && span.end <= body.end).then_some(*ancestor);
            }

            _ => {}
        }
    }

    None
}

fn requires_effect(kind: &NodeKind) -> bool {
    matches!(
        kind,
        NodeKind::Name { .. }
            | NodeKind::Field { .. }
            | NodeKind::Index { .. }
            | NodeKind::Call { .. }
            | NodeKind::MethodCall { .. }
            | NodeKind::Binary { .. }
            | NodeKind::Unary { .. }
            | NodeKind::Table { .. }
            | NodeKind::CompoundAssignment { .. }
            | NodeKind::GenericFor { .. }
    )
}
