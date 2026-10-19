use std::{cell::OnceCell, cmp::Ordering, iter, str};

use vermis::{
    token::{Keyword, Symbol, TokenKind},
    tree::{NodeIndex, NodeKind, Tree},
};

use crate::{
    lint::{Edit, Error, Finding, Prepared, Symbols, bindings::Bindings},
    syntax,
};

pub(super) const RULES: &[&str] = &[
    "bad_string_escape",
    "compare_nan",
    "constant_condition",
    "constant_table_comparison",
    "length_as_condition",
    "zero_step_loop",
    "divide_by_zero",
    "empty_if",
    "empty_loop",
    "if_same_then_else",
    "mixed_table",
    "and_or_conditional",
    "collapsible_if",
    "else_after_return",
    "negated_condition",
    "parenthesized_conditions",
];

pub(super) struct Context<'tree, 'source> {
    pub(super) tree: &'tree Tree<'source>,
    pub(super) source: &'source str,
    pub(super) settings: &'tree Prepared,
    pub(super) symbols: &'tree Symbols,
    bindings: &'tree OnceCell<Bindings>,
    pub(super) findings: Vec<Finding>,
    lines: &'tree OnceCell<Vec<usize>>,
}

impl<'tree> Context<'tree, '_> {
    pub(super) fn active(&self, rules: &[&str]) -> bool {
        rules
            .iter()
            .any(|rule| self.settings.enabled(rule).is_some())
    }

    pub(super) fn bindings(&self) -> &'tree Bindings {
        self.bindings.get_or_init(|| Bindings::new(self.tree))
    }

    pub(super) fn lines(&self) -> &'tree [usize] {
        self.lines.get_or_init(|| {
            iter::once(0)
                .chain(
                    self.source
                        .match_indices('\n')
                        .map(|(offset, _)| offset + 1),
                )
                .collect()
        })
    }

    pub(super) fn emit(&mut self, node: NodeIndex, rule: &str, message: &str, edits: Vec<Edit>) {
        if let Some(severity) = self.settings.enabled(rule) {
            let span = self.tree.node(node).span;

            self.findings.push(Finding {
                rule: rule.to_owned(),
                severity,
                range: span.start..span.end,
                message: message.to_owned(),
                edits,
            });
        }
    }

    fn condition(&mut self, condition: NodeIndex) {
        let tree = self.tree;
        let unwrapped = unwrap(tree, condition);

        if self.active(&["constant_condition"]) && truth(tree, unwrapped).is_some() {
            self.emit(
                condition,
                "constant_condition",
                "condition has a fixed truth value",
                Vec::new(),
            );
        }

        if self.active(&["length_as_condition"])
            && matches!(&tree.node(unwrapped).kind, NodeKind::Unary { operator, .. }
                if tree.token(*operator).kind == TokenKind::Symbol(Symbol::Length))
        {
            self.emit(
                condition,
                "length_as_condition",
                "zero is truthy in Luau; compare the length explicitly",
                Vec::new(),
            );
        }

        if self.active(&["parenthesized_conditions"])
            && matches!(tree.node(condition).kind, NodeKind::Group { .. })
        {
            let mut edits = Vec::new();
            let mut inner = condition;

            while let NodeKind::Group {
                opening,
                expression,
                closing: Some(closing),
            } = &tree.node(inner).kind
            {
                for token in [opening, closing] {
                    let span = tree.token(*token).span;

                    edits.push(Edit {
                        range: span.start..span.end,
                        expected: self.source[span.start..span.end].to_owned(),
                        replacement: " ".to_owned(),
                    });
                }

                inner = *expression;
            }

            self.emit(
                condition,
                "parenthesized_conditions",
                "unnecessary parentheses around condition",
                edits,
            );
        }
    }

    fn visit(&mut self, node: NodeIndex) {
        let tree = self.tree;

        match &tree.node(node).kind {
            NodeKind::Branch { condition, .. }
            | NodeKind::While { condition, .. }
            | NodeKind::Repeat { condition, .. }
            | NodeKind::Conditional { condition, .. } => self.condition(*condition),

            _ => {}
        }

        match &tree.node(node).kind {
            NodeKind::String { token }
                if self.active(&["bad_string_escape"])
                    && tree.token(*token).kind != TokenKind::RawString =>
            {
                let interpolated = tree.token(*token).kind != TokenKind::QuotedString;
                let mut bytes = tree.token(*token).bytes(tree.source).iter();

                while let Some(byte) = bytes.next() {
                    if *byte == b'\\'
                        && let Some(escape) = bytes.next()
                        && !matches!(
                            escape,
                            b'a' | b'b'
                                | b'f'
                                | b'n'
                                | b'r'
                                | b't'
                                | b'v'
                                | b'z'
                                | b'x'
                                | b'u'
                                | b'0'
                                ..=b'9' | b'\\' | b'\'' | b'"' | b'\n' | b'\r'
                        )
                        && !(interpolated && matches!(escape, b'`' | b'{'))
                    {
                        self.emit(
                            node,
                            "bad_string_escape",
                            "unrecognized string escape",
                            Vec::new(),
                        );

                        break;
                    }
                }
            }

            NodeKind::NumericFor {
                step: Some(step), ..
            } if self.active(&["zero_step_loop"]) && number(tree, *step) == Some(0.0) => {
                self.emit(
                    *step,
                    "zero_step_loop",
                    "numeric loop step is zero",
                    Vec::new(),
                );
            }

            NodeKind::While { body, .. }
            | NodeKind::Repeat { body, .. }
            | NodeKind::NumericFor { body, .. }
            | NodeKind::GenericFor { body, .. }
                if self.active(&["empty_loop"]) && empty(tree, *body) =>
            {
                self.emit(node, "empty_loop", "empty loop body", Vec::new());
            }

            NodeKind::If {
                branches,
                otherwise,
                ..
            } if self.active(&[
                "empty_if",
                "if_same_then_else",
                "else_after_return",
                "negated_condition",
                "collapsible_if",
            ]) =>
            {
                for branch in tree.list(branches) {
                    if self.active(&["empty_if"])
                        && let NodeKind::Branch { body, .. } = tree.node(branch.node).kind
                        && empty(tree, body)
                    {
                        self.emit(
                            branch.node,
                            "empty_if",
                            "empty conditional branch",
                            Vec::new(),
                        );
                    }
                }

                if let Some(otherwise) = otherwise {
                    let bodies: Vec<_> = tree
                        .list(branches)
                        .iter()
                        .filter_map(|branch| {
                            if let NodeKind::Branch { body, .. } = tree.node(branch.node).kind {
                                Some(body)
                            } else {
                                None
                            }
                        })
                        .collect();

                    if self.active(&["empty_if"]) && empty(tree, *otherwise) {
                        self.emit(
                            *otherwise,
                            "empty_if",
                            "empty conditional branch",
                            Vec::new(),
                        );
                    }

                    if self.active(&["if_same_then_else"])
                        && let Some(first) = bodies.first()
                        && bodies.iter().all(|body| same(tree, *first, *body))
                        && same(tree, *first, body(tree, *otherwise))
                    {
                        self.emit(
                            node,
                            "if_same_then_else",
                            "conditional branches have identical bodies",
                            Vec::new(),
                        );
                    }

                    if self.active(&["else_after_return"])
                        && bodies.iter().all(|body| {
                            let NodeKind::Block { statements } = &tree.node(*body).kind else {
                                return false;
                            };

                            tree.list(statements).last().is_some_and(|statement| {
                                matches!(tree.node(statement.node).kind, NodeKind::Return { .. })
                            })
                        })
                    {
                        self.emit(
                            *otherwise,
                            "else_after_return",
                            "else follows branches that return",
                            Vec::new(),
                        );
                    }

                    if self.active(&["negated_condition"])
                        && let [branch] = tree.list(branches)
                        && let NodeKind::Branch { condition, .. } = tree.node(branch.node).kind
                        && matches!(&tree.node(unwrap(tree, condition)).kind, NodeKind::Unary { operator, .. }
                            if tree.token(*operator).kind == TokenKind::Keyword(Keyword::Not))
                    {
                        self.emit(
                            condition,
                            "negated_condition",
                            "negated condition can be expressed by swapping branches",
                            Vec::new(),
                        );
                    }
                } else if let [branch] = tree.list(branches)
                    && self.active(&["collapsible_if"])
                    && let NodeKind::Branch { body, .. } = tree.node(branch.node).kind
                    && let Some(inner) = single(tree, body)
                    && let NodeKind::If {
                        branches,
                        otherwise: None,
                        ..
                    } = &tree.node(inner).kind
                    && tree.list(branches).len() == 1
                {
                    self.emit(
                        node,
                        "collapsible_if",
                        "nested single-branch conditionals can be combined",
                        Vec::new(),
                    );
                }
            }

            NodeKind::Binary {
                left,
                operator,
                right,
            } => {
                let operator = tree.token(*operator).kind;

                if self.active(&["divide_by_zero"])
                    && matches!(
                        operator,
                        TokenKind::Symbol(Symbol::Divide | Symbol::FloorDivide | Symbol::Modulo)
                    )
                    && number(tree, *right) == Some(0.0)
                {
                    self.emit(
                        *right,
                        "divide_by_zero",
                        "division or modulo by zero",
                        Vec::new(),
                    );
                }

                if matches!(
                    operator,
                    TokenKind::Symbol(Symbol::Equal | Symbol::NotEqual)
                ) {
                    if self.active(&["compare_nan"])
                        && (number(tree, *left).is_some_and(f64::is_nan)
                            || number(tree, *right).is_some_and(f64::is_nan))
                    {
                        self.emit(
                            node,
                            "compare_nan",
                            "comparison with NaN has a fixed result",
                            Vec::new(),
                        );
                    }

                    if self.active(&["constant_table_comparison"])
                        && (matches!(tree.node(unwrap(tree, *left)).kind, NodeKind::Table { .. })
                            || matches!(
                                tree.node(unwrap(tree, *right)).kind,
                                NodeKind::Table { .. }
                            ))
                    {
                        self.emit(
                            node,
                            "constant_table_comparison",
                            "fresh tables are compared by identity",
                            Vec::new(),
                        );
                    }
                }

                if self.active(&["and_or_conditional"])
                    && operator == TokenKind::Keyword(Keyword::Or)
                    && let NodeKind::Binary {
                        operator, right, ..
                    } = &tree.node(unwrap(tree, *left)).kind
                    && tree.token(*operator).kind == TokenKind::Keyword(Keyword::And)
                    && truth(tree, *right) != Some(false)
                {
                    self.emit(
                        node,
                        "and_or_conditional",
                        "use an if expression instead of an and/or conditional",
                        Vec::new(),
                    );
                }
            }

            NodeKind::Table { fields, .. } if self.active(&["mixed_table"]) => {
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

                if positional && keyed {
                    self.emit(
                        node,
                        "mixed_table",
                        "table mixes positional and keyed entries",
                        Vec::new(),
                    );
                }
            }

            _ => {}
        }
    }
}

