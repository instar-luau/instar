use std::{
    cmp::Ordering,
    collections::{BTreeMap, BTreeSet},
    mem,
};

use vermis::{
    token::{Keyword, Symbol, TokenKind},
    tree::{NodeIndex, NodeKind, NodeList, Tree},
};

use crate::{
    lint::{
        Edit, Finding,
        bindings::{Category, expands, text},
        syntax::{Context, number, same, single, truth, unwrap},
    },
    syntax::literal,
};

pub(super) const RULES: &[&str] = &[
    "almost_swapped",
    "must_use",
    "self_assignment",
    "ignored_pcall_result",
    "unscoped_variables",
    "roblox_incorrect_color3_new_bounds",
    "roblox_manual_fromscale_or_fromoffset",
    "roblox_prefer_get_players",
    "roblox_suspicious_udim2_new",
    "if_expression_assignment",
    "non_const_require",
    "global_usage",
    "type_check_inside_call",
    "loop_invariant_call",
    "manual_table_clone",
    "string_concat_in_loop",
    "unused_variable",
    "prefer_const",
    "restricted_globals",
    "restricted_module_paths",
    "UnknownGlobal",
    "GlobalUsedAsLocal",
    "LocalShadow",
    "SameLineStatement",
    "MultiLineStatement",
    "LocalUnused",
    "FunctionUnused",
    "ImportUnused",
    "BuiltinGlobalWrite",
    "PlaceholderRead",
    "UnreachableCode",
    "UnknownType",
    "ForRange",
    "UnbalancedAssignment",
    "ImplicitReturn",
    "TableLiteral",
    "UninitializedLocal",
    "DuplicateFunction",
    "DuplicateCondition",
    "DuplicateLocal",
    "MisleadingAndOr",
    "IntegerParsing",
    "ComparisonPrecedence",
    "CommentDirective",
    "RedundantNativeAttribute",
];

const RESULTS: &[&str] = &[
    "math.abs",
    "math.floor",
    "math.ceil",
    "math.sqrt",
    "math.max",
    "math.min",
    "math.sin",
    "math.cos",
    "math.tan",
    "math.log",
    "math.exp",
    "string.lower",
    "string.upper",
    "string.sub",
    "string.len",
    "string.rep",
    "string.reverse",
    "string.format",
    "table.clone",
    "table.concat",
    "table.find",
];

const ARITHMETIC: &[&str] = &[
    "math.abs",
    "math.floor",
    "math.ceil",
    "math.sqrt",
    "math.sin",
    "math.cos",
    "math.tan",
    "math.exp",
];

const PRIMITIVES: &[&str] = &[
    "nil", "boolean", "userdata", "number", "string", "table", "function", "thread", "buffer",
    "vector",
];

#[derive(Copy, Clone, Eq, Ord, PartialEq, PartialOrd)]
enum Exit {
    Unknown,
    Continue,
    Break,
    Return,
    Error,
}

