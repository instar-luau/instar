use vermis::token::{Keyword, Span, Symbol, TokenKind};

use crate::config::{BeforeFunctionParentheses, FormatOptions, Wrap};

use super::preparation::{FormatSyntax, Kind, Prepared, Token, delimiter_end, type_table};

pub(super) struct Delimiter {
    pub(super) kind: char,
    pub(super) wrapped: bool,
    pub(super) mode: Wrap,
    pub(super) call: bool,
    pub(super) parameters: bool,
    pub(super) hug_start: Option<usize>,
}

pub(super) struct Line {
    pub(super) break_before: bool,
    pub(super) continuation: usize,
    pub(super) conditional_depth: isize,
}

pub(super) struct Layout<'a> {
    prepared: &'a Prepared,
    options: &'a FormatOptions,
    expression_wraps: Vec<Option<bool>>,
    type_chain_wraps: Vec<Option<bool>>,
    conditional_events: Vec<isize>,
    conditional_depth: isize,
}

impl<'a> Layout<'a> {
    pub(super) fn new(prepared: &'a Prepared, options: &'a FormatOptions) -> Self {
        Self {
            prepared,
            options,
            expression_wraps: vec![None; prepared.tokens.len()],
            type_chain_wraps: vec![None; prepared.syntax.type_chains.len()],
            conditional_events: Vec::new(),
            conditional_depth: 0,
        }
    }

    pub(super) fn space(&self, previous: usize, current: usize) -> bool {
        token_needs_space(
            &self.prepared.tokens,
            previous,
            current,
            self.options,
            &self.prepared.syntax,
            &self.prepared.tight_type,
        )
    }

    pub(super) fn expression_start(&self, index: usize) -> Option<usize> {
        if_expression_start(&self.prepared.tokens, index, &self.prepared.syntax)
    }

    pub(super) fn delimiter(
        &self,
        index: usize,
        previous: Option<usize>,
        line_width: usize,
    ) -> Option<Delimiter> {
        let tokens = &self.prepared.tokens;
        let syntax = &self.prepared.syntax;
        let options = self.options;
        let tight_type = &self.prepared.tight_type;
        let text = tokens[index].text.as_str();
        let in_declaration = self.prepared.declared.get(index) == Some(&true);

        let opener = match text {
            "(" => Some('('),
            "{" => Some('{'),
            "[" => Some('['),
            _ => None,
        };

        let role = paren_role(&tokens[index], syntax);

        let open = opener?;
        let end = delimiter_end(tokens, index)?;
        let nonempty = end > index + 1;

        let function_parameters = role == ParenRole::Definition;
        let call = role == ParenRole::Call;

        let table_type = open == '{' && type_table(syntax, &tokens[index]);

        let mode = if open == '{' {
            if table_type {
                options.types.tables.wrap
            } else {
                options.tables.wrap
            }
        } else if open == '[' {
            Wrap::Never
        } else if function_parameters {
            options.parameters.wrap
        } else if call {
            options.calls.wrap
        } else {
            Wrap::Preserve
        };

        let hug_start = if call && options.calls.layout == crate::config::CallLayout::HugLast {
            hug_last_start(tokens, index, &syntax.last_arguments)
        } else {
            None
        };

        let content_end = hug_start.unwrap_or(end);

        let (multiline, mut width_end) = if call {
            call_content_layout(tokens, index, content_end, &syntax.function_arguments)
        } else {
            (
                tokens[index + 1..=content_end]
                    .iter()
                    .any(|token| token.newlines > 0),
                content_end,
            )
        };

        if function_parameters
            && let Ok(signature) = syntax
                .signature_ends
                .binary_search_by_key(&tokens[index].start, |&(start, _)| start)
        {
            let end = tokens
                .partition_point(|token| token.start < syntax.signature_ends[signature].1)
                - 1;

            if tokens[width_end + 1..=end]
                .iter()
                .all(|token| token.newlines == 0)
            {
                width_end = end;
            }
        }

        let over_width = nonempty
            && mode == Wrap::Auto
            && line_width
                .saturating_add(usize::from(previous.is_some_and(|p| {
                    token_needs_space(tokens, p, index, options, syntax, tight_type)
                })))
                .saturating_add(flat_width(
                    tokens, index, width_end, options, syntax, tight_type,
                ))
                > options.width;

        let wrapped = if open == '{' && mode == Wrap::Auto {
            multiline
                || over_width
                || tokens[end - 1].text == ","
                || (table_type && tokens[end - 1].text == ";")
        } else {
            wrap_break(
                mode,
                multiline,
                over_width || (in_declaration && function_parameters && multiline),
                nonempty,
            )
        };

        Some(Delimiter {
            kind: open,
            wrapped,
            mode,
            call: call && !function_parameters,
            parameters: open == '(' && function_parameters && !in_declaration,
            hug_start,
        })
    }

