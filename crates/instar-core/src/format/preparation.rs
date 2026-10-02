use std::{io, mem};

use vermis::{Parts, Span, TokenKind, View};

use crate::config::{CallParentheses, FormatOptions, LeadingZero, QuoteStyle, Semicolons};

use super::invalid;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Kind {
    Word,
    Number,
    String,
    Symbol,
    Comment,
}

pub(super) struct Token {
    pub(super) text: String,
    pub(super) kind: Kind,
    pub(super) syntax_kind: TokenKind,
    pub(super) start: usize,
    pub(super) end: usize,
    pub(super) newlines: usize,
}

#[derive(Default)]
pub(super) struct FormatSyntax {
    pub(super) starts: Vec<usize>,
    ends: Vec<usize>,
    pub(super) edges: Vec<usize>,
    declarations: Vec<(usize, usize)>,
    pub(super) class_headers: Vec<(usize, usize)>,
    bare_calls: Vec<(usize, usize)>,
    pub(super) calls: Vec<usize>,
    pub(super) list_starts: Vec<usize>,
    pub(super) function_arguments: Vec<(usize, Span)>,
    pub(super) last_arguments: Vec<(usize, usize)>,
    pub(super) return_values: Vec<usize>,
    return_spans: Vec<(Span, std::ops::Range<usize>)>,
    pub(super) conditionals: Vec<Span>,
    pub(super) conditional_branches: Vec<(usize, Span)>,
    pub(super) statement_ifs: Vec<Span>,
    pub(super) binary_operators: Vec<usize>,
    pub(super) interpolation_expressions: Vec<Span>,
    pub(super) unary_minus: Vec<usize>,
    type_tables: Vec<usize>,
    type_spans: Vec<(usize, usize)>,
    pub(super) typeof_gaps: Vec<(usize, usize)>,
    pub(super) type_operator_gaps: Vec<(usize, usize)>,
    pub(super) type_chains: Vec<(usize, usize)>,
    optional_ends: Vec<usize>,
    outer_type_spans: Vec<(usize, usize)>,
    pub(super) access_modifiers: Vec<usize>,
    pub(super) parameters: Vec<usize>,
    pub(super) signature_ends: Vec<(usize, usize)>,
}

pub(super) struct Prepared {
    pub(super) tokens: Vec<Token>,
    pub(super) syntax: FormatSyntax,
    pub(super) statement_end: Vec<bool>,
    pub(super) tight_type: Vec<bool>,
    pub(super) declared: Vec<bool>,
    pub(super) compact_ends: Vec<bool>,
    pub(super) return_levels: Vec<isize>,
}

fn tokens(tree: &vermis::Tree<'_>) -> io::Result<Vec<Token>> {
    let bytes = tree.source;
    let mut tokens = Vec::new();
    let mut newlines = 0;

    for token in &tree.tokens {
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

pub(super) fn type_table(syntax: &FormatSyntax, opener: &Token) -> bool {
    syntax.type_tables.binary_search(&opener.start).is_ok()
}

pub(super) fn enclosing_brace(tokens: &[Token], index: usize) -> Option<usize> {
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

pub(super) fn delimiter_end(tokens: &[Token], start: usize) -> Option<usize> {
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

fn collect_metadata(tree: &vermis::Tree<'_>, options: &FormatOptions) -> FormatSyntax {
    let mut syntax = FormatSyntax::default();

    if let Some(root) = tree.view(tree.root) {
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

    syntax
}

fn insert_call_parentheses(
    tokens: &mut Vec<Token>,
    syntax: &mut FormatSyntax,
    options: &FormatOptions,
) {
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
}

fn normalize_token_spellings(tokens: &mut [Token], options: &FormatOptions) {
    for token in tokens {
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
}

pub(super) fn prepare(tree: &vermis::Tree<'_>, options: &FormatOptions) -> io::Result<Prepared> {
    let mut syntax = collect_metadata(tree, options);
    let mut tokens = tokens(tree)?;
    insert_call_parentheses(&mut tokens, &mut syntax, options);
    normalize_calls(&mut tokens, options.calls.parentheses, &syntax.calls);

    for index in 0..tokens.len() {
        if matches!(tokens[index].text.as_str(), "," | ";")
            && enclosing_brace(&tokens, index)
                .is_some_and(|start| type_table(&syntax, &tokens[start]))
        {
            tokens[index].text = match (tokens[index].text.as_str(), options.types.tables.separator)
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

    normalize_token_spellings(&mut tokens, options);

    let return_levels = return_continuation_levels(&tokens, &syntax);
    syntax.return_values.sort_unstable();

    Ok(Prepared {
        tokens,
        syntax,
        statement_end,
        tight_type,
        declared,
        compact_ends,
        return_levels,
    })
}
