use std::{
    cmp::Ordering::{Equal, Greater, Less},
    io,
    time::Instant,
};

use vermis::{
    token::{Keyword, Span, Symbol, TokenKind},
    tree::{NodeIndex, NodeKind, Tree},
};

use crate::bindings::{Bindings, Category};
use crate::{Completion, Diagnostic, Kind, Level, Location, Options, Reason, Result, Rule, Source};

struct Context<'tree, 'source> {
    tree: &'tree Tree<'source>,
    source: &'tree Source<'source>,
    bindings: Bindings,
    ignored: regex::Regex,
    findings: Vec<(Span, Rule, String)>,
}

pub(super) fn lint<Module: Clone>(
    module: Module,
    source: &Source<'_>,
    options: &Options,
) -> io::Result<Result<Module>> {
    source.configuration.validate().map_err(io::Error::other)?;
    let started = Instant::now();

    let mut result = Result {
        modules: vec![module.clone()],
        diagnostics: Vec::new(),
        completion: Completion::Complete,
    };

    if let Some(reason) = interrupted(options, started) {
        result.completion = Completion::Incomplete(reason);

        return Ok(result);
    }

    validate_ranges(source)?;
    let tree = vermis::parse(source.text.as_bytes());

    if let Some(reason) = interrupted(options, started) {
        result.completion = Completion::Incomplete(reason);

        return Ok(result);
    }

    if !tree.diagnostics.is_empty() {
        for diagnostic in &tree.diagnostics {
            result.diagnostics.push(Diagnostic {
                location: Location {
                    module: module.clone(),
                    revision: source.revision,
                    range: [diagnostic.span.start, diagnostic.span.end],
                },
                kind: Kind::Syntax,
                level: Level::Deny,
                message: diagnostic.message.to_owned(),
                related: Vec::new(),
            });
        }

        return Ok(result);
    }

    let bindings = Bindings::analyze(&tree, options, started);

    if let Some(reason) = bindings.interruption {
        result.completion = Completion::Incomplete(reason);

        return Ok(result);
    }

    let mut context = Context {
        tree: &tree,
        source,
        bindings,
        ignored: regex::Regex::new(&source.configuration.unused_variable.ignore_pattern)
            .map_err(io::Error::other)?,
        findings: Vec::new(),
    };

    context.declarations();
    let mut pending = vec![(tree.root, Vec::new())];

    while let Some((node, ancestors)) = pending.pop() {
        if let Some(reason) = interrupted(options, started) {
            result.completion = Completion::Incomplete(reason);
            break;
        }

        context.check(node, &ancestors);
        let mut ancestors = ancestors;
        ancestors.push(node);

        for child in tree.children(node).into_iter().rev() {
            pending.push((child, ancestors.clone()));
        }
    }

    context.requires();
    context.inferences(&module, &mut result)?;

    context
        .findings
        .sort_by_key(|(span, rule, _)| (span.start, span.end, *rule));

    context.findings.dedup();

    for (span, rule, message) in context.findings {
        result.diagnostics.push(Diagnostic {
            location: Location {
                module: module.clone(),
                revision: source.revision,
                range: [span.start, span.end],
            },
            kind: Kind::Rule(rule),
            level: source.configuration.level(rule),
            message,
            related: Vec::new(),
        });
    }

    Ok(result)
}

fn valid_range(text: &str, range: [usize; 2]) -> bool {
    range[0] <= range[1]
        && range[1] <= text.len()
        && text.is_char_boundary(range[0])
        && text.is_char_boundary(range[1])
}

fn validate_ranges(source: &Source<'_>) -> io::Result<()> {
    for site in source.requires {
        if !valid_range(source.text, site.call)
            || !valid_range(source.text, site.argument)
            || site.call[0] > site.argument[0]
            || site.argument[1] > site.call[1]
            || site.path.is_some() && !site.constant
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "require site is not anchored to source bytes",
            ));
        }
    }

    Ok(())
}

fn interrupted(options: &Options, started: Instant) -> Option<Reason> {
    if options.cancellation.requested() {
        Some(Reason::Cancelled)
    } else if started.elapsed() >= options.timeout {
        Some(Reason::Timeout)
    } else {
        None
    }
}