    pub(super) fn line(
        &mut self,
        index: usize,
        previous: Option<usize>,
        line_width: usize,
        parent: Option<&Delimiter>,
        opening: Option<&Delimiter>,
        expression_start: Option<usize>,
    ) -> Line {
        let prepared = self.prepared;
        let tokens = &prepared.tokens;
        let syntax = &prepared.syntax;
        let options = self.options;
        let text = tokens[index].text.as_str();
        let current_if_expression = expression_start.is_some();

        let hugged_argument = parent.is_some_and(|delimiter| {
            delimiter.call && !delimiter.wrapped && delimiter.hug_start == Some(index)
        });

        let block_closer = matches!(text, "end" | "until")
            || (matches!(text, "else" | "elseif") && !current_if_expression);

        let close_block = block_closer && !self.prepared.compact_ends[index];
        let close_delimiter = matches!(text, ")" | "}" | "]");
        let parent_wrapped = parent.is_some_and(|delimiter| delimiter.wrapped);
        let parent_mode = parent.map(|delimiter| delimiter.mode);
        let explicit_break = tokens[index].newlines > 0;

        let type_operator = self.type_operator(index);

        let chain = chain_step(tokens, index);

        let preserve_break = parent_mode.map_or_else(
            || {
                if current_if_expression {
                    matches!(options.if_expressions.wrap, Wrap::Preserve | Wrap::Always)
                } else if type_operator {
                    matches!(options.types.operators.wrap, Wrap::Preserve | Wrap::Always)
                } else if chain {
                    matches!(options.chains.wrap, Wrap::Preserve | Wrap::Always)
                } else {
                    true
                }
            },
            |mode| matches!(mode, Wrap::Preserve | Wrap::Always),
        );

        let mut continuation = 0;

        let mut line_break = close_block
            || (matches!(text, "else" | "elseif") && !current_if_expression)
            || (previous.is_some() && syntax.starts.binary_search(&tokens[index].start).is_ok())
            || (explicit_break
                && !hugged_argument
                && previous.is_some()
                && (preserve_break
                    || tokens[index].kind == Kind::Comment
                    || previous.is_some_and(|p| tokens[p].kind == Kind::Comment)));

        if let Some(indent) =
            self.conditional_continuation(index, previous, line_width, expression_start)
        {
            line_break = true;
            continuation = indent;
        }

        if type_operator && self.type_operator_wrap(index, previous, line_width, explicit_break) {
            line_break = true;
            continuation = 1;
        }

        if chain && self.chain_wrap(index, line_width, explicit_break) {
            line_break = true;
        }

        let hug_inline = hugged_argument
            || opening.or(parent).is_some_and(|delimiter| {
                delimiter.call && delimiter.hug_start.is_some_and(|start| index < start)
            });

        if index > 0
            && parent_wrapped
            && (matches!(tokens[index - 1].text.as_str(), "(" | "{")
                || close_delimiter
                || (tokens[index - 1].text == ","
                    && (syntax
                        .list_starts
                        .binary_search(&tokens[index].start)
                        .is_ok()
                        || tokens[index].kind == Kind::Comment)))
            && (!hug_inline || close_delimiter)
        {
            line_break = true;
        }

        if explicit_break
            && syntax
                .return_values
                .binary_search(&tokens[index].start)
                .is_ok()
        {
            line_break = true;
        }

        if chain && line_break {
            continuation = 1;
        }

        if explicit_break
            && syntax
                .binary_operators
                .binary_search(&tokens[index].start)
                .is_ok()
        {
            line_break = true;
            continuation = 1;
        }

        Line {
            break_before: line_break,
            continuation,
            conditional_depth: self.conditional_depth,
        }
    }