impl Context<'_, '_> {
    pub(super) fn analyze(&mut self, node: NodeIndex) {
        let tree = self.tree;

        match &tree.node(node).kind {
            NodeKind::Assignment {
                targets, values, ..
            }
            | NodeKind::Local {
                bindings: targets,
                values,
                ..
            }
            | NodeKind::Constant {
                bindings: targets,
                values,
                ..
            } => {
                let targets = tree.list(targets);
                let values = tree.list(values);

                if self.active(&["UnbalancedAssignment"])
                    && let Some(last) = values.last()
                    && (values.len() > targets.len()
                        || (values.len() < targets.len()
                            && !expands(tree, last.node)
                            && !matches!(tree.node(last.node).kind, NodeKind::Nil { .. })))
                {
                    self.warn(
                        node,
                        "UnbalancedAssignment",
                        "assignment has a different number of values and bindings",
                    );
                }

                if matches!(tree.node(node).kind, NodeKind::Assignment { .. }) {
                    for (target, value) in targets.iter().zip(values) {
                        if self.active(&["self_assignment"])
                            && self.bindings().same(tree, target.node, value.node)
                        {
                            let span = tree.node(node).span;

                            let fixable = targets.len() == 1
                                && values.len() == 1
                                && self.bindings().references[target.node.get()].is_some()
                                && !tree.tokens.iter().any(|token| {
                                    matches!(
                                        token.kind,
                                        TokenKind::Comment | TokenKind::BlockComment
                                    ) && token.span.start < span.end
                                        && span.start < token.span.end
                                });

                            let edits = if fixable {
                                vec![Edit {
                                    range: span.start..span.end,
                                    expected: self.source[span.start..span.end].to_owned(),
                                    replacement: ";".to_owned(),
                                }]
                            } else {
                                Vec::new()
                            };

                            self.emit(
                                if fixable { node } else { target.node },
                                "self_assignment",
                                "assignment leaves a value unchanged",
                                edits,
                            );
                        }
                    }

                    if self.active(&["string_concat_in_loop"])
                        && let ([target], [value]) = (targets, values)
                        && self.bindings().loops[node.get()].is_some()
                        && let NodeKind::Binary { left, operator, .. } =
                            tree.node(unwrap(tree, value.node)).kind
                        && tree.token(operator).kind == TokenKind::Symbol(Symbol::Concatenate)
                        && self.bindings().same(tree, target.node, left)
                    {
                        self.warn(
                            node,
                            "string_concat_in_loop",
                            "repeated concatenation grows a string quadratically",
                        );
                    }
                }
            }

            NodeKind::CompoundAssignment { operator, .. }
                if self.active(&["string_concat_in_loop"])
                    && self.bindings().loops[node.get()].is_some()
                    && tree.token(*operator).kind
                        == TokenKind::Symbol(Symbol::ConcatenateAssignment) =>
            {
                self.warn(
                    node,
                    "string_concat_in_loop",
                    "repeated concatenation grows a string quadratically",
                );
            }

            NodeKind::Block { statements }
                if self.active(&[
                    "DuplicateFunction",
                    "MultiLineStatement",
                    "SameLineStatement",
                    "almost_swapped",
                    "UnreachableCode",
                ]) =>
            {
                let statements = tree.list(statements);
                let mut functions = BTreeSet::new();
                let mut last_line = None;
                let mut terminated = false;

                for (index, statement) in statements.iter().enumerate() {
                    if self.active(&["DuplicateFunction"])
                        && let NodeKind::Function {
                            name: Some(name), ..
                        } = tree.node(statement.node).kind
                        && !functions.insert(text(tree, name).replace(':', "."))
                    {
                        self.warn(
                            name,
                            "DuplicateFunction",
                            "function has already been defined in this block",
                        );
                    }

                    if self.active(&["MultiLineStatement"]) {
                        self.layout(statement.node);
                    }

                    if index == 0 {
                        continue;
                    }

                    let previous = statements[index - 1];

                    if self.active(&["almost_swapped"])
                        && let (
                            Some((first_target, first_value)),
                            Some((second_target, second_value)),
                        ) = (
                            assignment(tree, previous.node),
                            assignment(tree, statement.node),
                        )
                        && self.bindings().same(tree, first_target, second_value)
                        && self.bindings().same(tree, first_value, second_target)
                        && !self.bindings().same(tree, first_target, first_value)
                    {
                        self.warn(
                            statement.node,
                            "almost_swapped",
                            "sequential assignments overwrite a value before swapping it",
                        );
                    }

                    let line = self
                        .active(&["SameLineStatement"])
                        .then(|| self.line(tree.node(statement.node).span.start));

                    if let Some(line) = line
                        && line == self.line(tree.node(previous.node).span.end)
                        && last_line != Some(line)
                        && previous.separator.is_none()
                        && !(matches!(
                            tree.node(previous.node).kind,
                            NodeKind::Local { .. } | NodeKind::Constant { .. }
                        ) && matches!(tree.node(statement.node).kind, NodeKind::Do { .. }))
                    {
                        self.warn(
                            statement.node,
                            "SameLineStatement",
                            "a new statement is on the same line; add a semicolon to separate statements",
                        );

                        last_line = Some(line);
                    }

                    let exit = if self.active(&["UnreachableCode"]) && !terminated {
                        self.exit(previous.node)
                    } else {
                        Exit::Unknown
                    };

                    if !terminated && exit != Exit::Unknown {
                        if !(exit == Exit::Error
                            && matches!(tree.node(statement.node).kind, NodeKind::Return { .. })
                            && index + 1 == statements.len())
                        {
                            self.warn(
                                statement.node,
                                "UnreachableCode",
                                "previous statement always terminates control flow",
                            );
                        }

                        terminated = true;
                    }
                }
            }

            NodeKind::If {
                branches,
                otherwise,
                ..
            } if self.active(&["DuplicateCondition", "if_expression_assignment"]) => {
                if self.active(&["DuplicateCondition"]) {
                    let conditions: Vec<_> = tree
                        .list(branches)
                        .iter()
                        .filter_map(|entry| {
                            if let NodeKind::Branch { condition, .. } = tree.node(entry.node).kind {
                                Some(condition)
                            } else {
                                None
                            }
                        })
                        .collect();

                    self.duplicates(&conditions);
                }

                if self.active(&["if_expression_assignment"])
                    && let ([branch], Some(otherwise)) = (tree.list(branches), otherwise)
                    && let NodeKind::Branch { body, .. } = tree.node(branch.node).kind
                    && let (Some(first), Some(second)) =
                        (single(tree, body), single(tree, *otherwise))
                    && let (Some((first, _)), Some((second, _))) =
                        (assignment(tree, first), assignment(tree, second))
                    && self.bindings().references[first.get()].is_some()
                    && self.bindings().same(tree, first, second)
                {
                    self.warn(
                        node,
                        "if_expression_assignment",
                        "matching branch assignments can use an if expression",
                    );
                }
            }

            NodeKind::Conditional { .. } if self.active(&["DuplicateCondition"]) => {
                if !self.bindings().parents[node.get()].is_some_and(|parent| {
                    matches!(
                        tree.node(parent).kind,
                        NodeKind::Conditional { falsy, .. } if falsy == node
                    )
                }) {
                    let mut conditions = Vec::new();
                    let mut branch = node;

                    while let NodeKind::Conditional {
                        condition, falsy, ..
                    } = tree.node(branch).kind
                    {
                        conditions.push(condition);
                        branch = falsy;
                    }

                    self.duplicates(&conditions);
                }
            }

            NodeKind::Binary {
                left,
                operator,
                right,
            } if self.active(&[
                "ComparisonPrecedence",
                "UnknownType",
                "MisleadingAndOr",
                "DuplicateCondition",
            ]) =>
            {
                self.binary(node, *left, tree.token(*operator).kind, *right);
            }

            NodeKind::Call { callee, arguments }
                if self.active(&[
                    "type_check_inside_call",
                    "restricted_module_paths",
                    "non_const_require",
                    "roblox_incorrect_color3_new_bounds",
                    "roblox_suspicious_udim2_new",
                    "roblox_manual_fromscale_or_fromoffset",
                    "loop_invariant_call",
                ]) =>
            {
                self.call(node, *callee, *arguments);
            }

            NodeKind::MethodCall {
                receiver, method, ..
            } if self.active(&["roblox_prefer_get_players"])
                && tree.text(*method) == b"GetChildren" =>
            {
                let receiver = self.bindings().value(tree, *receiver);

                if let NodeKind::MethodCall {
                    receiver,
                    method,
                    arguments,
                    ..
                } = tree.node(receiver).kind
                    && tree.text(method) == b"GetService"
                    && self.bindings().global(tree, receiver, "game")
                    && let [argument] = arguments_list(tree, arguments).as_slice()
                    && string(tree, *argument).as_deref() == Some(b"Players")
                {
                    self.warn(
                        node,
                        "roblox_prefer_get_players",
                        "use Players:GetPlayers() to select players",
                    );
                }
            }

            NodeKind::CallStatement { call }
                if self.active(&["ignored_pcall_result", "must_use"]) =>
            {
                if let NodeKind::Call { callee, .. } = tree.node(unwrap(tree, *call)).kind {
                    let path = self.bindings().path(tree, callee);

                    if self.active(&["ignored_pcall_result"])
                        && (self.bindings().global(tree, callee, "pcall")
                            || self.bindings().global(tree, callee, "xpcall"))
                    {
                        self.warn(
                            *call,
                            "ignored_pcall_result",
                            "protected-call result is discarded",
                        );
                    }

                    if self.active(&["must_use"])
                        && RESULTS.contains(&path.as_str())
                        && self.bindings().global(tree, callee, &path)
                    {
                        self.warn(
                            *call,
                            "must_use",
                            "result of a result-producing function is discarded",
                        );
                    }
                }
            }

            NodeKind::NumericFor {
                start,
                end,
                step: None,
                ..
            } if self.active(&["ForRange"]) => {
                let start_number = number(tree, *start);
                let end_number = number(tree, *end);

                let length = |node| {
                    matches!(
                        tree.node(node).kind,
                        NodeKind::Unary { operator, .. }
                            if tree.token(operator).kind == TokenKind::Symbol(Symbol::Length)
                    )
                };

                if (length(*start) && (end_number == Some(0.0) || end_number == Some(1.0)))
                    || start_number.zip(end_number).is_some_and(|(start, end)| {
                        start > end
                            || (start + (end - start).floor()).partial_cmp(&end)
                                != Some(Ordering::Equal)
                    })
                    || (start_number == Some(0.0) && length(*end))
                {
                    self.warn(
                        node,
                        "ForRange",
                        "numeric loop bounds require an explicit step or a one-based starting index",
                    );
                }
            }

            NodeKind::GenericFor {
                bindings,
                values,
                body,
                ..
            } if self.active(&["manual_table_clone"]) => {
                if let ([key, value], [iterator], Some(statement)) =
                    (tree.list(bindings), tree.list(values), single(tree, *body))
                    && let NodeKind::Call { callee, arguments } =
                        tree.node(unwrap(tree, iterator.node)).kind
                    && self.bindings().global(tree, callee, "pairs")
                    && arguments_list(tree, arguments).len() == 1
                    && let Some((target, assigned)) = assignment(tree, statement)
                    && let NodeKind::Index {
                        receiver,
                        key: indexed,
                        ..
                    } = tree.node(target).kind
                    && self.bindings().references[indexed.get()]
                        == self.bindings().references[key.node.get()]
                    && self.bindings().references[assigned.get()]
                        == self.bindings().references[value.node.get()]
                    && let Some(destination) = self.bindings().references[receiver.get()]
                    && !self.bindings().locals[destination].assigned
                    && let Some(initializer) = self.bindings().locals[destination].value
                    && matches!(
                        &tree.node(unwrap(tree, initializer)).kind,
                        NodeKind::Table { fields, .. } if tree.list(fields).is_empty()
                    )
                {
                    self.warn(
                        node,
                        "manual_table_clone",
                        "table-copy loop can use table.clone",
                    );
                }
            }

            NodeKind::Table { fields, .. } | NodeKind::TypeTable { fields, .. }
                if self.active(&["TableLiteral"]) =>
            {
                self.table(fields);
            }

            NodeKind::Function {
                body: Some(body), ..
            } if self.active(&["ImplicitReturn"]) => {
                if self.exit(*body) == Exit::Unknown
                    && self.value_return(*body)
                    && self.falls_through(*body)
                {
                    self.warn(
                        node,
                        "ImplicitReturn",
                        "function can implicitly return no values despite an explicit value return",
                    );
                }
            }

            NodeKind::Number { .. } if self.active(&["IntegerParsing"]) => {
                let literal = text(tree, node).replace('_', "").to_ascii_lowercase();
                let integer = literal.strip_suffix('i');
                let digits = integer.unwrap_or(&literal);

                let (digits, radix) = if let Some(digits) = digits.strip_prefix("0x") {
                    (digits, 16)
                } else if let Some(digits) = digits.strip_prefix("0b") {
                    (digits, 2)
                } else {
                    (digits, 10)
                };

                if !digits.is_empty() && digits.chars().all(|digit| digit.is_digit(radix)) {
                    let parsed = u128::from_str_radix(digits, radix).ok();

                    let imprecise = if integer.is_some() {
                        parsed.is_none_or(|value| value > (1_u128 << 63))
                    } else {
                        parsed.is_none_or(|value| {
                            value > u128::from(u64::MAX)
                                || (value > (1_u128 << 53)
                                    && value.trailing_zeros() < 128 - value.leading_zeros() - 53)
                        })
                    };

                    if imprecise {
                        self.warn(
                            node,
                            "IntegerParsing",
                            "number literal exceeds its available precision or range",
                        );
                    }
                }
            }

            _ => {}
        }
    }