impl Context<'_, '_> {
    fn inferences<Module: Clone>(
        &mut self,
        module: &Module,
        result: &mut Result<Module>,
    ) -> io::Result<()> {
        let source = self.source;

        if let Some(inferred) = source.inferred {
            for inference in inferred {
                if !valid_range(source.text, inference.range) {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "native inference is not anchored to source bytes",
                    ));
                }

                let rule = if inference.parameter {
                    Rule::ImplicitAnyParameter
                } else {
                    Rule::ImplicitAnyLocal
                };

                self.emit(
                    Span {
                        start: inference.range[0],
                        end: inference.range[1],
                    },
                    rule,
                    "unannotated binding has inferred any type",
                );
            }
        } else if source.configuration.semantic() {
            for rule in [Rule::ImplicitAnyLocal, Rule::ImplicitAnyParameter] {
                if source.configuration.level(rule) != Level::Allow {
                    result.diagnostics.push(Diagnostic {
                        location: Location {
                            module: module.clone(),
                            revision: source.revision,
                            range: [0, 0],
                        },
                        kind: Kind::Unsupported,
                        level: Level::Deny,
                        message: format!("{rule} requires shared native inferred binding types"),
                        related: Vec::new(),
                    });
                }
            }

            if result.completion == Completion::Complete {
                result.completion = Completion::Incomplete(Reason::Unsupported);
            }
        }

        Ok(())
    }

    fn emit(&mut self, span: Span, rule: Rule, message: impl Into<String>) {
        if self.source.configuration.level(rule) != Level::Allow {
            self.findings.push((span, rule, message.into()));
        }
    }

    fn finding(&mut self, node: NodeIndex, rule: Rule, message: impl Into<String>) {
        self.emit(self.tree.node(node).span, rule, message);
    }

    fn global(&self, node: NodeIndex, name: &[u8]) -> bool {
        matches!(self.tree.node(node).kind, NodeKind::Name { .. })
            && self.tree.text(node) == name
            && self.bindings.global(self.tree, node)
    }

    fn declarations(&mut self) {
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

    fn requires(&mut self) {
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

    fn check(&mut self, node: NodeIndex, ancestors: &[NodeIndex]) {
        match &self.tree.node(node).kind {
            NodeKind::Block { .. } => self.swapped(node),
            NodeKind::Binary { .. } => self.binary(node, ancestors),

            NodeKind::Assignment { .. } | NodeKind::CompoundAssignment { .. } => {
                self.assignments(node, ancestors);
            }

            NodeKind::Branch { condition, .. }
            | NodeKind::While { condition, .. }
            | NodeKind::Repeat { condition, .. }
            | NodeKind::Conditional { condition, .. } => self.condition(*condition),

            NodeKind::CallStatement { call } => self.discarded(*call),
            NodeKind::Call { callee, arguments } => self.call(node, *callee, *arguments, ancestors),

            NodeKind::If {
                branches,
                otherwise,
                ..
            } => self.conditional(node, branches, *otherwise),

            NodeKind::Else { .. } => self.else_branch(node, ancestors),
            NodeKind::Table { .. } => self.table(node),
            NodeKind::Name { .. } => self.name(node),
            NodeKind::Function { .. } => self.function(node),

            NodeKind::String { token }
                if matches!(
                    self.tree.token(*token).kind,
                    TokenKind::QuotedString
                        | TokenKind::RawString
                        | TokenKind::InterpolatedStringSimple
                ) && crate::literal::bytes(self.tree, node).is_none() =>
            {
                self.finding(
                    node,
                    Rule::BadStringEscape,
                    "string contains an invalid escape",
                );
            }

            _ => {}
        }

        self.loops(node);
        self.deprecated(node, ancestors);
        self.performance(node, ancestors);

        if self.source.roblox {
            self.roblox(node);
        }
    }

    fn swapped(&mut self, node: NodeIndex) {
        let tree = self.tree;

        let NodeKind::Block { statements } = &tree.node(node).kind else {
            return;
        };

        for pair in tree.list(statements).windows(2) {
            if let (Some((left, right)), Some((other_left, other_right))) = (
                assignment(tree, pair[0].node),
                assignment(tree, pair[1].node),
            ) && matches!(tree.node(left).kind, NodeKind::Name { .. })
                && matches!(tree.node(right).kind, NodeKind::Name { .. })
                && same(tree, left, other_right)
                && same(tree, right, other_left)
                && !same(tree, left, right)
            {
                self.finding(
                    pair[1].node,
                    Rule::AlmostSwapped,
                    "sequential assignments overwrite a value before swapping it",
                );
            }
        }
    }

    fn binary(&mut self, node: NodeIndex, ancestors: &[NodeIndex]) {
        let tree = self.tree;

        let NodeKind::Binary {
            left,
            operator,
            right,
        } = &tree.node(node).kind
        else {
            return;
        };

        let operator = tree.token(*operator).kind;

        if matches!(
            operator,
            TokenKind::Symbol(Symbol::Divide | Symbol::FloorDivide | Symbol::Modulo)
        ) && number(tree, *right) == Some(0.0)
        {
            self.finding(*right, Rule::DivideByZero, "division or modulo by zero");
        }

        if matches!(
            operator,
            TokenKind::Symbol(Symbol::Equal | Symbol::NotEqual)
        ) {
            if nan(tree, *left) || nan(tree, *right) {
                self.finding(
                    node,
                    Rule::CompareNan,
                    "comparison with NaN has a fixed result",
                );
            }

            if matches!(tree.node(unwrap(tree, *left)).kind, NodeKind::Table { .. })
                || matches!(tree.node(unwrap(tree, *right)).kind, NodeKind::Table { .. })
            {
                self.finding(
                    node,
                    Rule::ConstantTableComparison,
                    "fresh tables are compared by identity",
                );
            }
        }

        if operator == TokenKind::Keyword(Keyword::Or)
            && let NodeKind::Binary {
                operator, right, ..
            } = &tree.node(unwrap(tree, *left)).kind
            && tree.token(*operator).kind == TokenKind::Keyword(Keyword::And)
            && truth(tree, *right) != Some(false)
        {
            self.finding(
                node,
                Rule::AndOrConditional,
                "use an if expression instead of an and/or conditional",
            );
        }

        if operator == TokenKind::Symbol(Symbol::Concatenate)
            && in_loop(tree, node, ancestors)
            && let Some(parent) = ancestors.last()
            && let Some((target, value)) = assignment(tree, *parent)
            && value == node
            && same(tree, target, *left)
        {
            self.finding(
                node,
                Rule::StringConcatInLoop,
                "repeated concatenation grows a string quadratically",
            );
        }
    }

    fn assignments(&mut self, node: NodeIndex, ancestors: &[NodeIndex]) {
        let tree = self.tree;

        match &tree.node(node).kind {
            NodeKind::Assignment {
                targets, values, ..
            } => {
                for (target, value) in tree.list(targets).iter().zip(tree.list(values)) {
                    if stable(tree, target.node) && same(tree, target.node, value.node) {
                        self.finding(
                            target.node,
                            Rule::SelfAssignment,
                            "assignment leaves a value unchanged",
                        );
                    }
                }

                for target in tree.list(targets) {
                    self.unscoped(target.node);
                }
            }

            NodeKind::CompoundAssignment {
                target, operator, ..
            } => {
                self.unscoped(*target);

                if tree.token(*operator).kind == TokenKind::Symbol(Symbol::ConcatenateAssignment)
                    && in_loop(tree, node, ancestors)
                {
                    self.finding(
                        node,
                        Rule::StringConcatInLoop,
                        "repeated concatenation grows a string quadratically",
                    );
                }
            }

            _ => {}
        }
    }

    fn table(&mut self, node: NodeIndex) {
        let tree = self.tree;

        let NodeKind::Table { fields, .. } = &tree.node(node).kind else {
            return;
        };

        let mut positional = false;
        let mut keyed = false;

        for field in tree.list(fields) {
            if let NodeKind::TableField { key, opening, .. } = &tree.node(field.node).kind {
                if key.is_some() || opening.is_some() {
                    keyed = true;
                } else {
                    positional = true;
                }
            }
        }

        if keyed && positional {
            self.finding(
                node,
                Rule::MixedTable,
                "table mixes positional and keyed entries",
            );
        }
    }

    fn name(&mut self, node: NodeIndex) {
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

    fn function(&mut self, node: NodeIndex) {
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

    fn loops(&mut self, node: NodeIndex) {
        if let NodeKind::NumericFor {
            step: Some(step), ..
        } = &self.tree.node(node).kind
            && number(self.tree, *step) == Some(0.0)
        {
            self.finding(*step, Rule::ZeroStepLoop, "numeric loop step is zero");
        }

        match &self.tree.node(node).kind {
            NodeKind::While { body, .. }
            | NodeKind::Repeat { body, .. }
            | NodeKind::NumericFor { body, .. }
            | NodeKind::GenericFor { body, .. }
                if empty(self.tree, *body) =>
            {
                self.finding(*body, Rule::EmptyLoop, "empty loop body");
            }

            _ => {}
        }
    }

    fn condition(&mut self, condition: NodeIndex) {
        if matches!(self.tree.node(condition).kind, NodeKind::Group { .. }) {
            self.finding(
                condition,
                Rule::ParenthesizedConditions,
                "unnecessary parentheses around condition",
            );
        }

        let node = unwrap(self.tree, condition);

        if truth(self.tree, node).is_some() {
            self.finding(
                node,
                Rule::ConstantCondition,
                "condition has a fixed truth value",
            );
        }

        if let NodeKind::Unary { operator, .. } = &self.tree.node(node).kind
            && self.tree.token(*operator).kind == TokenKind::Symbol(Symbol::Length)
        {
            self.finding(
                node,
                Rule::LengthAsCondition,
                "zero is truthy in Luau; compare the length explicitly",
            );
        }
    }

    fn unscoped(&mut self, node: NodeIndex) {
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

    fn discarded(&mut self, node: NodeIndex) {
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

    fn call(
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

    fn conditional(
        &mut self,
        node: NodeIndex,
        branches: &vermis::tree::NodeList,
        otherwise: Option<NodeIndex>,
    ) {
        let tree = self.tree;
        let branches = tree.list(branches);

        let bodies = branches
            .iter()
            .filter_map(|branch| match &tree.node(branch.node).kind {
                NodeKind::Branch { body, .. } => Some(*body),
                _ => None,
            })
            .collect::<Vec<_>>();

        for body in &bodies {
            if empty(tree, *body) {
                self.finding(*body, Rule::EmptyIf, "empty conditional branch");
            }
        }

        if let Some(otherwise) = otherwise {
            let body = body(tree, otherwise);

            if empty(tree, body) {
                self.finding(body, Rule::EmptyIf, "empty conditional branch");
            }

            if let Some(first) = bodies.first()
                && bodies.iter().all(|other| same(tree, *first, *other))
                && same(tree, *first, body)
            {
                self.finding(
                    node,
                    Rule::IfSameThenElse,
                    "conditional branches have identical bodies",
                );
            }

            if branches.len() == 1 {
                if let NodeKind::Branch { condition, .. } = &tree.node(branches[0].node).kind
                    && let NodeKind::Unary { operator, .. } =
                        &tree.node(unwrap(tree, *condition)).kind
                    && tree.token(*operator).kind == TokenKind::Keyword(Keyword::Not)
                {
                    self.finding(
                        *condition,
                        Rule::NegatedCondition,
                        "negated condition can be expressed by swapping branches",
                    );
                }

                if let Some(first) = bodies.first()
                    && let (Some(left), Some(right)) = (single(tree, *first), single(tree, body))
                    && let (Some((left, _)), Some((right, _))) =
                        (assignment(tree, left), assignment(tree, right))
                    && matches!(tree.node(left).kind, NodeKind::Name { .. })
                    && same(tree, left, right)
                {
                    self.finding(
                        node,
                        Rule::IfExpressionAssignment,
                        "matching branch assignments can use an if expression",
                    );
                }
            }
        } else if branches.len() == 1
            && let Some(first) = bodies.first()
            && let Some(inner) = single(tree, *first)
            && let NodeKind::If {
                branches,
                otherwise: None,
                ..
            } = &tree.node(inner).kind
            && tree.list(branches).len() == 1
        {
            self.finding(
                node,
                Rule::CollapsibleIf,
                "nested single-branch conditionals can be combined",
            );
        }
    }

    fn else_branch(&mut self, node: NodeIndex, ancestors: &[NodeIndex]) {
        let Some(parent) = ancestors.last() else {
            return;
        };

        let NodeKind::If { branches, .. } = &self.tree.node(*parent).kind else {
            return;
        };

        if self.tree.list(branches).iter().all(|branch| {
            let NodeKind::Branch { body, .. } = &self.tree.node(branch.node).kind else {
                return false;
            };

            let NodeKind::Block { statements } = &self.tree.node(*body).kind else {
                return false;
            };

            self.tree.list(statements).last().is_some_and(|statement| {
                matches!(self.tree.node(statement.node).kind, NodeKind::Return { .. })
            })
        }) {
            self.finding(
                node,
                Rule::ElseAfterReturn,
                "else follows branches that return",
            );
        }
    }

    fn deprecated(&mut self, node: NodeIndex, ancestors: &[NodeIndex]) {
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
                crate_path(tree, *parent).is_some_and(|parent| options.paths.contains_key(&parent))
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

    fn performance(&mut self, node: NodeIndex, ancestors: &[NodeIndex]) {
        let tree = self.tree;

        if let NodeKind::MethodCall {
            receiver,
            method,
            arguments,
            ..
        } = &tree.node(node).kind
            && self.global(*receiver, b"game")
            && tree.text(*method) == b"GetService"
            && arguments_values(tree, *arguments).len() == 1
            && matches!(
                tree.node(arguments_values(tree, *arguments)[0]).kind,
                NodeKind::String { .. }
            )
            && in_loop(tree, node, ancestors)
        {
            self.finding(
                node,
                Rule::LoopInvariantCall,
                "GetService call repeated inside a loop",
            );
        }

        let NodeKind::GenericFor {
            bindings,
            values,
            body,
            ..
        } = &tree.node(node).kind
        else {
            return;
        };

        let bindings = tree.list(bindings);
        let values = tree.list(values);

        if bindings.len() != 2 || values.len() != 1 {
            return;
        }

        let (NodeKind::Binding { name: key, .. }, NodeKind::Binding { name: value, .. }) = (
            &tree.node(bindings[0].node).kind,
            &tree.node(bindings[1].node).kind,
        ) else {
            return;
        };

        let NodeKind::Call { callee, arguments } = &tree.node(values[0].node).kind else {
            return;
        };

        if !self.global(*callee, b"pairs") || arguments_values(tree, *arguments).len() != 1 {
            return;
        }

        let Some(statement) = single(tree, *body) else {
            return;
        };

        let Some((target, assigned)) = assignment(tree, statement) else {
            return;
        };

        let NodeKind::Index {
            receiver,
            key: assigned_key,
            ..
        } = &tree.node(target).kind
        else {
            return;
        };

        let Some(destination) = self.bindings.declaration(*receiver) else {
            return;
        };

        if destination.assigned
            || !destination
                .value
                .is_some_and(|value| empty_table(tree, value))
        {
            return;
        }

        if matches!(tree.node(assigned).kind, NodeKind::Name { .. })
            && same(tree, assigned, *value)
            && same(tree, *assigned_key, *key)
            && self.bindings.references.get(&assigned.get()) == Some(&value.get())
            && self.bindings.references.get(&assigned_key.get()) == Some(&key.get())
        {
            self.finding(
                node,
                Rule::ManualTableClone,
                "table-copy loop can use table.clone",
            );
        }
    }

    fn roblox(&mut self, node: NodeIndex) {
        let tree = self.tree;

        if let NodeKind::Call { callee, arguments } = &tree.node(node).kind
            && let NodeKind::Field { receiver, name, .. } = &tree.node(*callee).kind
            && tree.text(*name) == b"new"
        {
            let values = arguments_values(tree, *arguments);

            if self.global(*receiver, b"Color3")
                && values.iter().any(|value| {
                    number(tree, *value).is_some_and(|number| !(0.0..=1.0).contains(&number))
                })
            {
                self.finding(
                    node,
                    Rule::RobloxIncorrectColor3NewBounds,
                    "Color3.new channels use a 0–1 scale",
                );
            }

            if self.global(*receiver, b"UDim2") {
                if values.len() == 2 {
                    self.finding(
                        node,
                        Rule::RobloxSuspiciousUdim2New,
                        "UDim2.new needs scale and offset components for both axes",
                    );
                }

                if values.len() == 4 {
                    let scale = number(tree, values[1]) == Some(0.0)
                        && number(tree, values[3]) == Some(0.0);

                    let offset = number(tree, values[0]) == Some(0.0)
                        && number(tree, values[2]) == Some(0.0);

                    if scale || offset {
                        self.finding(
                            node,
                            Rule::RobloxManualFromscaleOrFromoffset,
                            if scale {
                                "use UDim2.fromScale when both offsets are zero"
                            } else {
                                "use UDim2.fromOffset when both scales are zero"
                            },
                        );
                    }
                }
            }
        }

        if let NodeKind::MethodCall {
            receiver, method, ..
        } = &tree.node(node).kind
            && tree.text(*method) == b"GetChildren"
            && self.players(*receiver)
        {
            self.finding(
                node,
                Rule::RobloxPreferGetPlayers,
                "use Players:GetPlayers() to select players",
            );
        }
    }

    fn players(&self, node: NodeIndex) -> bool {
        let node = unwrap(self.tree, node);

        let value = if matches!(self.tree.node(node).kind, NodeKind::Name { .. }) {
            let Some(binding) = self.bindings.declaration(node) else {
                return false;
            };

            if binding.assigned {
                return false;
            }

            let Some(value) = binding.value else {
                return false;
            };

            value
        } else {
            node
        };

        let NodeKind::MethodCall {
            receiver,
            method,
            arguments,
            ..
        } = &self.tree.node(value).kind
        else {
            return false;
        };

        let values = arguments_values(self.tree, *arguments);

        self.global(*receiver, b"game")
            && self.tree.text(*method) == b"GetService"
            && values.len() == 1
            && matches!(self.tree.node(values[0]).kind, NodeKind::String { .. })
            && crate::literal::string(self.tree, values[0]).as_deref() == Some("Players")
    }
}

fn unwrap(tree: &Tree<'_>, node: NodeIndex) -> NodeIndex {
    match &tree.node(node).kind {
        NodeKind::Group { expression, .. }
        | NodeKind::Assertion { expression, .. }
        | NodeKind::Instantiate { expression, .. } => unwrap(tree, *expression),

        _ => node,
    }
}

fn number(tree: &Tree<'_>, node: NodeIndex) -> Option<f64> {
    let node = unwrap(tree, node);

    if let NodeKind::Unary { operator, operand } = &tree.node(node).kind
        && tree.token(*operator).kind == TokenKind::Symbol(Symbol::Subtract)
    {
        return number(tree, *operand).map(|number| -number);
    }

    if let NodeKind::Binary {
        left,
        operator,
        right,
    } = &tree.node(node).kind
    {
        let (left, right) = (number(tree, *left)?, number(tree, *right)?);

        return match tree.token(*operator).kind {
            TokenKind::Symbol(Symbol::Add) => Some(left + right),
            TokenKind::Symbol(Symbol::Subtract) => Some(left - right),
            TokenKind::Symbol(Symbol::Multiply) => Some(left * right),
            TokenKind::Symbol(Symbol::Divide) => Some(left / right),
            TokenKind::Symbol(Symbol::FloorDivide) => Some((left / right).floor()),
            TokenKind::Symbol(Symbol::Modulo) => Some(left - right * (left / right).floor()),
            TokenKind::Symbol(Symbol::Power) => Some(left.powf(right)),
            _ => None,
        };
    }

    if !matches!(tree.node(node).kind, NodeKind::Number { .. }) {
        return None;
    }

    let value = String::from_utf8_lossy(tree.text(node)).replace('_', "");

    if let Some(digits) = value
        .strip_prefix("0x")
        .or_else(|| value.strip_prefix("0X"))
    {
        return integer(digits, 16);
    }

    if let Some(digits) = value
        .strip_prefix("0b")
        .or_else(|| value.strip_prefix("0B"))
    {
        return integer(digits, 2);
    }

    value.parse().ok()
}

fn integer(digits: &str, radix: u32) -> Option<f64> {
    if digits.is_empty() {
        return None;
    }

    digits.chars().try_fold(0.0, |value, digit| {
        digit
            .to_digit(radix)
            .map(|digit| value * f64::from(radix) + f64::from(digit))
    })
}

fn truth(tree: &Tree<'_>, node: NodeIndex) -> Option<bool> {
    let node = unwrap(tree, node);

    if number(tree, node).is_some() {
        return Some(true);
    }

    match &tree.node(node).kind {
        NodeKind::Nil { .. } => Some(false),

        NodeKind::Boolean { token } => {
            Some(tree.token(*token).kind == TokenKind::Keyword(Keyword::True))
        }

        NodeKind::String { .. } | NodeKind::Table { .. } | NodeKind::Function { .. } => Some(true),

        NodeKind::Unary { operator, operand }
            if tree.token(*operator).kind == TokenKind::Keyword(Keyword::Not) =>
        {
            truth(tree, *operand).map(|value| !value)
        }

        NodeKind::Binary {
            left,
            operator,
            right,
        } => {
            let ordering = number(tree, *left)?.partial_cmp(&number(tree, *right)?);

            match tree.token(*operator).kind {
                TokenKind::Symbol(Symbol::Equal) => Some(ordering == Some(Equal)),
                TokenKind::Symbol(Symbol::NotEqual) => Some(ordering != Some(Equal)),
                TokenKind::Symbol(Symbol::LessThan) => Some(ordering == Some(Less)),
                TokenKind::Symbol(Symbol::GreaterThan) => Some(ordering == Some(Greater)),

                TokenKind::Symbol(Symbol::LessThanOrEqual) => {
                    Some(matches!(ordering, Some(Less | Equal)))
                }

                TokenKind::Symbol(Symbol::GreaterThanOrEqual) => {
                    Some(matches!(ordering, Some(Greater | Equal)))
                }

                _ => None,
            }
        }

        _ => None,
    }
}

fn nan(tree: &Tree<'_>, node: NodeIndex) -> bool {
    number(tree, node).is_some_and(f64::is_nan)
}

fn comparison(tree: &Tree<'_>, node: NodeIndex) -> bool {
    matches!(&tree.node(unwrap(tree, node)).kind, NodeKind::Binary { operator, .. } if matches!(tree.token(*operator).kind, TokenKind::Symbol(Symbol::Equal | Symbol::NotEqual | Symbol::LessThan | Symbol::GreaterThan | Symbol::LessThanOrEqual | Symbol::GreaterThanOrEqual)))
}

fn range(tree: &Tree<'_>, node: NodeIndex) -> [usize; 2] {
    let span = tree.node(node).span;

    [span.start, span.end]
}

fn same(tree: &Tree<'_>, left: NodeIndex, right: NodeIndex) -> bool {
    let tokens = |node: NodeIndex| {
        let span = tree.node(node).span;

        tree.tokens
            .iter()
            .filter(move |token| {
                token.span.start >= span.start
                    && token.span.end <= span.end
                    && !matches!(
                        token.kind,
                        TokenKind::Whitespace | TokenKind::Comment | TokenKind::BlockComment
                    )
            })
            .map(|token| (token.kind, token.bytes(tree.source)))
    };

    tokens(left).eq(tokens(right))
}

fn stable(tree: &Tree<'_>, node: NodeIndex) -> bool {
    match &tree.node(node).kind {
        NodeKind::Name { .. } => true,
        NodeKind::Field { receiver, .. } => stable(tree, *receiver),

        NodeKind::Index { receiver, key, .. } => {
            stable(tree, *receiver)
                && matches!(
                    tree.node(*key).kind,
                    NodeKind::String { .. } | NodeKind::Number { .. }
                )
        }

        _ => false,
    }
}

fn body(tree: &Tree<'_>, node: NodeIndex) -> NodeIndex {
    if let NodeKind::Else { body, .. } = &tree.node(node).kind {
        *body
    } else {
        node
    }
}

fn empty(tree: &Tree<'_>, node: NodeIndex) -> bool {
    let node = body(tree, node);

    matches!(&tree.node(node).kind, NodeKind::Block { statements } if tree.list(statements).is_empty())
}

fn empty_table(tree: &Tree<'_>, node: NodeIndex) -> bool {
    matches!(&tree.node(unwrap(tree, node)).kind, NodeKind::Table { fields, .. } if tree.list(fields).is_empty())
}

fn single(tree: &Tree<'_>, node: NodeIndex) -> Option<NodeIndex> {
    let NodeKind::Block { statements } = &tree.node(body(tree, node)).kind else {
        return None;
    };

    let [statement] = tree.list(statements) else {
        return None;
    };

    Some(statement.node)
}

fn assignment(tree: &Tree<'_>, node: NodeIndex) -> Option<(NodeIndex, NodeIndex)> {
    let NodeKind::Assignment {
        targets, values, ..
    } = &tree.node(node).kind
    else {
        return None;
    };

    let ([target], [value]) = (tree.list(targets), tree.list(values)) else {
        return None;
    };

    Some((target.node, value.node))
}

fn arguments_values(tree: &Tree<'_>, node: NodeIndex) -> Vec<NodeIndex> {
    if let NodeKind::Arguments { values, .. } = &tree.node(node).kind {
        tree.list(values).iter().map(|value| value.node).collect()
    } else {
        Vec::new()
    }
}

fn in_loop(tree: &Tree<'_>, node: NodeIndex, ancestors: &[NodeIndex]) -> bool {
    for ancestor in ancestors.iter().rev() {
        match &tree.node(*ancestor).kind {
            NodeKind::Function { .. } => return false,

            NodeKind::While { body, .. }
            | NodeKind::Repeat { body, .. }
            | NodeKind::NumericFor { body, .. }
            | NodeKind::GenericFor { body, .. } => {
                let body = tree.node(*body).span;
                let span = tree.node(node).span;

                if body.start <= span.start && span.end <= body.end {
                    return true;
                }
            }

            _ => {}
        }
    }

    false
}

fn complexity(tree: &Tree<'_>, node: NodeIndex) -> usize {
    let branch = match &tree.node(node).kind {
        NodeKind::Function { .. } => return 0,

        NodeKind::Branch { .. }
        | NodeKind::While { .. }
        | NodeKind::Repeat { .. }
        | NodeKind::NumericFor { .. }
        | NodeKind::GenericFor { .. }
        | NodeKind::Conditional { .. } => 1,

        NodeKind::Binary { operator, .. } => usize::from(matches!(
            tree.token(*operator).kind,
            TokenKind::Keyword(Keyword::And | Keyword::Or)
        )),

        _ => 0,
    };

    branch
        + tree
            .children(node)
            .into_iter()
            .map(|child| complexity(tree, child))
            .sum::<usize>()
}

fn path(tree: &Tree<'_>, node: NodeIndex) -> Option<(NodeIndex, String)> {
    match &tree.node(node).kind {
        NodeKind::Name { .. } => {
            Some((node, String::from_utf8_lossy(tree.text(node)).into_owned()))
        }

        NodeKind::Field { receiver, name, .. }
        | NodeKind::MethodCall {
            receiver,
            method: name,
            ..
        } => {
            let (root, path) = path(tree, *receiver)?;

            Some((
                root,
                format!("{path}.{}", String::from_utf8_lossy(tree.text(*name))),
            ))
        }

        NodeKind::Group { expression, .. } => path(tree, *expression),
        _ => None,
    }
}

fn crate_path(tree: &Tree<'_>, node: NodeIndex) -> Option<String> {
    path(tree, node).map(|(_, path)| path)
}

fn builtin(name: &[u8]) -> bool {
    [
        b"assert".as_slice(),
        b"bit32",
        b"buffer",
        b"coroutine",
        b"debug",
        b"error",
        b"getmetatable",
        b"ipairs",
        b"math",
        b"next",
        b"os",
        b"pairs",
        b"pcall",
        b"print",
        b"rawequal",
        b"rawget",
        b"rawset",
        b"require",
        b"select",
        b"setmetatable",
        b"string",
        b"table",
        b"tonumber",
        b"tostring",
        b"type",
        b"typeof",
        b"utf8",
        b"xpcall",
        b"_G",
        b"_VERSION",
    ]
    .contains(&name)
}