    fn conditional_continuation(
        &mut self,
        index: usize,
        previous: Option<usize>,
        line_width: usize,
        expression_start: Option<usize>,
    ) -> Option<usize> {
        let tokens = &self.prepared.tokens;
        let syntax = &self.prepared.syntax;
        let options = self.options;
        let tight_type = &self.prepared.tight_type;
        let text = tokens[index].text.as_str();

        let expression_wrap = expression_start.is_some_and(|start| {
            *self.expression_wraps[start].get_or_insert_with(|| {
                if_expression_wrap(tokens, start, line_width, options, syntax, tight_type)
            })
        });

        if expression_start == Some(index)
            && expression_wrap
            && options.if_expressions.layout == crate::config::IfExpressionLayout::Block
        {
            let branches = syntax
                .conditional_branches
                .partition_point(|&(root, _)| root < tokens[index].start);

            for &(_, span) in syntax.conditional_branches[branches..]
                .iter()
                .take_while(|&&(root, _)| root == tokens[index].start)
            {
                if self.conditional_events.is_empty() {
                    self.conditional_events.resize(tokens.len() + 1, 0);
                }

                let first = tokens.partition_point(|token| token.start < span.start);
                let end = tokens.partition_point(|token| token.start < span.end);

                let indent = 1 + isize::from(
                    options.if_expressions.placement
                        == crate::config::IfExpressionPlacement::NextLine,
                );

                self.conditional_events[first] += indent;
                self.conditional_events[end] -= indent;
            }
        }

        self.conditional_depth += self.conditional_events.get(index).copied().unwrap_or(0);

        let wrapped_branch = options.if_expressions.layout
            == crate::config::IfExpressionLayout::Block
            && previous.is_some_and(|p| {
                matches!(tokens[p].text.as_str(), "then" | "else")
                    && syntax
                        .conditionals
                        .iter()
                        .rev()
                        .filter(|span| span.start <= tokens[p].start && tokens[p].end <= span.end)
                        .find_map(|span| {
                            let start = tokens.partition_point(|token| token.start < span.start);

                            self.expression_wraps[start]
                        })
                        .unwrap_or(false)
            });

        if wrapped_branch
            || (expression_wrap
                && ((text == "if"
                    && options.if_expressions.placement
                        == crate::config::IfExpressionPlacement::NextLine)
                    || matches!(text, "else" | "elseif")
                    || (options.if_expressions.layout
                        == crate::config::IfExpressionLayout::Leading
                        && matches!(text, "then" | "else"))))
        {
            return Some(if wrapped_branch {
                usize::from(
                    text == "if"
                        && options.if_expressions.placement
                            == crate::config::IfExpressionPlacement::NextLine,
                )
            } else if text == "if" {
                1
            } else {
                usize::from(
                    options.if_expressions.placement
                        == crate::config::IfExpressionPlacement::NextLine,
                ) + usize::from(!matches!(text, "else" | "elseif"))
            });
        }

        None
    }

    fn type_operator(&self, index: usize) -> bool {
        let tokens = &self.prepared.tokens;
        let syntax = &self.prepared.syntax;
        let text = tokens[index].text.as_str();

        matches!(text, "|" | "&")
            && syntax
                .type_operator_gaps
                .partition_point(|&(start, _)| start <= tokens[index].start)
                .checked_sub(1)
                .is_some_and(|gap| tokens[index].end <= syntax.type_operator_gaps[gap].1)
    }

    fn type_operator_wrap(
        &mut self,
        index: usize,
        previous: Option<usize>,
        line_width: usize,
        explicit_break: bool,
    ) -> bool {
        let tokens = &self.prepared.tokens;
        let syntax = &self.prepared.syntax;
        let options = self.options;
        let tight_type = &self.prepared.tight_type;
        let text = tokens[index].text.as_str();

        match options.types.operators.wrap {
            Wrap::Always => true,
            Wrap::Never => false,
            Wrap::Preserve => explicit_break,

            Wrap::Auto if text == "|" => {
                let mut chain = syntax
                    .type_chains
                    .partition_point(|&(start, _)| start <= tokens[index].start);

                let mut over_width = line_width > options.width;

                while chain > 0 {
                    chain -= 1;
                    let (start, end) = syntax.type_chains[chain];

                    if tokens[index].end <= end {
                        over_width |= *self.type_chain_wraps[chain].get_or_insert_with(|| {
                            let first = tokens.partition_point(|token| token.start < start);
                            let last = tokens.partition_point(|token| token.start < end);

                            let multiline_operand =
                                tokens[first..last].iter().skip(1).any(|token| {
                                    token.newlines > 0 && !matches!(token.text.as_str(), "|" | "&")
                                });

                            if multiline_operand {
                                false
                            } else {
                                let remaining = flat_width(
                                    tokens,
                                    index,
                                    last - 1,
                                    options,
                                    syntax,
                                    tight_type,
                                );

                                line_width
                                    .saturating_add(usize::from(previous.is_some_and(|p| {
                                        token_needs_space(
                                            tokens, p, index, options, syntax, tight_type,
                                        )
                                    })))
                                    .saturating_add(remaining)
                                    > options.width
                            }
                        });

                        break;
                    }
                }

                over_width || explicit_break
            }

            Wrap::Auto => line_width > options.width || explicit_break,
        }
    }