    pub(super) fn finish(&mut self) {
        let tree = self.tree;

        if self.active(&[
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
            for binding in &self.bindings().locals {
                let used = !binding.reads.is_empty() || !binding.type_reads.is_empty();

                let exported = binding.category == Category::Local
                    && self.bindings().exported(tree, binding.statement);

                let constant = matches!(
                    tree.node(binding.statement).kind,
                    NodeKind::Constant { .. }
                ) || matches!(tree.node(binding.statement).kind, NodeKind::Function { prefix, .. }
                    if exported || prefix.is_some_and(|prefix| tree.token(prefix).bytes(tree.source) == b"const"));

                let function =
                    matches!(tree.node(binding.statement).kind, NodeKind::Function { .. });

                let import = binding.value.is_some_and(|value| matches!(tree.node(unwrap(tree, value)).kind, NodeKind::Call { callee, .. } if self.bindings().global(tree, callee, "require")));

                if self.active(&["unused_variable"])
                    && !exported
                    && binding.reads.is_empty()
                    && !self.settings.ignored.is_match(&binding.name)
                    && (binding.category != Category::Parameter || self.settings.unused.parameters)
                    && (binding.category != Category::Loop || self.settings.unused.loop_variables)
                {
                    self.warn(
                        binding.declaration,
                        "unused_variable",
                        "binding is never read",
                    );
                }

                if self.active(&["LocalUnused", "FunctionUnused", "ImportUnused"])
                    && !used
                    && binding.category == Category::Local
                    && !binding.name.starts_with('_')
                {
                    self.warn(
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

                if self.active(&["DuplicateLocal", "LocalShadow"])
                    && let Some(shadow) = binding.shadow
                {
                    let shadow = &self.bindings().locals[shadow];

                    if shadow.statement == binding.statement && binding.name != "_" {
                        if self.active(&["DuplicateLocal"]) {
                            self.warn(
                                binding.declaration,
                                "DuplicateLocal",
                                "binding has already been declared in this statement",
                            );
                        }
                    } else if self.active(&["LocalShadow"])
                        && used
                        && shadow.function == binding.function
                        && !(function
                            && matches!(
                                tree.node(shadow.statement).kind,
                                NodeKind::Function { .. }
                            )
                            && self.settings.enabled("DuplicateFunction").is_some())
                    {
                        self.warn(
                            binding.declaration,
                            "LocalShadow",
                            "binding shadows a previous local declaration",
                        );
                    }
                } else if self.active(&["LocalShadow"])
                    && used
                    && self.bindings().globals.contains_key(&binding.name)
                    && !self.symbols.globals.contains(&binding.name)
                {
                    self.warn(
                        binding.declaration,
                        "LocalShadow",
                        "binding shadows a global used in this module",
                    );
                }

                if self.active(&["UninitializedLocal"])
                    && !binding.initialized
                    && !binding.assigned
                    && binding.category == Category::Local
                    && let Some(first) = binding.reads.first()
                {
                    self.warn(
                        *first,
                        "UninitializedLocal",
                        "binding is never initialized or assigned; initialize with nil to silence",
                    );
                }

                if let Some(value) = binding.value {
                    let keep_local = self.settings.immutable.mutated_tables_stay_local
                        && binding.mutated
                        && matches!(tree.node(unwrap(tree, value)).kind, NodeKind::Table { .. });

                    if self.active(&["prefer_const"])
                        && !(binding.assigned || constant || keep_local)
                        && binding.category == Category::Local
                    {
                        self.warn(
                            binding.declaration,
                            "prefer_const",
                            "unchanged local binding can be declared const",
                        );
                    }

                    if self.active(&["ignored_pcall_result"])
                        && binding.reads.is_empty()
                        && matches!(tree.node(unwrap(tree, value)).kind, NodeKind::Call { callee, .. } if self.bindings().global(tree, callee, "pcall") || self.bindings().global(tree, callee, "xpcall"))
                    {
                        self.warn(
                            binding.declaration,
                            "ignored_pcall_result",
                            "protected-call success status is never read",
                        );
                    }
                }
            }
        }

        if self.active(&[
            "global_usage",
            "UnknownGlobal",
            "BuiltinGlobalWrite",
            "unscoped_variables",
            "GlobalUsedAsLocal",
            "FunctionUnused",
        ]) {
            for (name, global) in &self.bindings().globals {
                let known =
                    self.symbols.globals.contains(name) || self.bindings().declared.contains(name);

                for reference in &global.references {
                    if self.active(&["global_usage"]) {
                        self.warn(*reference, "global_usage", "access to a global binding");
                    }

                    if self.active(&["UnknownGlobal"]) && !known && global.writes.is_empty() {
                        self.warn(
                            *reference,
                            "UnknownGlobal",
                            "unknown global; consider assigning to it first",
                        );
                    }
                }

                if self.active(&[if known {
                    "BuiltinGlobalWrite"
                } else {
                    "unscoped_variables"
                }]) {
                    for write in &global.writes {
                        self.warn(
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

                if self.active(&["GlobalUsedAsLocal"])
                    && !known
                    && name != "_"
                    && !global.writes.is_empty()
                    && (!global.functions.is_empty()
                        || (!global.read_before_write && !global.module_assignment))
                    && let Some(first) = global.references.first()
                {
                    self.warn(
                        *first,
                        "GlobalUsedAsLocal",
                        "global is only used locally; consider declaring a local",
                    );
                }

                if self.active(&["FunctionUnused"])
                    && let Some(definition) = global.definition
                    && !global.read
                    && !name.starts_with('_')
                {
                    self.warn(
                        definition,
                        "FunctionUnused",
                        "function is never used; prefix with `_` to silence",
                    );
                }
            }
        }

        if self.active(&["restricted_globals"]) && !self.settings.globals.is_empty() {
            for (name, global) in &self.bindings().globals {
                if let Some(reason) = self.settings.globals.get(name) {
                    let message = format!("global `{name}` is restricted: {reason}");

                    for reference in &global.references {
                        self.warn(*reference, "restricted_globals", &message);
                    }
                }
            }
        }

        if self.active(&["PlaceholderRead"]) {
            for index in 0..tree.nodes.len() {
                let node = NodeIndex::new(index);

                if self.bindings().reads[index] && tree.text(node) == b"_" {
                    self.warn(
                        node,
                        "PlaceholderRead",
                        "placeholder value `_` is read; use a named variable",
                    );
                }
            }
        }

        if self.active(&["CommentDirective", "RedundantNativeAttribute"]) {
            self.comments();
        }
    }

    fn warn(&mut self, node: NodeIndex, rule: &str, message: &str) {
        self.emit(node, rule, message, Vec::new());
    }

    fn line(&self, offset: usize) -> usize {
        self.lines()
            .partition_point(|start| *start <= offset)
            .saturating_sub(1)
    }

    fn layout(&mut self, statement: NodeIndex) {
        if matches!(self.tree.node(statement).kind, NodeKind::Repeat { .. }) {
            return;
        }

        let start = self.tree.node(statement).span.start;
        let mut line = self.line(start);
        let column = start - self.lines()[line];
        let mut pending = self.tree.children(statement);
        pending.reverse();

        while let Some(node) = pending.pop() {
            if matches!(
                self.tree.node(node).kind,
                NodeKind::Block { .. }
                    | NodeKind::Table { .. }
                    | NodeKind::TypeAlias { .. }
                    | NodeKind::TypeTable { .. }
                    | NodeKind::Returns { .. }
                    | NodeKind::Binding { .. }
                    | NodeKind::Parameters { .. }
                    | NodeKind::FunctionName { .. }
            ) {
                continue;
            }

            let offset = self.tree.node(node).span.start;
            let current = self.line(offset);

            if current > line {
                line = current;

                if offset - self.lines()[current] <= column {
                    self.warn(
                        node,
                        "MultiLineStatement",
                        "statement spans multiple lines; indent its continuation",
                    );

                    break;
                }
            }

            pending.extend(self.tree.children(node).into_iter().rev());
        }
    }

    fn call(&mut self, node: NodeIndex, callee: NodeIndex, arguments: NodeIndex) {
        let tree = self.tree;
        let arguments = arguments_list(tree, arguments);
        let path = self.bindings().path(tree, callee);

        if self.active(&["type_check_inside_call"])
            && (self.bindings().global(tree, callee, "type")
                || self.bindings().global(tree, callee, "typeof"))
            && let [argument] = arguments.as_slice()
            && matches!(tree.node(unwrap(tree, *argument)).kind, NodeKind::Binary { operator, .. } if comparison(tree.token(operator).kind))
        {
            self.warn(
                node,
                "type_check_inside_call",
                "comparison belongs outside the type check",
            );
        }

        if self.active(&["restricted_module_paths", "non_const_require"])
            && self.bindings().global(tree, callee, "require")
        {
            if let [argument] = arguments.as_slice() {
                let value = self.bindings().value(tree, *argument);

                if let Some(literal) = string(tree, value) {
                    if let Ok(path) = String::from_utf8(literal)
                        && let Some(reason) = self.settings.imports.get(&path)
                    {
                        self.warn(
                            *argument,
                            "restricted_module_paths",
                            &format!("module path `{path}` is restricted: {reason}"),
                        );
                    }
                } else if self.active(&["non_const_require"]) && !self.instance(value) {
                    self.warn(
                        node,
                        "non_const_require",
                        "require argument is not statically known",
                    );
                }
            } else {
                self.warn(
                    node,
                    "non_const_require",
                    "require argument is not statically known",
                );
            }
        }

        if self.active(&[
            "roblox_incorrect_color3_new_bounds",
            "roblox_suspicious_udim2_new",
            "roblox_manual_fromscale_or_fromoffset",
        ]) && let NodeKind::Field { receiver, .. } = tree.node(unwrap(tree, callee)).kind
        {
            let root = self.bindings().path(tree, receiver);

            if self.symbols.globals.contains(&root) && self.bindings().global(tree, callee, &path) {
                if self.active(&["roblox_incorrect_color3_new_bounds"])
                    && path == "Color3.new"
                    && arguments.iter().any(|argument| {
                        number(tree, *argument).is_some_and(|value| !(0.0..=1.0).contains(&value))
                    })
                {
                    self.warn(
                        node,
                        "roblox_incorrect_color3_new_bounds",
                        "Color3.new channels use a 0-1 scale",
                    );
                } else if path == "UDim2.new" {
                    if arguments.len() == 2 {
                        self.warn(
                            node,
                            "roblox_suspicious_udim2_new",
                            "UDim2.new needs scale and offset components for both axes",
                        );
                    } else if let [
                        horizontal_scale,
                        horizontal_offset,
                        vertical_scale,
                        vertical_offset,
                    ] = arguments.as_slice()
                        && self.active(&["roblox_manual_fromscale_or_fromoffset"])
                        && ((number(tree, *horizontal_offset) == Some(0.0)
                            && number(tree, *vertical_offset) == Some(0.0))
                            || (number(tree, *horizontal_scale) == Some(0.0)
                                && number(tree, *vertical_scale) == Some(0.0)))
                    {
                        self.warn(
                            node,
                            "roblox_manual_fromscale_or_fromoffset",
                            "use UDim2.fromScale or UDim2.fromOffset for zero components",
                        );
                    }
                }
            }
        }

        if self.active(&["loop_invariant_call"])
            && let Some(body) = self.bindings().loops[node.get()]
            && ARITHMETIC.contains(&path.as_str())
            && self.bindings().global(tree, callee, &path)
            && let [argument] = arguments.as_slice()
            && number(tree, self.bindings().value(tree, *argument)).is_some_and(f64::is_finite)
            && self.bindings().references[unwrap(tree, *argument).get()].is_none_or(|index| {
                tree.node(self.bindings().locals[index].declaration)
                    .span
                    .end
                    < tree.node(body).span.start
            })
        {
            self.warn(
                node,
                "loop_invariant_call",
                "deterministic numeric call has loop-invariant arguments",
            );
        }
    }

    fn instance(&self, node: NodeIndex) -> bool {
        let tree = self.tree;

        match tree.node(unwrap(tree, node)).kind {
            NodeKind::Name { .. } => matches!(
                self.bindings().path(tree, node).as_str(),
                "script" | "game" | "workspace"
            ),

            NodeKind::Field { receiver, .. } => self.instance(receiver),

            NodeKind::MethodCall {
                receiver,
                method,
                arguments,
                ..
            } => {
                matches!(
                    tree.text(method),
                    b"GetService" | b"WaitForChild" | b"FindFirstChild"
                ) && arguments_list(tree, arguments)
                    .first()
                    .is_some_and(|argument| string(tree, *argument).is_some())
                    && self.instance(receiver)
            }

            _ => false,
        }
    }

    fn binary(&mut self, node: NodeIndex, left: NodeIndex, operator: TokenKind, right: NodeIndex) {
        let tree = self.tree;

        if self.active(&["ComparisonPrecedence"]) && comparison(operator) {
            let negated = |node| matches!(tree.node(node).kind, NodeKind::Unary { operator, .. } if tree.token(operator).kind == TokenKind::Keyword(Keyword::Not));

            if (negated(left) && !negated(right))
                || matches!(tree.node(left).kind, NodeKind::Binary { operator, .. } if comparison(tree.token(operator).kind))
            {
                self.warn(node, "ComparisonPrecedence", "comparison precedence is misleading; add parentheses or separate the comparisons");
            }
        }

        if self.active(&["UnknownType"])
            && matches!(
                operator,
                TokenKind::Symbol(Symbol::Equal | Symbol::NotEqual)
            )
        {
            for (call, literal) in [(left, right), (right, left)] {
                if let NodeKind::Call { callee, .. } = tree.node(call).kind
                    && let Some(literal_text) = string(tree, literal)
                    && let Ok(name) = String::from_utf8(literal_text)
                {
                    let primitive = PRIMITIVES.contains(&name.as_str());
                    let valid = primitive || self.symbols.types.contains(&name);

                    if (self.bindings().global(tree, callee, "type") && !primitive)
                        || (self.bindings().global(tree, callee, "typeof") && !valid)
                    {
                        self.warn(literal, "UnknownType", &format!("unknown type `{name}`"));
                    }
                }
            }
        }

        if self.active(&["MisleadingAndOr"])
            && operator == TokenKind::Keyword(Keyword::Or)
            && let NodeKind::Binary {
                operator, right, ..
            } = tree.node(left).kind
            && tree.token(operator).kind == TokenKind::Keyword(Keyword::And)
            && matches!(
                tree.node(right).kind,
                NodeKind::Nil { .. } | NodeKind::Boolean { .. }
            )
            && truth(tree, right) == Some(false)
        {
            self.warn(
                node,
                "MisleadingAndOr",
                "and-or expression always evaluates to its second alternative",
            );
        }

        if self.active(&["DuplicateCondition"])
            && matches!(operator, TokenKind::Keyword(Keyword::And | Keyword::Or))
        {
            let parent = self.bindings().parents[node.get()];
            let in_chain = parent.is_some_and(|parent| matches!(tree.node(parent).kind, NodeKind::Binary { operator: parent_operator, .. } if tree.token(parent_operator).kind == operator));

            let ternary = parent.is_some_and(|parent| matches!(tree.node(parent).kind, NodeKind::Binary { operator: parent_operator, left: parent_left, .. } if parent_left == node && tree.token(parent_operator).kind == TokenKind::Keyword(Keyword::Or)))
                && operator == TokenKind::Keyword(Keyword::And)
                && ![left, right].iter().any(|child| matches!(tree.node(*child).kind, NodeKind::Binary { operator: child_operator, .. } if tree.token(child_operator).kind == operator));

            if !in_chain && !ternary {
                let mut conditions = Vec::new();
                chain(tree, node, operator, &mut conditions);
                self.duplicates(&conditions);
            }
        }
    }

    fn duplicates(&mut self, conditions: &[NodeIndex]) {
        for (index, condition) in conditions.iter().enumerate() {
            if conditions[index.saturating_sub(5)..index]
                .iter()
                .any(|previous| self.similar(*previous, *condition))
            {
                self.warn(
                    *condition,
                    "DuplicateCondition",
                    "condition has already been checked",
                );
            }
        }
    }

    fn similar(&self, left: NodeIndex, right: NodeIndex) -> bool {
        let tree = self.tree;
        let left_kind = &tree.node(left).kind;
        let right_kind = &tree.node(right).kind;

        if mem::discriminant(left_kind) != mem::discriminant(right_kind) {
            return false;
        }

        match (left_kind, right_kind) {
            (NodeKind::Name { .. }, _) => self.bindings().same(tree, left, right),

            (NodeKind::Number { .. }, _) => number(tree, left)
                .zip(number(tree, right))
                .is_some_and(|(left, right)| left.partial_cmp(&right) == Some(Ordering::Equal)),

            (NodeKind::String { .. }, _) => string(tree, left)
                .zip(string(tree, right))
                .is_some_and(|(left, right)| left == right),

            (NodeKind::Function { .. } | NodeKind::Assertion { .. }, _) => false,

            (
                NodeKind::Binary { operator: left, .. } | NodeKind::Unary { operator: left, .. },
                NodeKind::Binary {
                    operator: right, ..
                }
                | NodeKind::Unary {
                    operator: right, ..
                },
            ) if tree.token(*left).kind != tree.token(*right).kind => false,

            _ => {
                let left_children = tree.children(left);
                let right_children = tree.children(right);

                if left_children.is_empty() {
                    same(tree, left, right)
                } else {
                    left_children.len() == right_children.len()
                        && left_children
                            .iter()
                            .zip(right_children)
                            .all(|(left, right)| self.similar(*left, right))
                }
            }
        }
    }

    fn table(&mut self, fields: &NodeList) {
        let tree = self.tree;
        let fields = tree.list(fields);

        let count = fields
            .iter()
            .filter(|entry| {
                matches!(
                    tree.node(entry.node).kind,
                    NodeKind::TableField { key: None, .. }
                )
            })
            .count();

        let mut names = BTreeMap::<(bool, Vec<u8>), u8>::new();

        for field in fields {
            let (key, identifier, access) = match tree.node(field.node).kind {
                NodeKind::TableField {
                    key: Some(key),
                    opening,
                    ..
                } => (key, opening.is_none(), 3),

                NodeKind::TypeField { key, access, .. } => (
                    key,
                    matches!(tree.node(key).kind, NodeKind::Name { .. }),
                    access.map_or(3, |access| {
                        if tree.token(access).bytes(tree.source) == b"read" {
                            1
                        } else {
                            2
                        }
                    }),
                ),

                _ => continue,
            };

            let key_value = if identifier {
                Some((true, tree.text(key).to_vec()))
            } else if let Some(value) = string(tree, key) {
                Some((true, value))
            } else if let Some(value) = number(tree, key)
                .filter(|value| (0.0..=f64::from(i32::MAX)).contains(value) && value.fract() == 0.0)
            {
                if value >= 1.0 && value <= f64::from(u32::try_from(count).unwrap_or(u32::MAX)) {
                    self.warn(key, "TableLiteral", "table index duplicates a list entry");
                }

                Some((
                    false,
                    if value == 0.0 {
                        "0".to_owned()
                    } else {
                        value.to_string()
                    }
                    .into_bytes(),
                ))
            } else {
                None
            };

            if let Some(key_value) = key_value {
                let previous = names.entry(key_value).or_default();

                if *previous & access != 0 {
                    self.warn(key, "TableLiteral", "table field is a duplicate");
                }

                *previous |= access;
            }
        }
    }

    fn exit(&self, node: NodeIndex) -> Exit {
        let tree = self.tree;

        match &tree.node(node).kind {
            NodeKind::Block { statements } => tree
                .list(statements)
                .iter()
                .map(|entry| self.exit(entry.node))
                .find(|exit| *exit != Exit::Unknown)
                .unwrap_or(Exit::Unknown),

            NodeKind::Do { body, .. }
            | NodeKind::Else { body, .. }
            | NodeKind::Branch { body, .. } => self.exit(*body),

            NodeKind::If {
                branches,
                otherwise: Some(otherwise),
                ..
            } => tree
                .list(branches)
                .iter()
                .map(|entry| self.exit(entry.node))
                .chain([self.exit(*otherwise)])
                .min()
                .unwrap_or(Exit::Unknown),

            NodeKind::Return { .. } => Exit::Return,
            NodeKind::Break { .. } => Exit::Break,
            NodeKind::Continue { .. } => Exit::Continue,

            NodeKind::CallStatement { call } => {
                if let NodeKind::Call { callee, arguments } = tree.node(*call).kind
                    && (self.bindings().global(tree, callee, "error")
                        || (self.bindings().global(tree, callee, "assert")
                            && arguments_list(tree, arguments)
                                .first()
                                .is_some_and(|argument| truth(tree, *argument) == Some(false))))
                {
                    return Exit::Error;
                }

                Exit::Unknown
            }

            _ => Exit::Unknown,
        }
    }

    fn value_return(&self, node: NodeIndex) -> bool {
        match &self.tree.node(node).kind {
            NodeKind::Function { .. } => false,
            NodeKind::Return { values, .. } => !self.tree.list(values).is_empty(),

            _ => self
                .tree
                .children(node)
                .iter()
                .any(|child| self.value_return(*child)),
        }
    }

    fn falls_through(&self, node: NodeIndex) -> bool {
        match &self.tree.node(node).kind {
            NodeKind::Block { statements } => self
                .tree
                .list(statements)
                .iter()
                .all(|entry| self.falls_through(entry.node)),

            NodeKind::While {
                condition, body, ..
            } if truth(self.tree, *condition) == Some(true) => contains_break(self.tree, *body),

            NodeKind::If {
                branches,
                otherwise: Some(otherwise),
                ..
            } => {
                self.tree
                    .list(branches)
                    .iter()
                    .any(|entry| self.falls_through(entry.node))
                    || self.falls_through(*otherwise)
            }

            NodeKind::Do { body, .. }
            | NodeKind::Else { body, .. }
            | NodeKind::Branch { body, .. } => self.falls_through(*body),

            _ => self.exit(node) == Exit::Unknown,
        }
    }

    fn comments(&mut self) {
        let mut leading = true;
        let mut mode = false;
        let mut native = false;

        for token in &self.tree.tokens {
            if matches!(token.kind, TokenKind::Whitespace | TokenKind::EndOfFile) {
                continue;
            }

            if !matches!(token.kind, TokenKind::Comment | TokenKind::BlockComment) {
                leading = false;
                continue;
            }

            let Some(content) = token.bytes(self.tree.source).strip_prefix(b"--!") else {
                continue;
            };

            if content.is_empty() || content.first().is_some_and(u8::is_ascii_whitespace) {
                continue;
            }

            let content = String::from_utf8_lossy(content);
            let words: Vec<_> = content.split_whitespace().collect();

            let message = if leading {
                match words.as_slice() {
                    ["nolint", ..] | ["optimize", "0" | "1" | "2"] => None,

                    ["strict" | "nonstrict" | "nocheck"] => {
                        if mode {
                            Some("type checking mode was already specified")
                        } else {
                            mode = true;

                            None
                        }
                    }

                    ["strict" | "nonstrict" | "nocheck", ..] => {
                        Some("type checking directive has extra symbols")
                    }

                    ["optimize", ..] => Some("optimize directive requires a level from 0 to 2"),

                    ["native"] => {
                        native = true;

                        None
                    }

                    ["native", ..] => Some("native directive has extra symbols"),
                    _ => Some("unknown comment directive"),
                }
            } else {
                Some("comment directive appears after the first non-comment token")
            };

            if let Some(message) = message
                && let Some(severity) = self.settings.enabled("CommentDirective")
            {
                self.findings.push(Finding {
                    rule: "CommentDirective".to_owned(),
                    severity,
                    range: token.span.start..token.span.end,
                    message: message.to_owned(),
                    edits: Vec::new(),
                });
            }
        }

        if native && self.active(&["RedundantNativeAttribute"]) {
            for index in 0..self.tree.nodes.len() {
                let node = NodeIndex::new(index);

                if let NodeKind::Attribute { name, .. } = self.tree.node(node).kind
                    && matches!(self.tree.text(name), b"native" | b"@native")
                {
                    self.warn(
                        node,
                        "RedundantNativeAttribute",
                        "native attribute is redundant in a native module",
                    );
                }
            }
        }
    }
}

fn assignment(tree: &Tree<'_>, node: NodeIndex) -> Option<(NodeIndex, NodeIndex)> {
    if let NodeKind::Assignment {
        targets, values, ..
    } = &tree.node(node).kind
        && let ([target], [value]) = (tree.list(targets), tree.list(values))
    {
        Some((target.node, value.node))
    } else {
        None
    }
}

fn arguments_list(tree: &Tree<'_>, node: NodeIndex) -> Vec<NodeIndex> {
    match &tree.node(node).kind {
        NodeKind::Arguments { values, .. } => {
            tree.list(values).iter().map(|entry| entry.node).collect()
        }

        NodeKind::String { .. } | NodeKind::Table { .. } => vec![node],
        _ => Vec::new(),
    }
}

fn comparison(operator: TokenKind) -> bool {
    matches!(
        operator,
        TokenKind::Symbol(
            Symbol::Equal
                | Symbol::NotEqual
                | Symbol::LessThan
                | Symbol::LessThanOrEqual
                | Symbol::GreaterThan
                | Symbol::GreaterThanOrEqual
        )
    )
}

fn chain(tree: &Tree<'_>, node: NodeIndex, operator: TokenKind, conditions: &mut Vec<NodeIndex>) {
    match tree.node(node).kind {
        NodeKind::Binary {
            left,
            operator: current,
            right,
        } if tree.token(current).kind == operator => {
            chain(tree, left, operator, conditions);
            chain(tree, right, operator, conditions);
        }

        NodeKind::Group { expression, .. } => chain(tree, expression, operator, conditions),
        _ => conditions.push(node),
    }
}

fn contains_break(tree: &Tree<'_>, node: NodeIndex) -> bool {
    match tree.node(node).kind {
        NodeKind::Break { .. } => true,

        NodeKind::Function { .. }
        | NodeKind::While { .. }
        | NodeKind::Repeat { .. }
        | NodeKind::NumericFor { .. }
        | NodeKind::GenericFor { .. } => false,

        _ => tree
            .children(node)
            .iter()
            .any(|child| contains_break(tree, *child)),
    }
}

fn string(tree: &Tree<'_>, node: NodeIndex) -> Option<Vec<u8>> {
    let NodeKind::String { token } = tree.node(unwrap(tree, node)).kind else {
        return None;
    };

    let token = tree.token(token);
    let bytes = token.bytes(tree.source);

    if token.kind == TokenKind::RawString {
        return literal::decode(bytes);
    }

    if !matches!(
        token.kind,
        TokenKind::QuotedString | TokenKind::InterpolatedStringSimple
    ) {
        return None;
    }

    let mut input = bytes
        .get(1..bytes.len().checked_sub(1)?)?
        .iter()
        .copied()
        .peekable();

    let mut output = Vec::new();

    while let Some(byte) = input.next() {
        if byte != b'\\' {
            output.push(byte);
            continue;
        }

        let byte = input.next()?;

        output.push(match byte {
            b'a' => 7,
            b'b' => 8,
            b'f' => 12,
            b'n' => b'\n',
            b'r' => b'\r',
            b't' => b'\t',
            b'v' => 11,

            b'z' => {
                while input.peek().is_some_and(u8::is_ascii_whitespace) {
                    input.next();
                }

                continue;
            }

            b'\r' => {
                if input.peek() == Some(&b'\n') {
                    input.next();
                }

                b'\n'
            }

            b'x' => {
                let first = char::from(input.next()?).to_digit(16)?;
                let second = char::from(input.next()?).to_digit(16)?;

                u8::try_from(first * 16 + second).ok()?
            }

            b'u' => {
                if input.next()? != b'{' {
                    return None;
                }

                let mut value = 0_u32;

                loop {
                    let digit = input.next()?;

                    if digit == b'}' {
                        break;
                    }

                    value = value
                        .checked_mul(16)?
                        .checked_add(char::from(digit).to_digit(16)?)?;
                }

                let mut buffer = [0; 4];

                output
                    .extend_from_slice(char::from_u32(value)?.encode_utf8(&mut buffer).as_bytes());

                continue;
            }

            b'0'..=b'9' => {
                let mut value = u16::from(byte - b'0');

                for _ in 0..2 {
                    if let Some(digit) = input.next_if(u8::is_ascii_digit) {
                        value = value * 10 + u16::from(digit - b'0');
                    } else {
                        break;
                    }
                }

                u8::try_from(value).ok()?
            }

            b'\\' | b'\'' | b'"' | b'`' | b'{' | b'\n' => byte,
            _ => return None,
        });
    }

    Some(output)
}
