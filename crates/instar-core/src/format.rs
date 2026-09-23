//! Luau source formatting.

use std::{io, mem};

use vermis::{Parts, View};

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
    start: usize,
    end: usize,
    newlines: usize,
}

#[derive(Default)]
struct FormatSyntax {
    starts: Vec<usize>,
    ends: Vec<usize>,
    edges: Vec<usize>,
    declarations: Vec<(usize, usize)>,
    class_headers: Vec<(usize, usize)>,
    bare_calls: Vec<(usize, usize)>,
    type_spans: Vec<(usize, usize)>,
    optional_ends: Vec<usize>,
    outer_type_spans: Vec<(usize, usize)>,
    access_modifiers: Vec<usize>,
    parameters: Vec<usize>,
}

fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

#[expect(
    clippy::too_many_lines,
    reason = "one lexer pass must track trivia and token boundaries together"
)]
fn lex(input: &str) -> io::Result<Vec<Token>> {
    let bytes = input.as_bytes();
    let mut tokens = Vec::new();
    let mut at = 0;
    let mut newlines = 0;

    while at < bytes.len() {
        match bytes[at] {
            b' ' | b'\t' | b'\r' => at += 1,

            b'\n' => {
                newlines += 1;
                at += 1;
            }

            _ => {
                let start = at;
                let kind;

                if bytes[at] == b'-' && bytes.get(at + 1) == Some(&b'-') {
                    kind = Kind::Comment;
                    at += 2;

                    if bytes.get(at) == Some(&b'[') {
                        let mut cursor = at + 1;

                        while bytes.get(cursor) == Some(&b'=') {
                            cursor += 1;
                        }

                        if bytes.get(cursor) == Some(&b'[') {
                            let level = cursor - at - 1;
                            at = cursor + 1;

                            while at < bytes.len() {
                                if bytes[at] == b']' {
                                    let mut end = at + 1;
                                    let mut count = 0;

                                    while bytes.get(end) == Some(&b'=') {
                                        count += 1;
                                        end += 1;
                                    }

                                    if count == level && bytes.get(end) == Some(&b']') {
                                        at = end + 1;
                                        break;
                                    }
                                }

                                at += 1;
                            }
                        } else {
                            while at < bytes.len() && bytes[at] != b'\n' {
                                at += 1;
                            }
                        }
                    } else {
                        while at < bytes.len() && bytes[at] != b'\n' {
                            at += 1;
                        }
                    }
                } else if bytes[at] == b'\'' || bytes[at] == b'"' {
                    kind = Kind::String;
                    let quote = bytes[at];
                    at += 1;

                    while at < bytes.len() {
                        if bytes[at] == b'\\' {
                            at = (at + 2).min(bytes.len());
                        } else if bytes[at] == quote {
                            at += 1;
                            break;
                        } else {
                            at += 1;
                        }
                    }
                } else if bytes[at] == b'[' {
                    let mut cursor = at + 1;

                    while bytes.get(cursor) == Some(&b'=') {
                        cursor += 1;
                    }

                    if bytes.get(cursor) == Some(&b'[') {
                        kind = Kind::String;
                        let level = cursor - at - 1;
                        at = cursor + 1;

                        while at < bytes.len() {
                            if bytes[at] == b']' {
                                let mut end = at + 1;
                                let mut count = 0;

                                while bytes.get(end) == Some(&b'=') {
                                    count += 1;
                                    end += 1;
                                }

                                if count == level && bytes.get(end) == Some(&b']') {
                                    at = end + 1;
                                    break;
                                }
                            }

                            at += 1;
                        }
                    } else {
                        kind = Kind::Symbol;
                        at += 1;
                    }
                } else if bytes[at].is_ascii_alphabetic() || bytes[at] == b'_' {
                    kind = Kind::Word;
                    at += 1;

                    while at < bytes.len()
                        && (bytes[at].is_ascii_alphanumeric() || bytes[at] == b'_')
                    {
                        at += 1;
                    }
                } else if bytes[at].is_ascii_digit()
                    || (bytes[at] == b'.' && bytes.get(at + 1).is_some_and(u8::is_ascii_digit))
                {
                    kind = Kind::Number;

                    let hexadecimal =
                        bytes[start..].starts_with(b"0x") || bytes[start..].starts_with(b"0X");

                    if bytes[at] == b'.' {
                        at += 1;
                    } else if hexadecimal {
                        at += 2;
                    }

                    while at < bytes.len()
                        && if hexadecimal {
                            bytes[at].is_ascii_hexdigit() || bytes[at] == b'_'
                        } else {
                            bytes[at].is_ascii_digit() || bytes[at] == b'_'
                        }
                    {
                        at += 1;
                    }

                    if !hexadecimal
                        && bytes.get(at) == Some(&b'.')
                        && bytes.get(at + 1) != Some(&b'.')
                    {
                        at += 1;

                        while at < bytes.len() && (bytes[at].is_ascii_digit() || bytes[at] == b'_')
                        {
                            at += 1;
                        }
                    }

                    let exponent = if hexadecimal { b'p' } else { b'e' };

                    if bytes
                        .get(at)
                        .is_some_and(|byte| byte.to_ascii_lowercase() == exponent)
                    {
                        at += 1;

                        if bytes
                            .get(at)
                            .is_some_and(|byte| matches!(byte, b'+' | b'-'))
                        {
                            at += 1;
                        }

                        while at < bytes.len() && (bytes[at].is_ascii_digit() || bytes[at] == b'_')
                        {
                            at += 1;
                        }
                    }
                } else {
                    kind = Kind::Symbol;
                    at += 1;

                    for op in [
                        b"...".as_slice(),
                        b"::",
                        b"->",
                        b"..",
                        b"==",
                        b"~=",
                        b"<=",
                        b">=",
                        b"+=",
                        b"-=",
                        b"*=",
                        b"/=",
                        b"//",
                        b"&&",
                        b"||",
                    ] {
                        if bytes[start..].starts_with(op) {
                            at = start + op.len();
                            break;
                        }
                    }
                }

                let text = std::str::from_utf8(&bytes[start..at])
                    .map_err(|error| invalid(error.to_string()))?
                    .to_owned();

                tokens.push(Token {
                    text,
                    kind,
                    start,
                    end: at,
                    newlines: mem::take(&mut newlines),
                });
            }
        }
    }

    Ok(tokens)
}