    fn chain_wrap(&self, index: usize, line_width: usize, explicit_break: bool) -> bool {
        let tokens = &self.prepared.tokens;
        let syntax = &self.prepared.syntax;
        let options = self.options;

        let interpolation = syntax
            .interpolation_expressions
            .iter()
            .rev()
            .find(|span| span.start <= tokens[index].start && tokens[index].end <= span.end);

        let rest = (index + 3..tokens.len()).take_while(|&index| {
            tokens[index].newlines == 0
                && interpolation.is_none_or(|span| tokens[index].end <= span.end)
        });

        let more_calls = rest.clone().any(|index| chain_step(tokens, index));

        let previous_steps = (0..index)
            .rev()
            .take_while(|&index| {
                (tokens[index].newlines == 0 || chain_step(tokens, index))
                    && interpolation.is_none_or(|span| tokens[index].start >= span.start)
            })
            .filter(|&index| chain_step(tokens, index))
            .count();

        if more_calls || previous_steps > 0 {
            let mode = wrap_break(
                options.chains.wrap,
                explicit_break,
                interpolation.is_none() && line_width > options.width,
                true,
            ) || (options.chains.wrap == Wrap::Auto && explicit_break);

            let layout = match options.chains.layout {
                crate::config::ChainLayout::Full => true,
                crate::config::ChainLayout::Method => previous_steps > 0,
            };

            return mode && layout;
        }

        false
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ParenRole {
    Call,
    Definition,
    Group,
    TypeOf,
}

fn wrap_break(mode: Wrap, had_break: bool, over_width: bool, nonempty: bool) -> bool {
    match mode {
        Wrap::Always => nonempty,
        Wrap::Never => false,
        Wrap::Preserve => had_break,
        Wrap::Auto => over_width,
    }
}

fn chain_step(tokens: &[Token], index: usize) -> bool {
    matches!(tokens[index].text.as_str(), "." | ":")
        && tokens
            .get(index + 1)
            .is_some_and(|token| token.kind == Kind::Word)
        && tokens.get(index + 2).is_some_and(|token| token.text == "(")
}

fn if_expression_start(tokens: &[Token], index: usize, syntax: &FormatSyntax) -> Option<usize> {
    for start in (0..=index).rev() {
        if start < index
            && tokens[start].kind == Kind::Word
            && matches!(
                tokens[start].text.as_str(),
                "local" | "return" | "type" | "function"
            )
        {
            break;
        }

        if tokens[start].kind == Kind::Word && tokens[start].text == "end" {
            break;
        }

        if tokens[start].text == "if"
            && let Ok(conditional) = syntax
                .conditionals
                .binary_search_by_key(&tokens[start].start, |span| span.start)
            && tokens[index].end <= syntax.conditionals[conditional].end
        {
            return Some(start);
        }
    }

    if !matches!(tokens[index].text.as_str(), "then" | "else" | "elseif") {
        return None;
    }

    let conditional = syntax
        .conditionals
        .iter()
        .rev()
        .find(|span| span.start < tokens[index].start && tokens[index].end <= span.end)?;

    if syntax.statement_ifs.iter().any(|span| {
        span.start > conditional.start
            && span.start <= tokens[index].start
            && tokens[index].end <= span.end
    }) {
        return None;
    }

    Some(tokens.partition_point(|token| token.start < conditional.start))
}

fn if_expression_wrap(
    tokens: &[Token],
    start: usize,
    line_len: usize,
    options: &FormatOptions,
    syntax: &FormatSyntax,
    tight_type: &[bool],
) -> bool {
    let conditional = syntax
        .conditionals
        .binary_search_by_key(&tokens[start].start, |span| span.start)
        .expect("if-expression start recorded");

    let end = tokens
        .partition_point(|token| token.start < syntax.conditionals[conditional].end)
        .saturating_sub(1);

    let multiline = tokens[start + 1..=end]
        .iter()
        .any(|token| token.newlines > 0);

    let over_width = options.if_expressions.wrap == Wrap::Auto
        && line_len
            .saturating_add(usize::from(
                start > 0
                    && token_needs_space(tokens, start - 1, start, options, syntax, tight_type),
            ))
            .saturating_add(flat_width(tokens, start, end, options, syntax, tight_type))
            > options.width;

    wrap_break(
        options.if_expressions.wrap,
        multiline,
        over_width || multiline,
        end > start,
    )
}

fn is_spaced_operator(token: &Token) -> bool {
    matches!(
        token.syntax_kind,
        TokenKind::Symbol(
            Symbol::Assignment
                | Symbol::Add
                | Symbol::Subtract
                | Symbol::Multiply
                | Symbol::Divide
                | Symbol::FloorDivide
                | Symbol::Modulo
                | Symbol::Power
                | Symbol::Concatenate
                | Symbol::Equal
                | Symbol::NotEqual
                | Symbol::LessThan
                | Symbol::LessThanOrEqual
                | Symbol::GreaterThan
                | Symbol::GreaterThanOrEqual
                | Symbol::AddAssignment
                | Symbol::SubtractAssignment
                | Symbol::MultiplyAssignment
                | Symbol::DivideAssignment
                | Symbol::FloorDivideAssignment
                | Symbol::ModuloAssignment
                | Symbol::PowerAssignment
                | Symbol::ConcatenateAssignment
                | Symbol::Arrow
                | Symbol::DoubleColon
                | Symbol::Pipe
                | Symbol::Ampersand
                | Symbol::QuestionMark
        ) | TokenKind::Keyword(Keyword::And | Keyword::Or | Keyword::Not)
    )
}

fn needs_space(
    prev: &Token,
    current: &Token,
    next_role: ParenRole,
    options: &FormatOptions,
    role: ParenRole,
    tight_type_spacing: bool,
    unary_minus: bool,
) -> bool {
    let a = prev.text.as_str();
    let b = current.text.as_str();

    if prev.kind == Kind::Comment || current.kind == Kind::Comment {
        return true;
    }

    if matches!(
        prev.syntax_kind,
        TokenKind::InterpolatedStringStart | TokenKind::InterpolatedStringMiddle
    ) {
        return options.spacing.interpolation
            || current.syntax_kind == TokenKind::Symbol(Symbol::LeftBrace);
    }

    if matches!(
        current.syntax_kind,
        TokenKind::InterpolatedStringMiddle | TokenKind::InterpolatedStringEnd
    ) {
        return options.spacing.interpolation;
    }

    if unary_minus
        || tight_type_spacing
        || (a == ":"
            && current.kind == Kind::Word
            && matches!(next_role, ParenRole::Call | ParenRole::Definition))
    {
        return false;
    }

    if (a == "(" && b == ")") || (a == "[" && b == "]") {
        return false;
    }

    if a == "(" || b == ")" {
        return options.spacing.parentheses;
    }

    if a == "[" || b == "]" {
        return options.spacing.brackets;
    }

    if a == "." || matches!(b, "." | "," | ";") {
        return false;
    }

    if b == ":" {
        return false;
    }

    if b == "(" {
        return match role {
            ParenRole::TypeOf => false,

            ParenRole::Group => {
                is_spaced_operator(prev) || prev.kind == Kind::Word || matches!(a, ":" | "," | ";")
            }

            ParenRole::Call => matches!(
                options.spacing.before_function_parentheses,
                BeforeFunctionParentheses::Calls | BeforeFunctionParentheses::Always
            ),

            ParenRole::Definition => matches!(
                options.spacing.before_function_parentheses,
                BeforeFunctionParentheses::Definitions | BeforeFunctionParentheses::Always
            ),
        };
    }

    if a == "{" && b == "}" {
        return false;
    }

    if a == "{" {
        return options.spacing.braces;
    }

    if b == "}" {
        return options.spacing.braces;
    }

    if matches!(a, "," | ";") {
        return true;
    }

    if b == "[" {
        return false;
    }

    if matches!(a, ")" | "]" | "}")
        && matches!(current.kind, Kind::Word | Kind::Number | Kind::String)
    {
        return true;
    }

    if matches!(prev.kind, Kind::Word | Kind::Number | Kind::String)
        && matches!(current.kind, Kind::Word | Kind::Number | Kind::String)
    {
        return true;
    }

    if b == "{" && (prev.kind == Kind::Word || matches!(a, ")" | "]" | "}")) {
        return true;
    }

    if is_spaced_operator(prev) || is_spaced_operator(current) || a == ":" {
        return true;
    }

    (prev.kind == Kind::Word && current.syntax_kind == TokenKind::Symbol(Symbol::Length))
        || (prev.kind == Kind::Number && current.kind == Kind::Word)
}

fn paren_role(token: &Token, syntax: &FormatSyntax) -> ParenRole {
    if token.text != "(" {
        return ParenRole::Group;
    }

    if syntax.parameters.binary_search(&token.start).is_ok() {
        ParenRole::Definition
    } else if syntax
        .typeof_gaps
        .partition_point(|&(start, _)| start <= token.start)
        .checked_sub(1)
        .is_some_and(|gap| token.end <= syntax.typeof_gaps[gap].1)
    {
        ParenRole::TypeOf
    } else if syntax.calls.binary_search(&token.start).is_ok() {
        ParenRole::Call
    } else {
        ParenRole::Group
    }
}

fn token_needs_space(
    tokens: &[Token],
    previous: usize,
    current: usize,
    options: &FormatOptions,
    syntax: &FormatSyntax,
    tight_type: &[bool],
) -> bool {
    let prev = &tokens[previous];
    let token = &tokens[current];
    let text = token.text.as_str();

    let tight = (tight_type[previous] && prev.text == "<")
        || (tight_type[previous] && prev.text == ">" && text == "(")
        || (tight_type[current] && matches!(text, ">" | "?"))
        || (tight_type[current] && text == "<" && prev.kind == Kind::Word);

    (text == "[" && syntax.access_modifiers.binary_search(&prev.start).is_ok())
        || needs_space(
            prev,
            token,
            tokens
                .get(current + 1)
                .map_or(ParenRole::Group, |next| paren_role(next, syntax)),
            options,
            paren_role(token, syntax),
            tight,
            prev.syntax_kind == TokenKind::Symbol(Symbol::Subtract)
                && syntax.unary_minus.binary_search(&prev.start).is_ok(),
        )
}

fn flat_width(
    tokens: &[Token],
    start: usize,
    end: usize,
    options: &FormatOptions,
    syntax: &FormatSyntax,
    tight_type: &[bool],
) -> usize {
    let mut width = tokens[start].text.len();

    for current in start + 1..=end {
        width += tokens[current].text.len()
            + usize::from(token_needs_space(
                tokens,
                current - 1,
                current,
                options,
                syntax,
                tight_type,
            ));
    }

    width
}

fn call_content_layout(
    tokens: &[Token],
    start: usize,
    end: usize,
    function_arguments: &[(usize, Span)],
) -> (bool, usize) {
    let first = function_arguments.partition_point(|&(call, _)| call < tokens[start].start);
    let last = function_arguments.partition_point(|&(call, _)| call <= tokens[end].start);
    let functions = &function_arguments[first..last];
    let mut multiline = false;
    let mut width_end = end;

    for (index, token) in tokens.iter().enumerate().take(end + 1).skip(start + 1) {
        if token.newlines == 0 {
            continue;
        }

        if functions
            .iter()
            .any(|&(_, span)| span.start < token.start && token.end <= span.end)
        {
            width_end = width_end.min(index - 1);
        } else {
            multiline = true;
        }
    }

    (multiline, width_end)
}

fn hug_last_start(
    tokens: &[Token],
    start: usize,
    last_arguments: &[(usize, usize)],
) -> Option<usize> {
    let last = last_arguments
        .binary_search_by_key(&tokens[start].start, |&(list, _)| list)
        .ok()
        .map(|index| last_arguments[index].1)?;

    let index = tokens
        .partition_point(|token| token.start < last)
        .max(start + 1);

    let arg = tokens.get(index)?;

    (arg.text == "{"
        || arg.text == "function"
        || (arg.kind == Kind::String && arg.text.contains('\n')))
    .then_some(index)
}