/// Run syntax-only rules on a lossless source tree.
///
/// # Errors
/// Returns malformed or incomplete syntax without proposing corrections.
pub fn lint(source: &str, settings: &Prepared, symbols: &Symbols) -> Result<Vec<Finding>, Error> {
    let prefix = usize::from(source.starts_with('\u{feff}')) * 3;
    let text = &source[prefix..];

    let tree = parse(source)?;

    if settings.rules.is_empty() {
        return Ok(Vec::new());
    }

    let bindings = OnceCell::new();
    let lines = OnceCell::new();

    let mut context = Context {
        tree: &tree,
        source: text,
        settings,
        symbols,
        bindings: &bindings,
        findings: Vec::new(),
        lines: &lines,
    };

    for index in 0..tree.nodes.len() {
        if settings.syntax {
            context.visit(NodeIndex::new(index));
        }

        if settings.analysis {
            context.analyze(NodeIndex::new(index));
        }
    }

    if settings.analysis {
        context.finish();
    }

    for finding in &mut context.findings {
        finding.range.start += prefix;
        finding.range.end += prefix;

        for edit in &mut finding.edits {
            edit.range.start += prefix;
            edit.range.end += prefix;
        }
    }

    Ok(context.findings)
}

pub(crate) fn parse(source: &str) -> Result<Tree<'_>, Error> {
    let prefix = usize::from(source.starts_with('\u{feff}')) * 3;

    syntax::parse(&source.as_bytes()[prefix..], |range, message| Error {
        range: range.start + prefix..range.end + prefix,
        message: message.unwrap_or("incomplete syntax").to_owned(),
    })
}