fn body_starts(
    node: View<'_, '_>,
    parent: Option<vermis::Kind>,
    options: &FormatOptions,
    syntax: &mut FormatSyntax,
) {
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

    if let Some(Parts::Function {
        parameters: args, ..
    }) = node.parts()
    {
        syntax.parameters.push(args.span().start);
    }

    if options.calls.parentheses == CallParentheses::Always {
        let arguments = match node.parts() {
            Some(Parts::Call { arguments, .. } | Parts::MethodCall { arguments, .. }) => {
                Some(arguments)
            }

            _ => None,
        };

        if let Some(arguments) = arguments
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

fn type_table(tokens: &[Token], start: usize) -> bool {
    tokens[..start]
        .iter()
        .rev()
        .take_while(|token| token.newlines == 0)
        .any(|token| {
            (token.kind == Kind::Word && token.text == "type")
                || matches!(token.text.as_str(), ":" | "->")
        })
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

fn if_expression_start(tokens: &[Token], index: usize) -> Option<usize> {
    for start in (0..=index).rev() {
        if start < index
            && tokens[start].kind == Kind::Word
            && matches!(
                tokens[start].text.as_str(),
                "local" | "return" | "type" | "function"
            )
        {
            return None;
        }

        if tokens[start].kind == Kind::Word && tokens[start].text == "end" {
            return None;
        }

        if tokens[start].kind == Kind::Word && tokens[start].text == "if" {
            let previous = start.checked_sub(1).map(|position| &tokens[position]);

            let expression = previous.is_some_and(|token| {
                matches!(
                    token.text.as_str(),
                    "=" | "return" | "(" | "," | "then" | "else"
                )
            });

            return expression.then_some(start);
        }
    }

    None
}

fn if_expression_wrap(
    tokens: &[Token],
    start: usize,
    line_len: usize,
    options: &FormatOptions,
) -> bool {
    let end = (start..tokens.len())
        .find(|&position| tokens[position].kind == Kind::Word && tokens[position].text == "else")
        .unwrap_or_else(|| tokens.len().saturating_sub(1));

    let multiline = tokens[start..=end].iter().any(|token| token.newlines > 0);

    let width = tokens[start..=end]
        .iter()
        .map(|token| token.text.len() + 1)
        .sum::<usize>();

    wrap_break(
        options.if_expressions.wrap,
        multiline,
        line_len.saturating_add(width) > options.width,
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
) {
    let compact_conditionals = matches!(
        options.blocks.simple_bodies,
        crate::config::SimpleBodies::CompactConditionals | crate::config::SimpleBodies::CompactAll
    );

    for conditional in 0..tokens.len() {
        if tokens[conditional].kind != Kind::Word
            || tokens[conditional].text != "if"
            || if_expression_start(tokens, conditional).is_some()
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

fn is_spaced_operator(token: &str) -> bool {
    matches!(
        token,
        "=" | "+"
            | "-"
            | "*"
            | "/"
            | "//"
            | "%"
            | "^"
            | "=="
            | "~="
            | "<"
            | ">"
            | "<="
            | ">="
            | ".."
            | "and"
            | "or"
            | "->"
            | "|"
            | "&"
            | "?"
    )
}

fn needs_space(
    prev: &Token,
    current: &Token,
    options: &FormatOptions,
    definition: bool,
    method_name: bool,
    tight_type_spacing: bool,
) -> bool {
    let a = prev.text.as_str();
    let b = current.text.as_str();

    if prev.kind == Kind::Comment || current.kind == Kind::Comment {
        return true;
    }

    if method_name || tight_type_spacing {
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

    if matches!(a, "." | "::") || matches!(b, "." | "::" | "," | ";") {
        return false;
    }

    if b == ":" {
        return false;
    }

    if b == "(" {
        if matches!(a, "=" | ":" | "->" | "|" | "&") {
            return true;
        }

        return match options.spacing.before_function_parentheses {
            BeforeFunctionParentheses::Never => false,
            BeforeFunctionParentheses::Calls => !definition,
            BeforeFunctionParentheses::Definitions => definition,
            BeforeFunctionParentheses::Always => true,
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

    if matches!(prev.kind, Kind::Word | Kind::Number | Kind::String)
        && matches!(current.kind, Kind::Word | Kind::Number | Kind::String)
    {
        return true;
    }

    if b == "{" && (prev.kind == Kind::Word || matches!(a, ")" | "]" | "}")) {
        return true;
    }

    if is_spaced_operator(a) || is_spaced_operator(b) || matches!(a, "not" | ":") {
        return true;
    }

    prev.kind == Kind::Number && current.kind == Kind::Word
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

fn hug_last_start(tokens: &[Token], start: usize) -> Option<usize> {
    let end = delimiter_end(tokens, start)?;
    let mut nesting = 0usize;
    let mut last = start + 1;

    for (index, token) in tokens.iter().enumerate().take(end).skip(start + 1) {
        match token.text.as_str() {
            "(" | "{" | "[" => nesting += 1,
            ")" | "}" | "]" => nesting = nesting.saturating_sub(1),
            "," if nesting == 0 => last = index + 1,
            _ => {}
        }
    }

    let arg = tokens.get(last)?;

    (arg.text == "{"
        || arg.text == "function"
        || (arg.kind == Kind::String && arg.text.contains('\n')))
    .then_some(last)
}

fn normalize_calls(tokens: &mut Vec<Token>, style: CallParentheses) {
    if !matches!(
        style,
        CallParentheses::OmitString | CallParentheses::OmitTable | CallParentheses::OmitLiteral
    ) {
        return;
    }

    let mut index = 1;

    while index + 2 < tokens.len() {
        if tokens[index].text != "("
            || matches!(
                tokens[index - 1].text.as_str(),
                "if" | "while" | "for" | "function" | "typeof"
            )
        {
            index += 1;
            continue;
        }

        let is_call = tokens[index - 1].kind == Kind::Word
            || matches!(tokens[index - 1].text.as_str(), ")" | "]" | "}");

        let end = delimiter_end(tokens, index);

        let Some(end) = end else {
            index += 1;
            continue;
        };

        let literal = end == index + 2 && tokens[index + 1].kind == Kind::String;

        let table = end > index + 1
            && tokens[index + 1].text == "{"
            && delimiter_end(tokens, index + 1) == Some(end - 1);

        let omit = is_call
            && ((literal
                && matches!(
                    style,
                    CallParentheses::OmitString | CallParentheses::OmitLiteral
                ))
                || (table
                    && matches!(
                        style,
                        CallParentheses::OmitTable | CallParentheses::OmitLiteral
                    )));

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
            && enclosing_brace(&tokens, index).is_some_and(|start| type_table(&tokens, start))
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

    prepare_conditional_bodies(&mut tokens, options, &mut syntax.starts, &mut compact_ends);

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

    for i in 0..tokens.len() {
        let text = tokens[i].text.as_str();
        let in_declaration = declared.get(i) == Some(&true);
        let current_if_expression = if_expression_start(&tokens, i).is_some();

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

        let opener = match text {
            "(" => Some('('),
            "{" => Some('{'),
            "[" => Some('['),
            _ => None,
        };

        let mut open_wrapped = false;

        if let Some(open) = opener
            && let Some(end) = delimiter_end(&tokens, i)
        {
            let multiline = tokens[i + 1..=end].iter().any(|token| token.newlines > 0);

            let content_width = tokens[i..=end]
                .iter()
                .map(|token| token.text.len() + 1)
                .sum::<usize>();

            let nonempty = end > i + 1;

            let function_parameters =
                open == '(' && syntax.parameters.binary_search(&tokens[i].start).is_ok();

            let call = i > 0
                && ((tokens[i - 1].kind == Kind::Word
                    && !matches!(
                        tokens[i - 1].text.as_str(),
                        "if" | "while" | "for" | "function" | "typeof"
                    ))
                    || matches!(tokens[i - 1].text.as_str(), ")" | "]" | "}"));

            let mode = if open == '{' {
                if type_table(&tokens, i) {
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
                hug_last_start(&tokens, i)
            } else {
                None
            };

            open_wrapped = wrap_break(
                mode,
                multiline,
                line_len.saturating_add(content_width) > options.width,
                nonempty,
            );

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
        let in_wrapped_list = stack.last().is_some_and(|(_, wrapped)| *wrapped);

        let type_operator = matches!(text, "|" | "&")
            && tokens[..i]
                .iter()
                .rev()
                .take(24)
                .any(|token| matches!(token.text.as_str(), "type" | ":" | "->"));

        let chain = chain_step(&tokens, i);

        let preserve_break = modes.last().map_or_else(
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

        let mut continuation = false;

        let mut line_break = close_block
            || (matches!(text, "else" | "elseif") && !current_if_expression)
            || (previous.is_some() && syntax.starts.binary_search(&tokens[i].start).is_ok())
            || (explicit_break
                && previous.is_some()
                && (preserve_break
                    || tokens[i].kind == Kind::Comment
                    || previous.is_some_and(|p| tokens[p].kind == Kind::Comment)));

        let expression_start = if_expression_start(&tokens, i);

        let expression_wrap = expression_start
            .is_some_and(|start| if_expression_wrap(&tokens, start, line_len, options));

        if expression_wrap
            && ((text == "if"
                && options.if_expressions.placement
                    == crate::config::IfExpressionPlacement::NextLine)
                || text == "else"
                || (options.if_expressions.layout == crate::config::IfExpressionLayout::Block
                    && previous
                        .is_some_and(|p| matches!(tokens[p].text.as_str(), "then" | "else")))
                || (options.if_expressions.layout == crate::config::IfExpressionLayout::Leading
                    && matches!(text, "then" | "else")))
        {
            line_break = true;
            continuation = true;
        }

        if type_operator
            && (wrap_break(
                options.types.operator_wrap,
                explicit_break,
                line_len > options.width,
                true,
            ) || (options.types.operator_wrap == Wrap::Auto && explicit_break))
        {
            line_break = true;
            continuation = true;
        }

        if chain {
            let rest = (i + 3..tokens.len()).take_while(|&index| tokens[index].newlines == 0);
            let more_calls = rest.clone().any(|index| chain_step(&tokens, index));

            let previous_steps = (0..i)
                .rev()
                .take_while(|&index| tokens[index].newlines == 0 || chain_step(&tokens, index))
                .filter(|&index| chain_step(&tokens, index))
                .count();

            if more_calls || previous_steps > 0 {
                let mode = wrap_break(
                    options.chains.wrap,
                    explicit_break,
                    line_len > options.width,
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

        let hug_inline = call_lists.last() == Some(&true)
            && hug_starts
                .last()
                .copied()
                .flatten()
                .is_some_and(|start| i < start);

        if i > 0
            && in_wrapped_list
            && (matches!(tokens[i - 1].text.as_str(), "," | "(" | "{") || close_delimiter)
            && (!hug_inline || close_delimiter)
        {
            line_break = true;
        }

        if chain && line_break {
            continuation = true;
        }

        if text == "}"
            && stack
                .last()
                .is_some_and(|(kind, wrapped)| *kind == '{' && *wrapped)
            && options.tables.trailing_comma == TrailingComma::Multiline
            && !enclosing_brace(&tokens, i).is_some_and(|start| type_table(&tokens, start))
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
            let visual_level =
                level.saturating_sub(parameter_depth) + usize::from(continuation && line_break);

            match options.indent_style {
                IndentStyle::Tabs => output.extend(std::iter::repeat_n('\t', visual_level)),

                IndentStyle::Spaces => output.extend(std::iter::repeat_n(
                    ' ',
                    visual_level.saturating_mul(options.indent_width),
                )),
            }

            line_len = visual_level.saturating_mul(options.indent_width);
        } else if let Some(p) = previous {
            let definition =
                text == "(" && syntax.parameters.binary_search(&tokens[i].start).is_ok();

            let method_name = tokens[p].text == ":"
                && tokens[i].kind == Kind::Word
                && tokens.get(i + 1).is_some_and(|next| next.text == "(");

            let tight_type_spacing = (tight_type[p] && tokens[p].text == "<")
                || (tight_type[i] && matches!(text, ">" | "?"))
                || (tight_type[i] && text == "<" && tokens[p].kind == Kind::Word);

            if (text == "["
                && syntax
                    .access_modifiers
                    .binary_search(&tokens[p].start)
                    .is_ok())
                || needs_space(
                    &tokens[p],
                    &tokens[i],
                    options,
                    definition,
                    method_name,
                    tight_type_spacing,
                )
            {
                output.push(' ');
                line_len += 1;
            }
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

        if text == ";" {
            if options.semicolons == Semicolons::Necessary
                && enclosing_brace(&tokens, i).is_none()
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

    let ordered = crate::require_order::sort(input, &options.requires)?;
    let parsed_ordered = vermis::parse(ordered.as_bytes());
    let mut syntax = FormatSyntax::default();

    if let Some(root) = parsed_ordered.view(parsed_ordered.root) {
        body_starts(root, None, options, &mut syntax);
    }

    syntax.starts.sort_unstable();
    syntax.edges.sort_unstable();
    syntax.declarations.sort_unstable();
    syntax.type_spans.sort_unstable();
    syntax.optional_ends.sort_unstable();
    syntax.outer_type_spans.sort_unstable();
    syntax.parameters.sort_unstable();
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
                    start,
                    end: start,
                    newlines,
                },
            );
        }
    }

    normalize_calls(&mut tokens, options.calls.parentheses);
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
            "type Drawings = {\n",
            "    new: ((kind: \"Circle\") -> DrawingCircle)\n",
            "        & ((kind: \"Image\") -> DrawingImage)\n",
            "        & ((kind: \"Line\") -> DrawingLine)\n",
            "        & ((kind: \"Quad\") -> DrawingQuad)\n",
            "        & ((kind: \"Square\") -> DrawingSquare)\n",
            "        & ((kind: \"Text\") -> DrawingText)\n",
            "        & ((kind: \"Triangle\") -> DrawingTriangle),\n",
            "}\n",
        );

        let output = source(input, &options).unwrap();
        assert_eq!(output, input);
        assert_eq!(source(&output, &options).unwrap(), output);
    }

    #[test]
    fn indents_wrapped_declaration_parameters_without_a_runtime_body() {
        let options = FormatOptions {
            indent_style: IndentStyle::Spaces,
            ..FormatOptions::default()
        };

        let input = concat!(
            "declare function hookfunction<A1..., R1..., A2..., R2...>(\n",
            "    functionToHook: (A1...) -> R1...,\n",
            "    hook: (A2...) -> R2...\n",
            "): (A1...) -> R1...\n",
        );

        assert_eq!(source(input, &options).unwrap(), input);
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

        assert!(output.contains("\n    yes\n    else\n    no\n"), "{output}");
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
