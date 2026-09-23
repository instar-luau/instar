//! Luau source formatting.

use std::{borrow::Cow, io, mem};

use vermis::{InterpolatedKind, Keyword, Lexer, Operator, Parts, Span, TokenKind, View};

use crate::config::{
    BeforeFunctionParentheses, CallParentheses, FormatOptions, IndentStyle, LeadingZero,
    LineEnding, QuoteStyle, Semicolons, TrailingComma, Wrap,
};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Word,
    Number,
    String,
    Symbol,
    Comment,
}

struct Token {
    text: String,
    kind: Kind,
    syntax_kind: TokenKind,
    start: usize,
    end: usize,
    newlines: usize,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ParenRole {
    Call,
    Definition,
    Group,
    TypeOf,
}

#[derive(Default)]
struct FormatSyntax {
    starts: Vec<usize>,
    ends: Vec<usize>,
    edges: Vec<usize>,
    declarations: Vec<(usize, usize)>,
    class_headers: Vec<(usize, usize)>,
    bare_calls: Vec<(usize, usize)>,
    calls: Vec<usize>,
    list_starts: Vec<usize>,
    function_arguments: Vec<(usize, Span)>,
    last_arguments: Vec<(usize, usize)>,
    return_values: Vec<usize>,
    return_spans: Vec<(Span, std::ops::Range<usize>)>,
    conditionals: Vec<Span>,
    conditional_branches: Vec<(usize, Span)>,
    statement_ifs: Vec<Span>,
    binary_operators: Vec<usize>,
    interpolation_expressions: Vec<Span>,
    unary_minus: Vec<usize>,
    type_tables: Vec<usize>,
    type_spans: Vec<(usize, usize)>,
    typeof_gaps: Vec<(usize, usize)>,
    type_operator_gaps: Vec<(usize, usize)>,
    type_chains: Vec<(usize, usize)>,
    optional_ends: Vec<usize>,
    outer_type_spans: Vec<(usize, usize)>,
    access_modifiers: Vec<usize>,
    parameters: Vec<usize>,
    signature_ends: Vec<(usize, usize)>,
}

fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

fn lex(input: &str) -> io::Result<Vec<Token>> {
    let bytes = input.as_bytes();
    let mut tokens = Vec::new();
    let mut newlines = 0;

    for token in Lexer::new(bytes) {
        match token.kind {
            TokenKind::Eof => break,

            TokenKind::Whitespace => {
                newlines += token.bytes(bytes).split(|&byte| byte == b'\n').count() - 1;
                continue;
            }

            TokenKind::Error(_) => {
                return Err(invalid(format!(
                    "invalid token at byte {}",
                    token.span.start
                )));
            }

            _ => {}
        }

        let kind = match token.kind {
            TokenKind::Name | TokenKind::Keyword(_) => Kind::Word,
            TokenKind::Number => Kind::Number,

            TokenKind::QuotedString | TokenKind::RawString | TokenKind::Interpolated(_) => {
                Kind::String
            }

            TokenKind::Comment | TokenKind::BlockComment | TokenKind::MarkupComment => {
                Kind::Comment
            }

            _ => Kind::Symbol,
        };

        tokens.push(Token {
            text: token
                .utf8(bytes)
                .map_err(|error| invalid(error.to_string()))?
                .to_owned(),
            kind,
            syntax_kind: token.kind,
            start: token.span.start,
            end: token.span.end,
            newlines: mem::take(&mut newlines),
        });
    }

    Ok(tokens)
}

fn collect_type_chain(
    node: View<'_, '_>,
    parent: Option<vermis::Kind>,
    types: vermis::Children<'_, '_>,
    syntax: &mut FormatSyntax,
) {
    let span = node.span();

    if parent != Some(node.kind()) {
        syntax.type_chains.push((span.start, span.end));
    }

    let mut previous = span.start;

    for member in types {
        let start = member.span().start;

        if previous < start {
            syntax.type_operator_gaps.push((previous, start));
        }

        previous = member.span().end;
    }
}

fn collect_list_metadata(node: View<'_, '_>, syntax: &mut FormatSyntax) {
    match node.parts() {
        Some(Parts::TypeTable { fields, .. }) => {
            syntax.type_tables.push(node.span().start);

            syntax
                .list_starts
                .extend(fields.map(|field| field.span().start));
        }

        Some(Parts::Table { fields } | Parts::Parameters { parameters: fields }) => syntax
            .list_starts
            .extend(fields.map(|field| field.span().start)),

        Some(Parts::Arguments { values }) => {
            let list_start = node.span().start;
            let mut last = None;

            for value in values {
                let span = value.span();
                syntax.list_starts.push(span.start);
                last = Some(span.start);

                if value.kind() == vermis::Kind::Function {
                    syntax.function_arguments.push((list_start, span));
                }
            }

            if let Some(last) = last {
                syntax.last_arguments.push((list_start, last));
            }
        }

        Some(Parts::Return { values }) => {
            let first = syntax.return_values.len();

            syntax
                .return_values
                .extend(values.map(|value| value.span().start));

            syntax
                .return_spans
                .push((node.span(), first..syntax.return_values.len()));
        }

        _ => {}
    }
}

fn collect_conditional_branches(node: View<'_, '_>, syntax: &mut FormatSyntax) {
    if !node.text().starts_with(b"if") {
        return;
    }

    let root = node.span().start;
    let mut branch = node;

    while let Some(Parts::Conditional { truthy, falsy, .. }) = branch.parts() {
        syntax.conditional_branches.push((root, truthy.span()));

        if falsy.kind() == vermis::Kind::Conditional && falsy.text().starts_with(b"elseif") {
            branch = falsy;
        } else {
            syntax.conditional_branches.push((root, falsy.span()));
            break;
        }
    }
}

fn collect_signature_metadata(node: View<'_, '_>, syntax: &mut FormatSyntax) {
    if let Some(Parts::Function {
        parameters: args,
        returns,
        ..
    }) = node.parts()
    {
        syntax.parameters.push(args.span().start);

        if let Some(annotation) = returns {
            syntax
                .signature_ends
                .push((args.span().start, annotation.span().end));
        }
    }
}

fn body_starts(
    node: View<'_, '_>,
    parent: Option<vermis::Kind>,
    options: &FormatOptions,
    syntax: &mut FormatSyntax,
) {
    collect_list_metadata(node, syntax);
    collect_signature_metadata(node, syntax);

    if node.kind() == vermis::Kind::Declaration {
        let span = node.span();
        syntax.declarations.push((span.start, span.end));
    }

    if parent == Some(vermis::Kind::Declaration)
        && let Some(Parts::Class {
            name,
            extends,
            mut members,
        }) = node.parts()
    {
        let start = extends.map_or(name.span().end, |node| node.span().end);

        let end = members
            .next()
            .map_or(node.span().end, |member| member.span().start);

        syntax.class_headers.push((start, end));
    }

    match node.parts() {
        Some(Parts::TypeArguments { .. } | Parts::Generics { .. }) => {
            let span = node.span();
            syntax.type_spans.push((span.start, span.end));
        }

        Some(Parts::TypeOf { name, expression }) => syntax
            .typeof_gaps
            .push((name.span().end, expression.span().start)),

        Some(Parts::TypeUnion { types } | Parts::TypeIntersection { types }) => {
            collect_type_chain(node, parent, types, syntax);
        }

        Some(Parts::Conditional { .. }) => {
            syntax.conditionals.push(node.span());
            collect_conditional_branches(node, syntax);
        }

        Some(Parts::If { .. }) => syntax.statement_ifs.push(node.span()),

        Some(Parts::Binary { operator, .. }) => {
            syntax.binary_operators.push(operator.span().start);
        }

        Some(Parts::Interpolation { segments }) => syntax.interpolation_expressions.extend(
            segments
                .filter(|segment| segment.kind() != vermis::Kind::String)
                .map(View::span),
        ),

        Some(Parts::Unary { operator, .. }) if operator.text() == b"-" => {
            syntax.unary_minus.push(operator.span().start);
        }

        Some(Parts::TypeOptional { .. }) => syntax.optional_ends.push(node.span().end),

        Some(
            Parts::Instantiate { arguments, .. }
            | Parts::MethodCall {
                types: Some(arguments),
                ..
            },
        ) => {
            let span = arguments.span();
            syntax.outer_type_spans.push((span.start, span.end));
        }

        Some(Parts::TypeField {
            access: Some(access),
            ..
        }) => syntax.access_modifiers.push(access.span().start),

        _ => {}
    }

    if let Some(Parts::Block { statements }) = node.parts() {
        let mut first = true;

        for statement in statements {
            if first {
                if parent != Some(vermis::Kind::Root) {
                    syntax.edges.push(statement.span().start);
                }

                first = false;
            }

            syntax.ends.push(statement.span().end);
            syntax.starts.push(statement.span().start);
        }
    }

    if let Some(arguments) = match node.parts() {
        Some(Parts::Call { arguments, .. } | Parts::MethodCall { arguments, .. }) => {
            Some(arguments)
        }

        _ => None,
    } {
        syntax.calls.push(arguments.span().start);

        if options.calls.parentheses == CallParentheses::Always
            && matches!(arguments.text().first(), Some(b'\'' | b'"' | b'[' | b'{'))
        {
            syntax
                .bare_calls
                .push((arguments.span().start, arguments.span().end));
        }
    }

    for child in node.children() {
        body_starts(child, Some(node.kind()), options, syntax);
    }
}

fn wrap_break(mode: Wrap, had_break: bool, over_width: bool, nonempty: bool) -> bool {
    match mode {
        Wrap::Always => nonempty,
        Wrap::Never => false,
        Wrap::Preserve => had_break,
        Wrap::Auto => over_width,
    }
}

fn quote(text: &str, style: QuoteStyle) -> String {
    if text.len() < 2
        || !matches!(text.as_bytes()[0], b'\'' | b'"')
        || style == QuoteStyle::Preserve
    {
        return text.to_owned();
    }

    let body = &text[1..text.len() - 1];

    let cost = |delimiter: u8| {
        let mut length = body.len();
        let mut escaped = false;

        for byte in body.bytes() {
            if escaped {
                if matches!(byte, b'\'' | b'"') && byte != delimiter {
                    length -= 1;
                }

                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == delimiter {
                length += 1;
            }
        }

        length
    };

    let delimiter = match style {
        QuoteStyle::Double => b'"',
        QuoteStyle::Single => b'\'',

        QuoteStyle::PreferDouble => {
            if cost(b'"') <= cost(b'\'') {
                b'"'
            } else {
                b'\''
            }
        }

        QuoteStyle::PreferSingle => {
            if cost(b'\'') <= cost(b'"') {
                b'\''
            } else {
                b'"'
            }
        }

        QuoteStyle::Preserve => unreachable!(),
    };

    let mut result = String::with_capacity(cost(delimiter) + 2);
    result.push(delimiter as char);
    let mut escaped = false;

    for ch in body.chars() {
        if escaped {
            if ch == delimiter as char || !matches!(ch, '\'' | '"') {
                result.push('\\');
            }

            result.push(ch);
            escaped = false;
        } else if ch == '\\' {
            escaped = true;
        } else {
            if ch == delimiter as char {
                result.push('\\');
            }

            result.push(ch);
        }
    }

    if escaped {
        result.push('\\');
    }

    result.push(delimiter as char);

    result
}

fn type_table(syntax: &FormatSyntax, opener: &Token) -> bool {
    syntax.type_tables.binary_search(&opener.start).is_ok()
}

fn chain_step(tokens: &[Token], index: usize) -> bool {
    matches!(tokens[index].text.as_str(), "." | ":")
        && tokens
            .get(index + 1)
            .is_some_and(|token| token.kind == Kind::Word)
        && tokens.get(index + 2).is_some_and(|token| token.text == "(")
}

fn enclosing_brace(tokens: &[Token], index: usize) -> Option<usize> {
    let mut depth = 0usize;

    for position in (0..index).rev() {
        match tokens[position].text.as_str() {
            "}" => depth += 1,
            "{" if depth == 0 => return Some(position),
            "{" => depth -= 1,
            _ => {}
        }
    }

    None
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

fn prepare_function_bodies(
    tokens: &mut [Token],
    options: &FormatOptions,
    starts: &mut Vec<usize>,
    declared: &[bool],
) -> Vec<bool> {
    let mut compact_ends = vec![false; tokens.len()];

    let compact = matches!(
        options.blocks.simple_bodies,
        crate::config::SimpleBodies::CompactFunctions | crate::config::SimpleBodies::CompactAll
    );

    for function in 0..tokens.len() {
        if tokens[function].kind != Kind::Word
            || tokens[function].text != "function"
            || declared.get(function) == Some(&true)
        {
            continue;
        }

        let Some(open) = (function + 1..tokens.len())
            .take_while(|&index| tokens[index].newlines == 0)
            .find(|&index| tokens[index].text == "(")
        else {
            continue;
        };

        let Some(parameters_end) = delimiter_end(tokens, open) else {
            continue;
        };

        let body = parameters_end + 1;

        if body >= tokens.len() || tokens[body].text == ":" {
            continue;
        }

        let Some(end) = (body..tokens.len())
            .find(|&index| tokens[index].kind == Kind::Word && tokens[index].text == "end")
        else {
            continue;
        };

        let multiple_statements = starts
            .iter()
            .any(|&start| start > tokens[body].start && start < tokens[end].start);

        let body_tokens = &tokens[body..end];

        if multiple_statements
            || body_tokens.is_empty()
            || body_tokens.iter().any(|token| {
                token.kind == Kind::Comment
                    || (token.kind == Kind::Word
                        && matches!(
                            token.text.as_str(),
                            "function" | "if" | "for" | "while" | "do" | "repeat" | "end"
                        ))
                    || token.text == ";"
            })
            || body_tokens
                .iter()
                .filter(|token| token.newlines > 0)
                .count()
                > 1
        {
            continue;
        }

        if compact {
            tokens[body].newlines = 0;

            if let Ok(start) = starts.binary_search(&tokens[body].start) {
                starts.remove(start);
            }

            compact_ends[end] = true;
        } else {
            tokens[body].newlines = tokens[body].newlines.max(1);
        }
    }

    compact_ends
}

fn prepare_conditional_bodies(
    tokens: &mut [Token],
    options: &FormatOptions,
    starts: &mut Vec<usize>,
    compact_ends: &mut [bool],
    conditionals: &[Span],
) {
    let compact_conditionals = matches!(
        options.blocks.simple_bodies,
        crate::config::SimpleBodies::CompactConditionals | crate::config::SimpleBodies::CompactAll
    );

    for conditional in 0..tokens.len() {
        if tokens[conditional].kind != Kind::Word
            || tokens[conditional].text != "if"
            || conditionals
                .binary_search_by_key(&tokens[conditional].start, |span| span.start)
                .is_ok()
        {
            continue;
        }

        let Some(then) = (conditional + 1..tokens.len())
            .take_while(|&index| !(tokens[index].kind == Kind::Word && tokens[index].text == "end"))
            .find(|&index| tokens[index].kind == Kind::Word && tokens[index].text == "then")
        else {
            continue;
        };

        let body = then + 1;

        let Some(end) = (body..tokens.len())
            .find(|&index| tokens[index].kind == Kind::Word && tokens[index].text == "end")
        else {
            continue;
        };

        let multiple_statements = starts
            .iter()
            .any(|&start| start > tokens[body].start && start < tokens[end].start);

        let body_tokens = &tokens[body..end];

        if multiple_statements
            || body_tokens.is_empty()
            || body_tokens.iter().any(|token| {
                token.kind == Kind::Comment
                    || matches!(
                        token.text.as_str(),
                        "function"
                            | "if"
                            | "for"
                            | "while"
                            | "do"
                            | "repeat"
                            | "else"
                            | "elseif"
                            | ";"
                    )
            })
            || body_tokens
                .iter()
                .filter(|token| token.newlines > 0)
                .count()
                > 1
        {
            continue;
        }

        if compact_conditionals {
            tokens[body].newlines = 0;

            if let Ok(start) = starts.binary_search(&tokens[body].start) {
                starts.remove(start);
            }

            compact_ends[end] = true;
        } else {
            tokens[body].newlines = tokens[body].newlines.max(1);
        }
    }
}

fn type_punctuation(
    tokens: &[Token],
    spans: &[(usize, usize)],
    optional_ends: &[usize],
    outer_spans: &[(usize, usize)],
) -> Vec<bool> {
    let mut tight = vec![false; tokens.len()];

    for &(start, end) in spans {
        let open = tokens.partition_point(|token| token.start < start);

        if tokens
            .get(open)
            .is_some_and(|token| token.start == start && token.text == "<")
        {
            tight[open] = true;
        }

        let after = tokens.partition_point(|token| token.start < end);

        if after > 0 && tokens[after - 1].end == end && tokens[after - 1].text == ">" {
            tight[after - 1] = true;
        }
    }

    for &end in optional_ends {
        let after = tokens.partition_point(|token| token.end <= end);

        if after > 0 && tokens[after - 1].end == end && tokens[after - 1].text == "?" {
            tight[after - 1] = true;
        }
    }

    for &(arguments_start, arguments_end) in outer_spans {
        let after_open = tokens.partition_point(|token| token.end <= arguments_start);

        if after_open > 0 && tokens[after_open - 1].text == "<" {
            tight[after_open - 1] = true;
        }

        let close = tokens.partition_point(|token| token.start < arguments_end);

        if tokens.get(close).is_some_and(|token| token.text == ">") {
            tight[close] = true;
        }
    }

    tight
}

fn is_spaced_operator(token: &Token) -> bool {
    match token.syntax_kind {
        TokenKind::Operator(Operator::Ellipsis) => false,

        TokenKind::Operator(_)
        | TokenKind::Byte(
            b'=' | b'+' | b'-' | b'*' | b'/' | b'%' | b'^' | b'<' | b'>' | b'|' | b'&' | b'?',
        )
        | TokenKind::Keyword(Keyword::And | Keyword::Or | Keyword::Not) => true,

        _ => false,
    }
}

fn needs_space(
    prev: &Token,
    current: &Token,
    next: Option<&Token>,
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
        TokenKind::Interpolated(InterpolatedKind::Begin | InterpolatedKind::Middle)
    ) {
        return options.spacing.interpolation || current.syntax_kind == TokenKind::Byte(b'{');
    }

    if matches!(
        current.syntax_kind,
        TokenKind::Interpolated(InterpolatedKind::Middle | InterpolatedKind::End)
    ) {
        return options.spacing.interpolation;
    }

    if unary_minus
        || tight_type_spacing
        || (a == ":" && current.kind == Kind::Word && next.is_some_and(|next| next.text == "("))
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

    (prev.kind == Kind::Word && current.syntax_kind == TokenKind::Byte(b'#'))
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
            tokens.get(current + 1),
            options,
            paren_role(token, syntax),
            tight,
            prev.syntax_kind == TokenKind::Byte(b'-')
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
    let last = function_arguments.partition_point(|&(call, _)| call <= tokens[start].start);
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

fn delimiter_end(tokens: &[Token], start: usize) -> Option<usize> {
    let (open, close) = match tokens.get(start)?.text.as_str() {
        "(" => ("(", ")"),
        "{" => ("{", "}"),
        "[" => ("[", "]"),
        _ => return None,
    };

    let mut depth = 0usize;

    for (index, token) in tokens.iter().enumerate().skip(start) {
        if token.text == open {
            depth += 1;
        } else if token.text == close {
            depth = depth.checked_sub(1)?;

            if depth == 0 {
                return Some(index);
            }
        }
    }

    None
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

fn normalize_calls(tokens: &mut Vec<Token>, style: CallParentheses, calls: &[usize]) {
    if !matches!(
        style,
        CallParentheses::OmitString | CallParentheses::OmitTable | CallParentheses::OmitLiteral
    ) {
        return;
    }

    let mut index = 1;

    while index + 2 < tokens.len() {
        if tokens[index].text != "(" || calls.binary_search(&tokens[index].start).is_err() {
            index += 1;
            continue;
        }

        let end = delimiter_end(tokens, index);

        let Some(end) = end else {
            index += 1;
            continue;
        };

        let literal = end == index + 2 && tokens[index + 1].kind == Kind::String;

        let table = end > index + 1
            && tokens[index + 1].text == "{"
            && delimiter_end(tokens, index + 1) == Some(end - 1);

        let omit = (literal
            && matches!(
                style,
                CallParentheses::OmitString | CallParentheses::OmitLiteral
            ))
            || (table
                && matches!(
                    style,
                    CallParentheses::OmitTable | CallParentheses::OmitLiteral
                ));

        if omit {
            if literal {
                tokens[index + 1].newlines = tokens[index].newlines;
            }

            tokens.remove(end);
            tokens.remove(index);

            if table {
                tokens[index].newlines = 0;
            }
        } else {
            index += 1;
        }
    }
}

fn return_continuation_levels(tokens: &[Token], syntax: &FormatSyntax) -> Vec<isize> {
    let mut levels = Vec::new();

    for (span, values) in &syntax.return_spans {
        let first_break = syntax.return_values[values.clone()]
            .iter()
            .find_map(|&start| {
                let index = tokens.partition_point(|token| token.start < start);

                (tokens[index].newlines > 0).then_some(index)
            });

        if let Some(first) = first_break {
            if levels.is_empty() {
                levels.resize(tokens.len() + 1, 0);
            }

            let end = tokens.partition_point(|token| token.start < span.end);
            levels[first] += 1;
            levels[end] -= 1;
        }
    }

    let mut active = 0;

    for level in &mut levels {
        active += *level;
        *level = active;
    }

    levels
}

#[expect(
    clippy::too_many_lines,
    reason = "one rendering pass keeps nested line and indentation state coherent"
)]
fn format_tokens(
    mut tokens: Vec<Token>,
    options: &FormatOptions,
    syntax: &mut FormatSyntax,
) -> String {
    for index in 0..tokens.len() {
        if matches!(tokens[index].text.as_str(), "," | ";")
            && enclosing_brace(&tokens, index)
                .is_some_and(|start| type_table(syntax, &tokens[start]))
        {
            tokens[index].text = match (tokens[index].text.as_str(), options.types.table_separator)
            {
                (",", crate::config::TypeTableSeparator::Semicolon) => ";".to_owned(),
                (";", crate::config::TypeTableSeparator::Comma) => ",".to_owned(),
                _ => tokens[index].text.clone(),
            };
        }
    }

    let mut statement_end = vec![false; tokens.len()];

    if options.semicolons == Semicolons::Always {
        for &end in &syntax.ends {
            let mut after = tokens.partition_point(|token| token.start <= end);

            while after > 0 && tokens[after - 1].end > end {
                after -= 1;
            }

            if after > 0 {
                statement_end[after - 1] = true;
            }
        }
    }

    let tight_type = type_punctuation(
        &tokens,
        &syntax.type_spans,
        &syntax.optional_ends,
        &syntax.outer_type_spans,
    );

    let mut declared = Vec::new();

    if !syntax.declarations.is_empty() {
        declared.resize(tokens.len(), false);

        for &(start, end) in &syntax.declarations {
            let first = tokens.partition_point(|token| token.start < start);
            let last = tokens.partition_point(|token| token.start < end);
            declared[first..last].fill(true);
        }
    }

    let mut compact_ends =
        prepare_function_bodies(&mut tokens, options, &mut syntax.starts, &declared);

    prepare_conditional_bodies(
        &mut tokens,
        options,
        &mut syntax.starts,
        &mut compact_ends,
        &syntax.conditionals,
    );

    for token in &mut tokens {
        if syntax.starts.binary_search(&token.start).is_ok() {
            token.newlines = token.newlines.max(1);
        }
    }

    for token in &mut tokens {
        if token.kind == Kind::String {
            token.text = quote(&token.text, options.quote_style);
        } else if token.kind == Kind::Number && options.leading_zero != LeadingZero::Preserve {
            if token.text.starts_with('.') && options.leading_zero == LeadingZero::Add {
                token.text.insert(0, '0');
            } else if token.text.starts_with("0.") && options.leading_zero == LeadingZero::Strip {
                token.text.remove(0);
            }
        }
    }

    let return_levels = return_continuation_levels(&tokens, syntax);
    syntax.return_values.sort_unstable();

    let mut output = String::new();
    let mut level = 0usize;
    let mut line_len = 0usize;
    let mut stack: Vec<(char, bool)> = Vec::new();
    let mut modes: Vec<Wrap> = Vec::new();
    let mut call_lists: Vec<bool> = Vec::new();
    let mut parameter_lists: Vec<bool> = Vec::new();
    let mut parameter_depth = 0usize;
    let mut hug_starts: Vec<Option<usize>> = Vec::new();
    let mut previous: Option<usize> = None;
    let mut block_stack: Vec<bool> = Vec::new();
    let mut expression_wraps = vec![None; tokens.len()];
    let mut type_chain_wraps = vec![None; syntax.type_chains.len()];
    let mut conditional_events = Vec::new();
    let mut conditional_depth = 0;

    for i in 0..tokens.len() {
        let text = tokens[i].text.as_str();
        let in_declaration = declared.get(i) == Some(&true);
        let expression_start = if_expression_start(&tokens, i, syntax);
        let current_if_expression = expression_start.is_some();

        let hugged_argument = call_lists.last() == Some(&true)
            && stack.last().is_some_and(|(_, wrapped)| !wrapped)
            && hug_starts.last().copied().flatten() == Some(i);

        let block_closer = matches!(text, "end" | "until")
            || (matches!(text, "else" | "elseif") && !current_if_expression);

        let close_block = block_closer && !compact_ends[i];
        let close_delimiter = matches!(text, ")" | "}" | "]");
        let is_wrapped_close = close_delimiter && stack.last().is_some_and(|(_, wrapped)| *wrapped);

        if block_closer && block_stack.pop().unwrap_or(false) {
            level = level.saturating_sub(1);
        }

        if is_wrapped_close {
            level = level.saturating_sub(1);
        }

        let parent_wrapped = stack.last().is_some_and(|(_, wrapped)| *wrapped);
        let parent_mode = modes.last().copied();

        let opener = match text {
            "(" => Some('('),
            "{" => Some('{'),
            "[" => Some('['),
            _ => None,
        };

        let mut open_wrapped = false;
        let role = paren_role(&tokens[i], syntax);

        if let Some(open) = opener
            && let Some(end) = delimiter_end(&tokens, i)
        {
            let nonempty = end > i + 1;

            let function_parameters = role == ParenRole::Definition;
            let call = role == ParenRole::Call;

            let table_type = open == '{' && type_table(syntax, &tokens[i]);

            let mode = if open == '{' {
                if table_type {
                    options.types.table_wrap
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
                hug_last_start(&tokens, i, &syntax.last_arguments)
            } else {
                None
            };

            let content_end = hug_start.unwrap_or(end);

            let (multiline, mut width_end) = if call {
                call_content_layout(&tokens, i, content_end, &syntax.function_arguments)
            } else {
                (
                    tokens[i + 1..=content_end]
                        .iter()
                        .any(|token| token.newlines > 0),
                    content_end,
                )
            };

            if function_parameters
                && let Ok(signature) = syntax
                    .signature_ends
                    .binary_search_by_key(&tokens[i].start, |&(start, _)| start)
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
                && line_len
                    .saturating_add(usize::from(previous.is_some_and(|p| {
                        token_needs_space(&tokens, p, i, options, syntax, &tight_type)
                    })))
                    .saturating_add(flat_width(
                        &tokens,
                        i,
                        width_end,
                        options,
                        syntax,
                        &tight_type,
                    ))
                    > options.width;

            open_wrapped = if open == '{' && mode == Wrap::Auto {
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

            stack.push((open, open_wrapped));
            modes.push(mode);
            call_lists.push(call && !function_parameters);
            hug_starts.push(hug_start);
            let parameters = open == '(' && function_parameters && !in_declaration;
            parameter_lists.push(parameters);

            if parameters {
                parameter_depth += 1;
            }
        }

        let explicit_break = tokens[i].newlines > 0;
        let in_wrapped_list = parent_wrapped;

        let type_operator = matches!(text, "|" | "&")
            && syntax
                .type_operator_gaps
                .partition_point(|&(start, _)| start <= tokens[i].start)
                .checked_sub(1)
                .is_some_and(|gap| tokens[i].end <= syntax.type_operator_gaps[gap].1);

        let chain = chain_step(&tokens, i);

        let preserve_break = parent_mode.map_or_else(
            || {
                if current_if_expression {
                    matches!(options.if_expressions.wrap, Wrap::Preserve | Wrap::Always)
                } else if type_operator {
                    matches!(options.types.operator_wrap, Wrap::Preserve | Wrap::Always)
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
            || (previous.is_some() && syntax.starts.binary_search(&tokens[i].start).is_ok())
            || (explicit_break
                && !hugged_argument
                && previous.is_some()
                && (preserve_break
                    || tokens[i].kind == Kind::Comment
                    || previous.is_some_and(|p| tokens[p].kind == Kind::Comment)));

        let expression_wrap = expression_start.is_some_and(|start| {
            *expression_wraps[start].get_or_insert_with(|| {
                if_expression_wrap(&tokens, start, line_len, options, syntax, &tight_type)
            })
        });

        if expression_start == Some(i)
            && expression_wrap
            && options.if_expressions.layout == crate::config::IfExpressionLayout::Block
        {
            let branches = syntax
                .conditional_branches
                .partition_point(|&(root, _)| root < tokens[i].start);

            for &(_, span) in syntax.conditional_branches[branches..]
                .iter()
                .take_while(|&&(root, _)| root == tokens[i].start)
            {
                if conditional_events.is_empty() {
                    conditional_events.resize(tokens.len() + 1, 0);
                }

                let first = tokens.partition_point(|token| token.start < span.start);
                let end = tokens.partition_point(|token| token.start < span.end);

                let indent = 1 + isize::from(
                    options.if_expressions.placement
                        == crate::config::IfExpressionPlacement::NextLine,
                );

                conditional_events[first] += indent;
                conditional_events[end] -= indent;
            }
        }

        conditional_depth += conditional_events.get(i).copied().unwrap_or(0);

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

                            expression_wraps[start]
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
            line_break = true;

            continuation = if wrapped_branch {
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
            };
        }

        if type_operator {
            let wrap = match options.types.operator_wrap {
                Wrap::Always => true,
                Wrap::Never => false,
                Wrap::Preserve => explicit_break,

                Wrap::Auto if text == "|" => {
                    let mut chain = syntax
                        .type_chains
                        .partition_point(|&(start, _)| start <= tokens[i].start);

                    let mut over_width = line_len > options.width;

                    while chain > 0 {
                        chain -= 1;
                        let (start, end) = syntax.type_chains[chain];

                        if tokens[i].end <= end {
                            over_width |= *type_chain_wraps[chain].get_or_insert_with(|| {
                                let first = tokens.partition_point(|token| token.start < start);
                                let last = tokens.partition_point(|token| token.start < end);

                                let multiline_operand =
                                    tokens[first..last].iter().skip(1).any(|token| {
                                        token.newlines > 0
                                            && !matches!(token.text.as_str(), "|" | "&")
                                    });

                                if multiline_operand {
                                    false
                                } else {
                                    let remaining = flat_width(
                                        &tokens,
                                        i,
                                        last - 1,
                                        options,
                                        syntax,
                                        &tight_type,
                                    );

                                    line_len
                                        .saturating_add(usize::from(previous.is_some_and(|p| {
                                            token_needs_space(
                                                &tokens,
                                                p,
                                                i,
                                                options,
                                                syntax,
                                                &tight_type,
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

                Wrap::Auto => line_len > options.width || explicit_break,
            };

            if wrap {
                line_break = true;
                continuation = 1;
            }
        }

        if chain {
            let interpolation = syntax
                .interpolation_expressions
                .iter()
                .rev()
                .find(|span| span.start <= tokens[i].start && tokens[i].end <= span.end);

            let rest = (i + 3..tokens.len()).take_while(|&index| {
                tokens[index].newlines == 0
                    && interpolation.is_none_or(|span| tokens[index].end <= span.end)
            });

            let more_calls = rest.clone().any(|index| chain_step(&tokens, index));

            let previous_steps = (0..i)
                .rev()
                .take_while(|&index| {
                    (tokens[index].newlines == 0 || chain_step(&tokens, index))
                        && interpolation.is_none_or(|span| tokens[index].start >= span.start)
                })
                .filter(|&index| chain_step(&tokens, index))
                .count();

            if more_calls || previous_steps > 0 {
                let mode = wrap_break(
                    options.chains.wrap,
                    explicit_break,
                    interpolation.is_none() && line_len > options.width,
                    true,
                ) || (options.chains.wrap == Wrap::Auto && explicit_break);

                let layout = match options.chains.layout {
                    crate::config::ChainLayout::Full => true,
                    crate::config::ChainLayout::Method => previous_steps > 0,
                };

                if mode && layout {
                    line_break = true;
                }
            }
        }

        let hug_inline = hugged_argument
            || (call_lists.last() == Some(&true)
                && hug_starts
                    .last()
                    .copied()
                    .flatten()
                    .is_some_and(|start| i < start));

        if i > 0
            && in_wrapped_list
            && (matches!(tokens[i - 1].text.as_str(), "(" | "{")
                || close_delimiter
                || (tokens[i - 1].text == ","
                    && (syntax.list_starts.binary_search(&tokens[i].start).is_ok()
                        || tokens[i].kind == Kind::Comment)))
            && (!hug_inline || close_delimiter)
        {
            line_break = true;
        }

        if explicit_break && syntax.return_values.binary_search(&tokens[i].start).is_ok() {
            line_break = true;
        }

        if chain && line_break {
            continuation = 1;
        }

        if explicit_break
            && syntax
                .binary_operators
                .binary_search(&tokens[i].start)
                .is_ok()
        {
            line_break = true;
            continuation = 1;
        }

        if text == "}"
            && stack
                .last()
                .is_some_and(|(kind, wrapped)| *kind == '{' && *wrapped)
            && options.tables.trailing_comma == TrailingComma::Multiline
            && !enclosing_brace(&tokens, i).is_some_and(|start| type_table(syntax, &tokens[start]))
            && previous.is_some_and(|p| tokens[p].text != "," && tokens[p].text != "{")
        {
            output.push(',');
        }

        if line_break && !output.is_empty() && !output.ends_with('\n') {
            output.push('\n');
            line_len = 0;
        }

        if explicit_break && tokens[i].newlines > 1 && !output.is_empty() {
            let in_table = stack.last().is_some_and(|(kind, _)| *kind == '{');

            let block_edge = !current_if_expression
                && (syntax.edges.binary_search(&tokens[i].start).is_ok()
                    || matches!(text, "end" | "else" | "elseif" | "until"));

            let preserve_gap = if in_table {
                options.tables.blank_lines == crate::config::TableBlankLines::Preserve
            } else if block_edge {
                options.blocks.edge_blank_lines == crate::config::EdgeBlankLines::Preserve
            } else {
                true
            };

            if preserve_gap {
                if !output.ends_with('\n') {
                    output.push('\n');
                }

                output.push('\n');
                line_len = 0;
            }
        }

        if output.ends_with('\n') || output.is_empty() {
            let visual_level = level.saturating_sub(parameter_depth)
                + usize::try_from(return_levels.get(i).copied().unwrap_or(0))
                    .expect("return continuation is nonnegative")
                + usize::try_from(conditional_depth)
                    .expect("conditional continuation is nonnegative")
                + if line_break { continuation } else { 0 };

            match options.indent_style {
                IndentStyle::Tabs => output.extend(std::iter::repeat_n('\t', visual_level)),

                IndentStyle::Spaces => output.extend(std::iter::repeat_n(
                    ' ',
                    visual_level.saturating_mul(options.indent_width),
                )),
            }

            line_len = visual_level.saturating_mul(options.indent_width);
        } else if let Some(p) = previous
            && token_needs_space(&tokens, p, i, options, syntax, &tight_type)
        {
            output.push(' ');
            line_len += 1;
        }

        if text == ","
            && stack.last().is_some_and(|(kind, _)| *kind == '{')
            && options.tables.trailing_comma == TrailingComma::Never
            && tokens.get(i + 1).is_some_and(|next| next.text == "}")
        {
            previous = Some(i);
            continue;
        }

        output.push_str(text);
        line_len += text.len();

        if options.semicolons == Semicolons::Always
            && statement_end[i]
            && text != ";"
            && tokens.get(i + 1).is_none_or(|next| next.text != ";")
        {
            output.push(';');
        }

        if open_wrapped {
            level += 1;
        }

        if text == ";" && enclosing_brace(&tokens, i).is_none() {
            if options.semicolons == Semicolons::Necessary
                && !tokens
                    .get(i + 1)
                    .is_some_and(|next| matches!(next.text.as_str(), "(" | "["))
            {
                output.pop();
            }

            if !output.ends_with('\n') {
                output.push('\n');
                line_len = 0;
            }
        }

        if (text == "function" && !in_declaration)
            || matches!(text, "do" | "repeat")
            || (text == "with"
                && syntax
                    .class_headers
                    .iter()
                    .any(|&(start, end)| start <= tokens[i].start && tokens[i].end <= end))
            || (matches!(text, "then" | "else") && !current_if_expression)
        {
            block_stack.push(true);
            level += 1;
        }

        if close_delimiter {
            stack.pop();
            modes.pop();
            call_lists.pop();

            if parameter_lists.pop() == Some(true) {
                parameter_depth -= 1;
            }

            hug_starts.pop();
        }

        if tokens[i].kind == Kind::Comment && !output.ends_with('\n') {
            output.push('\n');
            line_len = 0;
        }

        previous = Some(i);
    }

    let mut result = output.trim_end_matches([' ', '\t', '\n']).to_owned();

    if options.final_newline && !result.is_empty() {
        result.push('\n');
    }

    if result.contains("\r\n") {
        let mut normalized = String::with_capacity(result.len());

        for line in result.split_inclusive('\n') {
            if let Some(line) = line.strip_suffix('\n') {
                normalized.push_str(line.trim_end_matches('\r'));
                normalized.push('\n');
            } else {
                normalized.push_str(line);
            }
        }

        result = normalized;
    }

    if options.line_ending == LineEnding::CrLf {
        result = result.replace('\n', "\r\n");
    }

    result
}

/// Formats Luau source according to the project format options.
/// # Errors
/// Returns a syntax or require-ordering error for invalid or unformattable source.
pub fn source(input: &str, options: &FormatOptions) -> io::Result<String> {
    let parsed = vermis::parse(input.as_bytes());

    if let Some(error) = parsed.diagnostics.first() {
        return Err(invalid(format!(
            "{} at byte {}",
            error.message, error.span.start
        )));
    }

    let ordered = crate::require_order::sort(input, &options.requires, &parsed)?;
    let reparsed;

    let parsed_ordered = match &ordered {
        Cow::Borrowed(_) => &parsed,

        Cow::Owned(source) => {
            reparsed = vermis::parse(source.as_bytes());

            &reparsed
        }
    };

    let mut syntax = FormatSyntax::default();

    if let Some(root) = parsed_ordered.view(parsed_ordered.root) {
        body_starts(root, None, options, &mut syntax);
    }

    syntax.starts.sort_unstable();
    syntax.calls.sort_unstable();
    syntax.list_starts.sort_unstable();

    syntax
        .function_arguments
        .sort_unstable_by_key(|&(call, span)| (call, span.start));

    syntax
        .last_arguments
        .sort_unstable_by_key(|&(list, _)| list);

    syntax.edges.sort_unstable();
    syntax.declarations.sort_unstable();
    syntax.conditionals.sort_unstable_by_key(|span| span.start);

    syntax
        .conditional_branches
        .sort_unstable_by_key(|&(root, _)| root);

    syntax.unary_minus.sort_unstable();
    syntax.binary_operators.sort_unstable();

    syntax
        .interpolation_expressions
        .sort_unstable_by_key(|span| span.start);

    syntax.type_spans.sort_unstable();
    syntax.typeof_gaps.sort_unstable();
    syntax.type_operator_gaps.sort_unstable();
    syntax.type_chains.sort_unstable();
    syntax.type_tables.sort_unstable();
    syntax.optional_ends.sort_unstable();
    syntax.outer_type_spans.sort_unstable();
    syntax.parameters.sort_unstable();

    syntax
        .signature_ends
        .sort_unstable_by_key(|&(start, _)| start);

    syntax.access_modifiers.sort_unstable();
    let mut tokens = lex(&ordered)?;

    if options.calls.parentheses == CallParentheses::Always {
        syntax
            .bare_calls
            .sort_unstable_by_key(|&(start, _)| std::cmp::Reverse(start));

        for (start, end) in syntax.bare_calls.iter().copied() {
            let close = tokens.partition_point(|token| token.start < end);

            tokens.insert(
                close,
                Token {
                    text: ")".to_owned(),
                    kind: Kind::Symbol,
                    syntax_kind: TokenKind::Byte(b')'),
                    start: end,
                    end,
                    newlines: 0,
                },
            );

            let open = tokens.partition_point(|token| token.start < start);
            let newlines = mem::take(&mut tokens[open].newlines);

            tokens.insert(
                open,
                Token {
                    text: "(".to_owned(),
                    kind: Kind::Symbol,
                    syntax_kind: TokenKind::Byte(b'('),
                    start,
                    end: start,
                    newlines,
                },
            );
        }
    }

    normalize_calls(&mut tokens, options.calls.parentheses, &syntax.calls);
    let formatted = format_tokens(tokens, options, &mut syntax);

    if let Some(error) = vermis::parse(formatted.as_bytes()).diagnostics.first() {
        return Err(invalid(format!(
            "formatter produced invalid Luau at byte {}: {}",
            error.span.start, error.message
        )));
    }

    Ok(formatted)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_require_comments_and_separators_across_reformatting() {
        let options = FormatOptions {
            line_ending: LineEnding::CrLf,
            ..FormatOptions::default()
        };

        let input = "--!strict\nlocal z = require(\n\"./z\"\n) -- z\n-- a\nlocal a = require(\"@a\")\nlocal f = print\nf(\"one\");(f)(\"two\")\n";
        let output = source(input, &options).unwrap();
        assert!(output.starts_with("--!strict\r\n-- a\r\nlocal a = require(\"@a\")\r\n"));
        assert!(output.contains("local z = require(\"./z\") -- z\r\n"));
        assert!(output.contains("f(\"one\");\r\n(f)(\"two\")\r\n"));
        assert_eq!(source(&output, &options).unwrap(), output);
    }

    #[test]
    fn omits_literal_call_parentheses_without_touching_declarations() {
        let mut options = FormatOptions::default();
        options.calls.parentheses = CallParentheses::OmitLiteral;
        let input = "function show(value) return value end\nlocal a = show(\"hello\")\nlocal b = show({ value = 1 })\n";
        let output = source(input, &options).unwrap();

        assert!(output.contains("function show(value)"), "{output}");
        assert!(output.contains("show \"hello\""), "{output}");
        assert!(output.contains("show { value = 1 }"), "{output}");
        assert_eq!(source(&output, &options).unwrap(), output);
    }

    #[test]
    fn never_wrap_collapses_type_operators_and_named_calls() {
        let mut options = FormatOptions::default();
        options.types.operator_wrap = Wrap::Never;
        options.chains.wrap = Wrap::Never;
        let input = "type Either = Left\n | Right\nlocal result = object:first()\n:second()\n";
        let output = source(input, &options).unwrap();
        assert!(output.lines().any(|line| line.contains("Left | Right")));

        assert!(
            output
                .lines()
                .any(|line| line.contains("first") && line.contains("second"))
        );

        assert_eq!(source(&output, &options).unwrap(), output);
    }

    #[test]
    fn spaces_every_compound_assignment_from_the_parser() {
        let options = FormatOptions::default();

        let input = concat!(
            "local users=1\n",
            "users+=1\nusers-=2\nusers*=3\nusers/=4\n",
            "users//=5\nusers%=6\nusers^=7\nusers+=(2)\n",
            "local label=\"a\"\nlabel..=\"b\"\n",
        );

        let expected = concat!(
            "local users = 1\n",
            "users += 1\nusers -= 2\nusers *= 3\nusers /= 4\n",
            "users //= 5\nusers %= 6\nusers ^= 7\nusers += (2)\n",
            "local label = \"a\"\nlabel ..= \"b\"\n",
        );

        assert_eq!(source(input, &options).unwrap(), expected);
        assert_eq!(source(expected, &options).unwrap(), expected);
    }

    #[test]
    fn keeps_unary_minus_tight_without_collapsing_binary_subtraction() {
        let options = FormatOptions::default();
        let input = "local a=-1\nlocal b=1- -2\nlocal c=#items\n";
        let expected = "local a = -1\nlocal b = 1 - -2\nlocal c = #items\n";

        assert_eq!(source(input, &options).unwrap(), expected);
        assert_eq!(source(expected, &options).unwrap(), expected);
    }

    #[test]
    fn recognizes_if_expressions_after_compound_assignment() {
        let mut options = FormatOptions {
            indent_style: IndentStyle::Spaces,
            ..FormatOptions::default()
        };

        options.if_expressions.wrap = Wrap::Always;
        options.if_expressions.placement = crate::config::IfExpressionPlacement::NextLine;
        let input = "local users = 1\nusers += if ready then 1 else 2\n";
        let output = source(input, &options).unwrap();

        assert!(output.contains("users +=\n    if ready then"), "{output}");
        assert_eq!(source(&output, &options).unwrap(), output);
    }

    #[test]
    fn keeps_short_conditional_cast_in_function_on_one_line() {
        let options = FormatOptions {
            indent_style: IndentStyle::Spaces,
            ..FormatOptions::default()
        };

        let input = concat!(
            "local handlers = {\n",
            "    choose = function(item): Result?\n",
            "        return if item:matches(\"Result\") then item :: Result else nil\n",
            "    end,\n",
            "}\n",
        );

        let output = source(input, &options).unwrap();
        assert_eq!(output, input);
        assert_eq!(source(&output, &options).unwrap(), output);
    }

    #[test]
    fn spaces_cast_before_parenthesized_function_type() {
        let options = FormatOptions::default();
        let input = "(callback :: (string) -> ())(value)\n";
        let output = source(input, &options).unwrap();

        assert_eq!(output, input);
        assert_eq!(source(&output, &options).unwrap(), output);
    }

    #[test]
    fn preserves_statement_gaps_independently_of_block_and_table_edges() {
        let input = "local a = 1\n\n\nlocal b = 2\nfunction f()\n\nlocal c = 3\n\nlocal d = 4\n\nend\nlocal t = {\na = 1,\n\n\nb = 2,\n}\n";
        let options = FormatOptions::default();
        let output = source(input, &options).unwrap();

        assert!(output.contains("local a = 1\n\nlocal b = 2\n"), "{output}");
        assert!(output.contains("local c = 3\n\n\tlocal d = 4"));
        assert!(!output.contains("function f()\n\n"));
        assert!(!output.contains("local d = 4\n\nend"));
        assert!(output.contains("a = 1,\n\n\tb = 2,"));
        assert_eq!(source(&output, &options).unwrap(), output);

        let mut preserve_edges = options.clone();
        preserve_edges.blocks.edge_blank_lines = crate::config::EdgeBlankLines::Preserve;
        let preserved = source(input, &preserve_edges).unwrap();
        assert!(preserved.contains("function f()\n\n\tlocal c = 3"));
        assert!(preserved.contains("local d = 4\n\nend"));

        let mut remove_table_gaps = options;
        remove_table_gaps.tables.blank_lines = crate::config::TableBlankLines::Remove;
        let table = source(input, &remove_table_gaps).unwrap();
        assert!(table.contains("a = 1,\n\tb = 2,"));
    }

    #[test]
    fn formats_expression_type_arguments_without_spacing_comparisons() {
        let options = FormatOptions::default();
        let input = "local x = factory<<Item?>>(nil)\nlocal less = a < b\n";
        let output = source(input, &options).unwrap();

        assert!(output.contains("factory<<Item?>>(nil)"));
        assert!(output.contains("a < b"));
        assert_eq!(source(&output, &options).unwrap(), output);
    }

    #[test]
    fn separates_generic_closer_from_type_alias_assignment() {
        let options = FormatOptions::default();

        let input =
            "export type Box<T> = { value: T }\ndeclare function take<T>(item: Box<T>): T\n";

        let output = source(input, &options).unwrap();

        assert_eq!(
            output,
            "export type Box<T> = { value: T }\ndeclare function take<T>(item: Box<T>): T\n"
        );

        assert_eq!(source(&output, &options).unwrap(), output);
    }

    #[test]
    fn formats_type_functions_and_readonly_indexers() {
        let options = FormatOptions::default();
        let input = "export type AnyFunction =(...any) ->(...any)\nexport type AnyTable = { [any]: any }\n\ntype ReadonlyArray<T> = { read[number]: T }\ntype ReadonlyTable = { read[any]: unknown }\n";
        let expected = "export type AnyFunction = (...any) -> (...any)\nexport type AnyTable = { [any]: any }\n\ntype ReadonlyArray<T> = { read [number]: T }\ntype ReadonlyTable = { read [any]: unknown }\n";

        assert_eq!(source(input, &options).unwrap(), expected);
        assert_eq!(source(expected, &options).unwrap(), expected);
    }

    #[test]
    fn keeps_value_table_separators_distinct_from_type_table_separators() {
        let mut options = FormatOptions::default();
        options.types.table_separator = crate::config::TypeTableSeparator::Semicolon;

        let input =
            "type Named = { a: number, b: number }\nlocal value: Named = { a = 1, b = 2 }\n";

        let expected =
            "type Named = { a: number; b: number }\nlocal value: Named = { a = 1, b = 2 }\n";

        assert_eq!(source(input, &options).unwrap(), expected);
        assert_eq!(source(expected, &options).unwrap(), expected);
    }

    #[test]
    fn respects_inner_delimiter_spacing_without_separating_calls_or_indexes() {
        let mut options = FormatOptions::default();
        options.spacing.parentheses = true;
        options.spacing.brackets = true;
        let input = "local x = f(a)[i]\nlocal y = read[1]\n";
        let expected = "local x = f( a )[ i ]\nlocal y = read[ 1 ]\n";

        assert_eq!(source(input, &options).unwrap(), expected);
        assert_eq!(source(expected, &options).unwrap(), expected);
    }

    #[test]
    fn preserves_multiline_type_intersection_layout() {
        let options = FormatOptions {
            indent_style: IndentStyle::Spaces,
            ..FormatOptions::default()
        };

        let input = concat!(
            "type Registry = {\n",
            "    create: ((tag: \"A\") -> A)\n",
            "        & ((tag: \"B\") -> B)\n",
            "        & ((tag: \"C\") -> C)\n",
            "        & ((tag: \"D\") -> D)\n",
            "        & ((tag: \"E\") -> E)\n",
            "        & ((tag: \"F\") -> F)\n",
            "        & ((tag: \"G\") -> G),\n",
            "}\n",
        );

        let output = source(input, &options).unwrap();
        assert_eq!(output, input);
        assert_eq!(source(&output, &options).unwrap(), output);
    }

    #[test]
    fn indents_wrapped_declaration_parameters_without_a_runtime_body() {
        let mut options = FormatOptions {
            indent_style: IndentStyle::Spaces,
            ..FormatOptions::default()
        };

        options.parameters.wrap = Wrap::Always;

        let input = concat!(
            "declare function wrap<A..., B..., C..., D...>(\n",
            "    target: (A...) -> B...,\n",
            "    replacement: (C...) -> D...\n",
            "): (A...) -> B...\n",
        );

        assert_eq!(source(input, &options).unwrap(), input);
    }

    #[test]
    fn preserves_declaration_parameters_when_return_type_exceeds_width() {
        let options = FormatOptions {
            indent_style: IndentStyle::Spaces,
            ..FormatOptions::default()
        };

        let multiline = concat!(
            "declare function replace_handler<A1..., R1..., A2..., R2...>(\n",
            "    original: (A1...) -> R1...,\n",
            "    replacement: (A2...) -> R2...\n",
            "): (A1...) -> R1...\n",
            "declare function patch_method(\n",
            "    receiver: AnyTable | Instance | userdata,\n",
            "    method: string,\n",
            "    replacement: AnyFunction\n",
            "): AnyFunction\n",
        );

        let flat = concat!(
            "declare function replace_handler<A1..., R1..., A2..., R2...>(original: (A1...) -> R1..., replacement: (A2...) -> R2...): (A1...) -> R1...\n",
            "declare function patch_method(receiver: AnyTable | Instance | userdata, method: string, replacement: AnyFunction): AnyFunction\n",
        );

        assert_eq!(source(multiline, &options).unwrap(), multiline);
        assert_eq!(source(flat, &options).unwrap(), multiline);

        assert_eq!(
            source(&source(flat, &options).unwrap(), &options).unwrap(),
            multiline
        );
    }

    #[test]
    fn wraps_if_expressions_only_past_the_formatted_line_width() {
        let input = "local choice = if data[1] == data[2] and data[3] then x else y\n";

        let mut options = FormatOptions {
            indent_style: IndentStyle::Spaces,
            width: input.trim_end().len(),
            ..FormatOptions::default()
        };

        let inline = source(input, &options).unwrap();
        assert_eq!(inline, input);
        assert_eq!(source(&inline, &options).unwrap(), inline);

        options.width -= 1;
        let wrapped = source(input, &options).unwrap();

        assert_eq!(
            wrapped,
            "local choice = if data[1] == data[2] and data[3] then\n    x\nelse\n    y\n"
        );

        assert_eq!(source(&wrapped, &options).unwrap(), wrapped);
    }

    #[test]
    fn preserves_multiline_binary_continuations_inside_auto_wrapped_calls() {
        let options = FormatOptions {
            indent_style: IndentStyle::Spaces,
            ..FormatOptions::default()
        };

        let input = concat!(
            "local value = compute(function()\n",
            "    local direction = axes.RightVector * horizontal\n",
            "        + Vector3.yAxis * vertical\n",
            "        + axes.LookVector * depth\n",
            "    local content_size = node.size\n",
            "        - Vector2.new(padding.left + padding.right, padding.top + padding.bottom) * node.scale\n",
            "    return direction\n",
            "end)\n",
        );

        let output = source(input, &options).unwrap();
        assert_eq!(output, input);
        assert_eq!(source(&output, &options).unwrap(), output);
    }

    #[test]
    fn indents_standalone_subtraction_continuation() {
        let options = FormatOptions {
            indent_style: IndentStyle::Spaces,
            ..FormatOptions::default()
        };

        let input = concat!(
            "function measure(node, padding)\n",
            "    const content_size = node.size\n",
            "        - Vector2.new(padding.left + padding.right, padding.top + padding.bottom) * node.scale\n",
            "    return content_size\n",
            "end\n",
        );

        let output = source(input, &options).unwrap();
        assert_eq!(output, input);
        assert_eq!(source(&output, &options).unwrap(), output);
    }

    #[test]
    fn indents_leading_logical_operators_as_continuations() {
        let options = FormatOptions {
            indent_style: IndentStyle::Spaces,
            ..FormatOptions::default()
        };

        let input = concat!(
            "function is_ready(flag)\n",
            "    return available\n",
            "        and flag ~= false\n",
            "        and (typeof(flag) ~= \"function\" or (flag :: Reader<boolean>)())\n",
            "end\n",
        );

        assert_eq!(source(input, &options).unwrap(), input);

        assert_eq!(
            source(&source(input, &options).unwrap(), &options).unwrap(),
            input
        );
    }

    #[test]
    fn keeps_nested_if_expression_inline_inside_multiline_branch() {
        let options = FormatOptions {
            indent_style: IndentStyle::Spaces,
            ..FormatOptions::default()
        };

        let input = concat!(
            "local function collect(dictionary, mapper)\n",
            "    local result = {}\n",
            "    for key, value in dictionary do\n",
            "        local choice = if mapper == nil then\n",
            "            if value == nil then nil else { key = key, value = value }\n",
            "        else\n",
            "            mapper(key, value)\n",
            "        if choice ~= nil then\n",
            "            table.insert(result, choice)\n",
            "        end\n",
            "    end\n",
            "    return result\n",
            "end\n",
        );

        let output = source(input, &options).unwrap();
        assert_eq!(output, input);
        assert_eq!(source(&output, &options).unwrap(), output);
    }

    #[test]
    fn preserves_multiline_if_expression_branch_alignment() {
        let options = FormatOptions {
            indent_style: IndentStyle::Spaces,
            ..FormatOptions::default()
        };

        let input = concat!(
            "local bounds = if corner then\n",
            "    Vector.offset(WIDTH, WIDTH)\n",
            "elseif direction ~= 0 then\n",
            "    Vector.new(0, WIDTH, 1, 0)\n",
            "else\n",
            "    Vector.new(1, 0, 0, WIDTH)\n",
        );

        let output = source(input, &options).unwrap();
        assert_eq!(output, input);
        assert_eq!(source(&output, &options).unwrap(), output);
    }

    #[test]
    fn keeps_statement_else_after_nested_if_expression() {
        let options = FormatOptions {
            indent_style: IndentStyle::Spaces,
            ..FormatOptions::default()
        };

        let input = concat!(
            "if options.pick == nil then\n",
            "    getter, setter = wire.signal(if options.default ~= nil then options.default else false)\n",
            "else\n",
            "    getter, setter = options.pick, options.set\n",
            "end\n",
        );

        let output = source(input, &options).unwrap();
        assert_eq!(output, input);
        assert_eq!(source(&output, &options).unwrap(), output);
    }

    #[test]
    fn indents_wrapped_chains_and_if_expressions() {
        let mut options = FormatOptions {
            indent_style: IndentStyle::Spaces,
            ..FormatOptions::default()
        };

        options.chains.wrap = Wrap::Always;
        options.if_expressions.wrap = Wrap::Always;
        options.if_expressions.placement = crate::config::IfExpressionPlacement::NextLine;
        let input = "local x = receiver:first():second():third()\nlocal flag = if condition then yes else no\n";
        let output = source(input, &options).unwrap();

        assert!(
            output.contains("receiver:first()\n    :second()\n    :third()"),
            "{output}"
        );

        assert!(
            output.contains("local flag =\n    if condition then"),
            "{output}"
        );

        assert!(
            output.contains("\n        yes\n    else\n        no\n"),
            "{output}"
        );

        assert_eq!(source(&output, &options).unwrap(), output);

        options.chains.wrap = Wrap::Auto;
        let chain = "local x = receiver:first()\n:second()\n:third()\n";
        let formatted = source(chain, &options).unwrap();

        assert_eq!(
            formatted,
            "local x = receiver:first()\n    :second()\n    :third()\n"
        );

        assert_eq!(source(&formatted, &options).unwrap(), formatted);
    }

    #[test]
    fn keeps_conditional_table_call_indented_through_nested_functions() {
        let mut options = FormatOptions {
            indent_style: IndentStyle::Spaces,
            ..FormatOptions::default()
        };

        options.calls.layout = crate::config::CallLayout::HugLast;

        let input = concat!(
            "local nodes = {\n",
            "    if state.active and on_action ~= nil then\n",
            "        UI.Button({\n",
            "            Opacity = function()\n",
            "                return if hovered() then 0 else 1\n",
            "            end,\n",
            "            Text = \"×\",\n",
            "            MouseEnter = function()\n",
            "                hovered(true)\n",
            "            end,\n",
            "        })\n",
            "    else\n",
            "        nil,\n",
            "}\n",
        );

        let output = source(input, &options).unwrap();
        assert_eq!(output, input);
        assert_eq!(source(&output, &options).unwrap(), output);
    }

    #[test]
    fn keeps_statement_else_and_elseif_branches_in_nested_conditional_call() {
        let mut options = FormatOptions {
            indent_style: IndentStyle::Spaces,
            ..FormatOptions::default()
        };

        options.calls.layout = crate::config::CallLayout::HugLast;

        let input = concat!(
            "local cards = {\n",
            "    if primary then\n",
            "        UI.Card({\n",
            "            OnClick = function()\n",
            "                if ready then\n",
            "                    fire()\n",
            "                else\n",
            "                    defer()\n",
            "                end\n",
            "            end,\n",
            "        })\n",
            "    elseif secondary then\n",
            "        UI.Card({\n",
            "            Label = \"retry\",\n",
            "        })\n",
            "    else\n",
            "        nil,\n",
            "}\n",
        );

        let output = source(input, &options).unwrap();
        assert_eq!(output, input);
        assert_eq!(source(&output, &options).unwrap(), output);
    }

    #[test]
    fn keeps_nested_table_indented_inside_multiline_return_list() {
        let mut options = FormatOptions {
            indent_style: IndentStyle::Spaces,
            ..FormatOptions::default()
        };

        options.calls.layout = crate::config::CallLayout::HugLast;

        let input = concat!(
            "function render(item)\n",
            "    return\n",
            "        UI.Panel({\n",
            "            Enabled = true,\n",
            "            Opacity = function()\n",
            "                return math.clamp(item.opacity, 0, 1)\n",
            "            end,\n",
            "            item.child,\n",
            "        }),\n",
            "        EXIT_TIME\n",
            "end\n",
        );

        let output = source(input, &options).unwrap();
        assert_eq!(output, input);
        assert_eq!(source(&output, &options).unwrap(), output);
    }

    #[test]
    fn keeps_multiline_function_argument_inline_and_return_values_separate() {
        let options = FormatOptions {
            indent_style: IndentStyle::Spaces,
            ..FormatOptions::default()
        };

        let input = concat!(
            "local rows = map_all(source, function(item, active)\n",
            "    return\n",
            "        make_row(item.label, active),\n",
            "        TIMEOUT\n",
            "end)\n",
        );

        let output = source(input, &options).unwrap();
        assert_eq!(output, input);
        assert_eq!(source(&output, &options).unwrap(), output);

        let nested_table = concat!(
            "local rows = map_all(source, function(item, active)\n",
            "    return\n",
            "        Row({\n",
            "            title = item.title,\n",
            "        }),\n",
            "        TIMEOUT\n",
            "end)\n",
        );

        let output = source(nested_table, &options).unwrap();
        assert!(output.starts_with("local rows = map_all(source, function(item, active)\n"));
        assert!(output.contains("\n    return\n        Row("));
        assert!(output.contains("\n        TIMEOUT\nend)\n"));
        assert_eq!(source(&output, &options).unwrap(), output);
    }

    #[test]
    fn hugs_last_function_argument_past_commas_in_its_body() {
        let mut options = FormatOptions {
            indent_style: IndentStyle::Spaces,
            ..FormatOptions::default()
        };

        options.calls.layout = crate::config::CallLayout::HugLast;
        options.width = "local rows = map_all(source, function".len();

        let input = concat!(
            "local rows = map_all(source, function()\n",
            "    return 1, 2\n",
            "end)\n",
        );

        let output = source(input, &options).unwrap();
        assert_eq!(output, input);
        assert_eq!(source(&output, &options).unwrap(), output);
    }

    #[test]
    fn hugs_multiline_final_table_argument() {
        let mut options = FormatOptions {
            indent_style: IndentStyle::Spaces,
            ..FormatOptions::default()
        };

        options.calls.layout = crate::config::CallLayout::HugLast;

        let expected = concat!(
            "local result = stream.watch(context, {\n",
            "    signal = item:on(\"Change\"),\n",
            "    read = function()\n",
            "        return item.value\n",
            "    end,\n",
            "})\n",
        );

        let vertical = concat!(
            "local result = stream.watch(\n",
            "    context,\n",
            "    {\n",
            "        signal = item:on(\"Change\"),\n",
            "        read = function()\n",
            "            return item.value\n",
            "        end,\n",
            "    }\n",
            ")\n",
        );

        assert_eq!(source(expected, &options).unwrap(), expected);
        assert_eq!(source(vertical, &options).unwrap(), expected);
    }

    #[test]
    fn wraps_nested_calls_only_past_the_formatted_line_width() {
        let call = "    return Keyframe.make(parts[1], Tone.make(parts[2], parts[3], parts[4]))";
        let input = format!("function build(parts)\n{call}\nend\n");

        let mut options = FormatOptions {
            indent_style: IndentStyle::Spaces,
            width: call.len(),
            ..FormatOptions::default()
        };

        let inline = source(&input, &options).unwrap();
        assert_eq!(inline, input);
        assert_eq!(source(&inline, &options).unwrap(), inline);

        options.width -= 1;
        let wrapped = source(&input, &options).unwrap();

        assert_eq!(
            wrapped,
            "function build(parts)\n    return Keyframe.make(\n        parts[1],\n        Tone.make(parts[2], parts[3], parts[4])\n    )\nend\n"
        );

        assert_eq!(source(&wrapped, &options).unwrap(), wrapped);
    }

    #[test]
    fn preserves_keyed_table_rows_without_wrapping_short_calls() {
        let options = FormatOptions {
            indent_style: IndentStyle::Spaces,
            ..FormatOptions::default()
        };

        let input = concat!(
            "local checks = {\n",
            "    plain = typeof(api.plain) == \"function\",\n",
            "    [\"api.alpha\"] = typeof(api.alpha) == \"function\",\n",
            "    [\"api.beta\"] = typeof(api.beta) == \"function\",\n",
            "}\n",
        );

        let output = source(input, &options).unwrap();
        assert_eq!(output, input);
        assert_eq!(source(&output, &options).unwrap(), output);
    }

    #[test]
    fn keeps_numeric_for_header_on_one_line_inside_a_table_function() {
        let options = FormatOptions {
            indent_style: IndentStyle::Spaces,
            ..FormatOptions::default()
        };

        let input = concat!(
            "local handlers = {\n",
            "    invoke = function(items)\n",
            "        for i = #items, 1, -1 do\n",
            "            if items[i]() then\n",
            "                break\n",
            "            end\n",
            "        end\n",
            "    end,\n",
            "}\n",
        );

        let output = source(input, &options).unwrap();
        assert_eq!(output, input);
        assert_eq!(source(&output, &options).unwrap(), output);
    }

    #[test]
    fn keeps_parenthesized_casts_and_unary_conditions_spaced() {
        let options = FormatOptions::default();

        let input = concat!(
            "local value = (factory :: Getter<number>)()\n",
            "local count = if #items > 0 then #items else 0\n",
            "local end_index = if offset == nil then #items else offset - 1\n",
            "if #items >= LIMIT then\n",
            "\ttable.remove(items, 1)\n",
            "end\n",
            "return (value :: Getter<number>)()\n",
        );

        let output = source(input, &options).unwrap();
        assert_eq!(output, input);
        assert_eq!(source(&output, &options).unwrap(), output);
    }

    #[test]
    fn keeps_generic_type_arguments_inline_inside_wrapped_type_table() {
        let options = FormatOptions {
            indent_style: IndentStyle::Spaces,
            ..FormatOptions::default()
        };

        let input = concat!(
            "export type Registry = {\n",
            "    read first: (self: Registry) -> Outcome<string, number>,\n",
            "    read second: <T>(self: Registry, key: string, fallback: T) -> Outcome<T, string>,\n",
            "    read third: (self: Registry) -> Outcome<string, number>,\n",
            "}\n",
        );

        let output = source(input, &options).unwrap();
        assert_eq!(output, input);
        assert_eq!(source(&output, &options).unwrap(), output);
    }

    #[test]
    fn keeps_multiline_table_operands_with_their_type_operators() {
        let options = FormatOptions {
            indent_style: IndentStyle::Spaces,
            ..FormatOptions::default()
        };

        let intersection = concat!(
            "export type Combined = Base & {\n",
            "    read kind: \"READY\",\n",
            "    read changed: ((boolean) -> ())?,\n",
            "    read reset: (() -> ())?,\n",
            "}\n",
        );

        let union = concat!(
            "type Variant = Base | {\n",
            "    read status: boolean,\n",
            "} | string\n",
        );

        for input in [intersection, union] {
            let output = source(input, &options).unwrap();
            assert_eq!(output, input);
            assert_eq!(source(&output, &options).unwrap(), output);
        }
    }

    #[test]
    fn wraps_flat_type_unions_only_past_the_formatted_line_width() {
        let input = "type Variant = Outcome<number> | Missing | false\n";

        let mut options = FormatOptions {
            indent_style: IndentStyle::Spaces,
            width: input.trim_end().len(),
            ..FormatOptions::default()
        };

        let inline = source(input, &options).unwrap();
        assert_eq!(inline, input);
        assert_eq!(source(&inline, &options).unwrap(), inline);

        options.width -= 1;
        let wrapped = source(input, &options).unwrap();

        assert_eq!(
            wrapped,
            "type Variant = Outcome<number>\n    | Missing\n    | false\n"
        );

        assert_eq!(source(&wrapped, &options).unwrap(), wrapped);
    }

    #[test]
    fn expands_entire_over_width_type_union() {
        let options = FormatOptions {
            indent_style: IndentStyle::Spaces,
            ..FormatOptions::default()
        };

        let choices = (0..40)
            .map(|index| format!("\"choice_{index:02}\""))
            .collect::<Vec<_>>();

        let input = format!("export type Choices = {}\n", choices.join(" | "));
        let expected = format!("export type Choices = {}\n", choices.join("\n    | "));
        let output = source(&input, &options).unwrap();

        assert_eq!(output, expected);
        assert_eq!(source(&output, &options).unwrap(), output);

        assert_eq!(
            source("type Short = \"a\" | \"b\"\n", &options).unwrap(),
            "type Short = \"a\" | \"b\"\n"
        );
    }

    #[test]
    fn auto_wraps_type_and_value_tables_with_trailing_separators() {
        let mut options = FormatOptions {
            indent_style: IndentStyle::Spaces,
            ..FormatOptions::default()
        };

        options.tables.wrap = Wrap::Auto;
        options.types.table_wrap = Wrap::Auto;

        let value = "local settings = { active = true, }\n";
        let expanded_value = "local settings = {\n    active = true,\n}\n";
        assert_eq!(source(value, &options).unwrap(), expanded_value);
        assert_eq!(source(expanded_value, &options).unwrap(), expanded_value);

        let ty = "type Shape = { active: boolean, }\n";
        let expanded_type = "type Shape = {\n    active: boolean,\n}\n";
        assert_eq!(source(ty, &options).unwrap(), expanded_type);
        assert_eq!(source(expanded_type, &options).unwrap(), expanded_type);

        options.tables.trailing_comma = TrailingComma::Never;
        let without_comma = "local settings = {\n    active = true\n}\n";
        assert_eq!(source(value, &options).unwrap(), without_comma);
        assert_eq!(source(without_comma, &options).unwrap(), without_comma);

        options.types.table_separator = crate::config::TypeTableSeparator::Semicolon;
        let with_semicolon = "type Shape = {\n    active: boolean;\n}\n";
        assert_eq!(source(ty, &options).unwrap(), with_semicolon);
        assert_eq!(source(with_semicolon, &options).unwrap(), with_semicolon);
    }

    #[test]
    fn configures_interpolation_padding_independently_of_table_spacing() {
        let mut options = FormatOptions::default();

        let input = concat!(
            "local text = `sum={left+right}, grouped={(left + right)}, count={count}`\n",
            "local card = `value={ { enabled=true } }`\n",
        );

        let compact = concat!(
            "local text = `sum={left + right}, grouped={(left + right)}, count={count}`\n",
            "local card = `value={ { enabled = true }}`\n",
        );

        let output = source(input, &options).unwrap();
        assert_eq!(output, compact);
        assert_eq!(source(&output, &options).unwrap(), output);

        options.spacing.braces = false;

        assert_eq!(
            source("local text = `value={count}`\n", &options).unwrap(),
            "local text = `value={count}`\n"
        );

        options.spacing.braces = true;
        options.spacing.interpolation = true;

        let padded = concat!(
            "local text = `sum={ left + right }, grouped={ (left + right) }, count={ count }`\n",
            "local card = `value={ { enabled = true } }`\n",
        );

        let output = source(input, &options).unwrap();
        assert_eq!(output, padded);
        assert_eq!(source(&output, &options).unwrap(), output);

        options.spacing.braces = false;

        assert_eq!(
            source("local card = `value={ { enabled=true } }`\n", &options).unwrap(),
            "local card = `value={ {enabled = true} }`\n"
        );
    }

    #[test]
    fn keeps_interpolated_chains_intact_past_line_width() {
        let options = FormatOptions {
            width: 80,
            ..FormatOptions::default()
        };

        let input = concat!(
            "local text = `alpha={first:read():format()}, beta={second:read()}, ",
            "gamma={third:read()}, delta={fourth:read()}, epsilon={fifth:read()}`\n",
        );

        let output = source(input, &options).unwrap();
        assert_eq!(output, input);
        assert_eq!(source(&output, &options).unwrap(), output);
    }

    #[test]
    fn keeps_typeof_parentheses_tight_inside_generic_types() {
        let mut options = FormatOptions::default();
        let input = "type Value = wrapper.infer<typeof (ROOT)>\ntype Direct = typeof (ROOT)\n";
        let expected = "type Value = wrapper.infer<typeof(ROOT)>\ntype Direct = typeof(ROOT)\n";

        let output = source(input, &options).unwrap();
        assert_eq!(output, expected);
        assert_eq!(source(&output, &options).unwrap(), output);

        options.spacing.before_function_parentheses = BeforeFunctionParentheses::Always;
        assert_eq!(source(input, &options).unwrap(), expected);
    }

    #[test]
    fn formats_declarations_without_inventing_function_bodies() {
        let options = FormatOptions::default();

        let input =
            "declare function first( x:number ):number\n\ndeclare function second():number\n";

        let output = source(input, &options).unwrap();

        assert_eq!(
            output,
            "declare function first(x: number): number\n\ndeclare function second(): number\n"
        );

        assert_eq!(source(&output, &options).unwrap(), output);

        let class = "declare extern type Widget with\nvalue:string\nfunction get():string\nend\n\ndeclare function third():Widget\n";
        let formatted = source(class, &options).unwrap();

        assert_eq!(
            formatted,
            "declare extern type Widget with\n\tvalue: string\n\tfunction get(): string\nend\n\ndeclare function third(): Widget\n"
        );

        assert_eq!(source(&formatted, &options).unwrap(), formatted);
    }
}