pub(super) fn unwrap(tree: &Tree<'_>, mut node: NodeIndex) -> NodeIndex {
    while let NodeKind::Group { expression, .. }
    | NodeKind::Assertion { expression, .. }
    | NodeKind::Instantiate { expression, .. } = tree.node(node).kind
    {
        node = expression;
    }

    node
}

pub(super) fn number(tree: &Tree<'_>, node: NodeIndex) -> Option<f64> {
    match &tree.node(unwrap(tree, node)).kind {
        NodeKind::Unary { operator, operand }
            if tree.token(*operator).kind == TokenKind::Symbol(Symbol::Subtract) =>
        {
            number(tree, *operand).map(|value| -value)
        }

        NodeKind::Binary {
            left,
            operator,
            right,
        } => {
            let left = number(tree, *left)?;
            let right = number(tree, *right)?;

            match tree.token(*operator).kind {
                TokenKind::Symbol(Symbol::Add) => Some(left + right),
                TokenKind::Symbol(Symbol::Subtract) => Some(left - right),
                TokenKind::Symbol(Symbol::Multiply) => Some(left * right),
                TokenKind::Symbol(Symbol::Divide) => Some(left / right),
                TokenKind::Symbol(Symbol::FloorDivide) => Some((left / right).floor()),
                TokenKind::Symbol(Symbol::Modulo) => Some(left - right * (left / right).floor()),
                TokenKind::Symbol(Symbol::Power) => Some(left.powf(right)),
                _ => None,
            }
        }

        NodeKind::Number { token } => {
            let text = str::from_utf8(tree.token(*token).span.bytes(tree.source))
                .ok()?
                .replace('_', "")
                .to_ascii_lowercase();

            if text.ends_with('i') {
                return None;
            }

            if let Some(digits) = text.strip_prefix("0b") {
                return digits.chars().try_fold(0.0, |value, digit| {
                    digit
                        .to_digit(2)
                        .map(|digit| value * 2.0 + f64::from(digit))
                });
            }

            if let Some(digits) = text.strip_prefix("0x") {
                let (significand, exponent) =
                    digits
                        .split_once('p')
                        .map_or(Some((digits, 0)), |(value, exponent)| {
                            exponent
                                .parse::<i32>()
                                .ok()
                                .map(|exponent| (value, exponent))
                        })?;

                let mut value = 0.0;
                let mut scale = None;

                for digit in significand.chars() {
                    if digit == '.' && scale.is_none() {
                        scale = Some(1.0);
                    } else {
                        let digit = f64::from(digit.to_digit(16)?);

                        if let Some(scale) = &mut scale {
                            *scale /= 16.0;
                            value += digit * *scale;
                        } else {
                            value = value * 16.0 + digit;
                        }
                    }
                }

                Some(value * 2.0_f64.powi(exponent))
            } else {
                text.parse().ok()
            }
        }

        _ => None,
    }
}

pub(super) fn truth(tree: &Tree<'_>, node: NodeIndex) -> Option<bool> {
    match &tree.node(unwrap(tree, node)).kind {
        NodeKind::Nil { .. } => Some(false),

        NodeKind::Boolean { token } => {
            Some(tree.token(*token).kind == TokenKind::Keyword(Keyword::True))
        }

        NodeKind::Number { .. }
        | NodeKind::String { .. }
        | NodeKind::Table { .. }
        | NodeKind::Function { .. } => Some(true),

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
            let comparison = number(tree, *left)?.partial_cmp(&number(tree, *right)?);

            match tree.token(*operator).kind {
                TokenKind::Symbol(Symbol::Equal) => Some(comparison == Some(Ordering::Equal)),
                TokenKind::Symbol(Symbol::NotEqual) => Some(comparison != Some(Ordering::Equal)),
                TokenKind::Symbol(Symbol::LessThan) => Some(comparison == Some(Ordering::Less)),

                TokenKind::Symbol(Symbol::GreaterThan) => {
                    Some(comparison == Some(Ordering::Greater))
                }

                TokenKind::Symbol(Symbol::LessThanOrEqual) => {
                    Some(matches!(comparison, Some(Ordering::Less | Ordering::Equal)))
                }

                TokenKind::Symbol(Symbol::GreaterThanOrEqual) => Some(matches!(
                    comparison,
                    Some(Ordering::Greater | Ordering::Equal)
                )),

                _ => number(tree, node).map(|_| true),
            }
        }

        _ => number(tree, node).map(|_| true),
    }
}

pub(super) fn body(tree: &Tree<'_>, node: NodeIndex) -> NodeIndex {
    if let NodeKind::Else { body, .. } = tree.node(node).kind {
        body
    } else {
        node
    }
}

pub(super) fn single(tree: &Tree<'_>, node: NodeIndex) -> Option<NodeIndex> {
    let NodeKind::Block { statements } = &tree.node(body(tree, node)).kind else {
        return None;
    };

    let [statement] = tree.list(statements) else {
        return None;
    };

    Some(statement.node)
}

pub(super) fn same(tree: &Tree<'_>, left: NodeIndex, right: NodeIndex) -> bool {
    let tokens = |node: NodeIndex| {
        let range = &tree.node(node).tokens;

        tree.tokens[range.start.get()..range.end.get()]
            .iter()
            .filter(|token| {
                !matches!(
                    token.kind,
                    TokenKind::Whitespace | TokenKind::Comment | TokenKind::BlockComment
                )
            })
            .map(|token| (token.kind, token.span.bytes(tree.source)))
    };

    tokens(left).eq(tokens(right))
}

fn empty(tree: &Tree<'_>, node: NodeIndex) -> bool {
    matches!(&tree.node(body(tree, node)).kind, NodeKind::Block { statements } if tree.list(statements).is_empty())
}
