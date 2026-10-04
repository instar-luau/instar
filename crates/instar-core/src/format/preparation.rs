use std::{io, mem};

use vermis::{
    token::{Keyword, Span, Symbol, TokenKind},
    tree::{NodeIndex, NodeKind, Tree},
};

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
    function_bodies: Vec<Span>,
    branch_bodies: Vec<Span>,
    pub(super) binary_operators: Vec<usize>,
    pub(super) interpolation_expressions: Vec<Span>,
    pub(super) unary_minus: Vec<usize>,
    type_tables: Vec<usize>,
    type_separators: Vec<usize>,
    type_spans: Vec<(usize, usize)>,
    pub(super) typeof_gaps: Vec<(usize, usize)>,
    pub(super) type_operator_gaps: Vec<(usize, usize)>,
    pub(super) type_chains: Vec<(usize, usize)>,
    optional_ends: Vec<usize>,
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

fn tokens(tree: &Tree<'_>) -> io::Result<Vec<Token>> {
    let bytes = tree.source;
    let mut tokens = Vec::new();
    let mut newlines = 0;

    for token in &tree.tokens {
        match token.kind {
            TokenKind::EndOfFile => break,

            TokenKind::Whitespace => {
                newlines += token.bytes(bytes).split(|&byte| byte == b'\n').count() - 1;
                continue;
            }

            TokenKind::MalformedString
            | TokenKind::MalformedComment
            | TokenKind::InvalidUnicode { .. }
            | TokenKind::InvalidInterpolationDoubleBrace
            | TokenKind::InvalidCharacter { .. } => {
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

            TokenKind::QuotedString
            | TokenKind::RawString
            | TokenKind::InterpolatedStringStart
            | TokenKind::InterpolatedStringMiddle
            | TokenKind::InterpolatedStringEnd
            | TokenKind::InterpolatedStringSimple => Kind::String,

            TokenKind::Comment | TokenKind::BlockComment => Kind::Comment,

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

fn collect_list_metadata(tree: &Tree<'_>, index: NodeIndex, syntax: &mut FormatSyntax) {
    let node = tree.node(index);

    match &node.kind {
        NodeKind::TypeTable { fields, .. } => {
            syntax.type_tables.push(node.span.start);

            syntax.type_separators.extend(
                tree.list(fields)
                    .iter()
                    .filter_map(|field| field.separator)
                    .map(|separator| tree.token(separator).span.start),
            );

            syntax.list_starts.extend(
                tree.list(fields)
                    .iter()
                    .map(|field| tree.node(field.node).span.start),
            );
        }

        NodeKind::Table { fields, .. }
        | NodeKind::Parameters {
            parameters: fields, ..
        } => syntax.list_starts.extend(
            tree.list(fields)
                .iter()
                .map(|field| tree.node(field.node).span.start),
        ),

        NodeKind::Arguments { values, .. } => {
            let list_start = node.span.start;
            let mut last = None;

            for value in tree.list(values) {
                let value = tree.node(value.node);
                let span = value.span;
                syntax.list_starts.push(span.start);
                last = Some(span.start);

                if matches!(value.kind, NodeKind::Function { .. }) {
                    syntax.function_arguments.push((list_start, span));
                }
            }

            if let Some(last) = last {
                syntax.last_arguments.push((list_start, last));
            }
        }

        NodeKind::Return { values, .. } => {
            let first = syntax.return_values.len();

            syntax.return_values.extend(
                tree.list(values)
                    .iter()
                    .map(|value| tree.node(value.node).span.start),
            );

            syntax
                .return_spans
                .push((node.span, first..syntax.return_values.len()));
        }

        _ => {}
    }
}

fn collect_conditional_branches(tree: &Tree<'_>, index: NodeIndex, syntax: &mut FormatSyntax) {
    if !tree.text(index).starts_with(b"if") {
        return;
    }

    let root = tree.node(index).span.start;
    let mut branch = index;

    while let NodeKind::Conditional { truthy, falsy, .. } = &tree.node(branch).kind {
        syntax
            .conditional_branches
            .push((root, tree.node(*truthy).span));

        if matches!(tree.node(*falsy).kind, NodeKind::Conditional { .. })
            && tree.text(*falsy).starts_with(b"elseif")
        {
            branch = *falsy;
        } else {
            syntax
                .conditional_branches
                .push((root, tree.node(*falsy).span));

            break;
        }
    }
}

fn collect_declaration_metadata(
    tree: &Tree<'_>,
    index: NodeIndex,
    parent: Option<&NodeKind>,
    syntax: &mut FormatSyntax,
) {
    let node = tree.node(index);

    if let NodeKind::Function {
        parameters,
        returns,
        ..
    } = &node.kind
    {
        let start = tree.node(*parameters).span.start;
        syntax.parameters.push(start);

        if let Some(annotation) = returns {
            syntax
                .signature_ends
                .push((start, tree.node(*annotation).span.end));
        }
    }

    if matches!(
        node.kind,
        NodeKind::Declaration { .. } | NodeKind::Function { body: None, .. }
    ) {
        syntax.declarations.push((node.span.start, node.span.end));
    }

    if matches!(parent, Some(NodeKind::Declaration { .. }))
        && let NodeKind::Class {
            name,
            extends,
            members,
            ..
        } = &node.kind
    {
        let start = tree.node(extends.unwrap_or(*name)).span.end;

        let end = tree
            .list(members)
            .first()
            .map_or(node.span.end, |member| tree.node(member.node).span.start);

        syntax.class_headers.push((start, end));
    }
}

fn collect_simple_body(
    tree: &Tree<'_>,
    index: NodeIndex,
    start: usize,
    end: usize,
    bodies: &mut Vec<Span>,
) {
    if let NodeKind::Block { statements } = &tree.node(index).kind
        && let [statement] = tree.list(statements)
        && statement.separator.is_none()
    {
        bodies.push(Span { start, end });
    }
}

fn collect_body_metadata(tree: &Tree<'_>, index: NodeIndex, syntax: &mut FormatSyntax) {
    let node = tree.node(index);

    match &node.kind {
        NodeKind::Conditional { .. } => {
            syntax.conditionals.push(node.span);
            collect_conditional_branches(tree, index, syntax);
        }

        NodeKind::Function {
            parameters,
            returns,
            body: Some(body),
            end: Some(end),
            ..
        } => collect_simple_body(
            tree,
            *body,
            tree.node(returns.unwrap_or(*parameters)).span.end,
            tree.token(*end).span.start,
            &mut syntax.function_bodies,
        ),

        NodeKind::If {
            branches,
            otherwise,
            end,
        } => {
            syntax.statement_ifs.push(node.span);

            if let Some(end) = end {
                let mut closing = tree.token(*end).span.start;

                if let Some(otherwise) = otherwise
                    && let NodeKind::Else { keyword, body } = &tree.node(*otherwise).kind
                {
                    collect_simple_body(
                        tree,
                        *body,
                        tree.token(*keyword).span.end,
                        closing,
                        &mut syntax.branch_bodies,
                    );

                    closing = tree.token(*keyword).span.start;
                }

                for branch in tree.list(branches).iter().rev() {
                    if let NodeKind::Branch {
                        keyword,
                        then,
                        body,
                        ..
                    } = &tree.node(branch.node).kind
                    {
                        if let Some(then) = then {
                            collect_simple_body(
                                tree,
                                *body,
                                tree.token(*then).span.end,
                                closing,
                                &mut syntax.branch_bodies,
                            );
                        }

                        closing = tree.token(*keyword).span.start;
                    }
                }
            }
        }

        _ => {}
    }
}

fn body_starts(
    tree: &Tree<'_>,
    index: NodeIndex,
    parent: Option<&NodeKind>,
    options: &FormatOptions,
    syntax: &mut FormatSyntax,
) {
    let node = tree.node(index);
    collect_list_metadata(tree, index, syntax);
    collect_declaration_metadata(tree, index, parent, syntax);
    collect_body_metadata(tree, index, syntax);

    match &node.kind {
        NodeKind::TypeArguments { .. }
        | NodeKind::Generics { .. }
        | NodeKind::InstantiationArguments { .. } => {
            syntax.type_spans.push((node.span.start, node.span.end));
        }

        NodeKind::TypeOf {
            keyword,
            expression,
            ..
        } => syntax.typeof_gaps.push((
            tree.token(*keyword).span.end,
            tree.node(*expression).span.start,
        )),

        NodeKind::TypeUnion { left, right, .. }
        | NodeKind::TypeIntersection { left, right, .. } => {
            if parent
                .is_none_or(|parent| mem::discriminant(parent) != mem::discriminant(&node.kind))
            {
                syntax.type_chains.push((node.span.start, node.span.end));
            }

            let start = left.map_or(node.span.start, |left| tree.node(left).span.end);

            syntax
                .type_operator_gaps
                .push((start, tree.node(*right).span.start));
        }

        NodeKind::Binary { operator, .. } => {
            syntax
                .binary_operators
                .push(tree.token(*operator).span.start);
        }

        NodeKind::Interpolation { segments } => syntax.interpolation_expressions.extend(
            tree.list(segments)
                .iter()
                .map(|segment| tree.node(segment.node))
                .filter(|segment| {
                    !matches!(segment.kind, NodeKind::String { token } if matches!(
                        tree.token(token).kind,
                        TokenKind::InterpolatedStringStart
                            | TokenKind::InterpolatedStringMiddle
                            | TokenKind::InterpolatedStringEnd
                            | TokenKind::InterpolatedStringSimple
                    ))
                })
                .map(|segment| segment.span),
        ),

        NodeKind::Unary { operator, .. }
            if tree.token(*operator).kind == TokenKind::Symbol(Symbol::Subtract) =>
        {
            syntax.unary_minus.push(tree.token(*operator).span.start);
        }

        NodeKind::TypeOptional { .. } => syntax.optional_ends.push(node.span.end),

        NodeKind::TypeField {
            access: Some(access),
            ..
        }
        | NodeKind::TypeIndexer {
            access: Some(access),
            ..
        } => {
            syntax.access_modifiers.push(tree.token(*access).span.start);
        }

        _ => {}
    }

    if let NodeKind::Block { statements } = &node.kind {
        for (position, statement) in tree.list(statements).iter().enumerate() {
            let span = tree.node(statement.node).span;

            if position == 0 && !matches!(parent, Some(NodeKind::Root { .. })) {
                syntax.edges.push(span.start);
            }

            syntax.ends.push(span.end);
            syntax.starts.push(span.start);
        }
    }

    if let NodeKind::Call { arguments, .. } | NodeKind::MethodCall { arguments, .. } = &node.kind {
        let span = tree.node(*arguments).span;
        syntax.calls.push(span.start);

        if options.calls.parentheses == CallParentheses::Always
            && matches!(
                tree.text(*arguments).first(),
                Some(b'\'' | b'"' | b'[' | b'{')
            )
        {
            syntax.bare_calls.push((span.start, span.end));
        }
    }

    for child in tree.children(index) {
        body_starts(tree, child, Some(&node.kind), options, syntax);
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

fn compact_bodies(
    tokens: &mut [Token],
    bodies: &[Span],
    starts: &mut Vec<usize>,
    compact_ends: &mut [bool],
) {
    for span in bodies {
        let body = tokens.partition_point(|token| token.start < span.start);
        let end = tokens.partition_point(|token| token.start < span.end);
        let body_tokens = &tokens[body..end];

        if body_tokens.is_empty()
            || body_tokens.iter().any(|token| {
                matches!(
                    token.syntax_kind,
                    TokenKind::Comment
                        | TokenKind::BlockComment
                        | TokenKind::Keyword(
                            Keyword::Function
                                | Keyword::If
                                | Keyword::For
                                | Keyword::While
                                | Keyword::Do
                                | Keyword::Repeat
                                | Keyword::End
                                | Keyword::Else
                                | Keyword::ElseIf
                        )
                        | TokenKind::Symbol(Symbol::Semicolon)
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

        tokens[body].newlines = 0;

        if let Ok(start) = starts.binary_search(&tokens[body].start) {
            starts.remove(start);
        }

        if tokens.get(end).is_some_and(|token| {
            token.start == span.end && token.syntax_kind == TokenKind::Keyword(Keyword::End)
        }) {
            tokens[end].newlines = 0;
            compact_ends[end] = true;
        }
    }
}

fn type_punctuation(
    tokens: &[Token],
    spans: &[(usize, usize)],
    optional_ends: &[usize],
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

fn collect_metadata(tree: &Tree<'_>, options: &FormatOptions) -> FormatSyntax {
    let mut syntax = FormatSyntax::default();

    body_starts(tree, tree.root, None, options, &mut syntax);

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
    syntax.type_separators.sort_unstable();
    syntax.optional_ends.sort_unstable();
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
                    syntax_kind: TokenKind::Symbol(Symbol::RightParenthesis),
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
                    syntax_kind: TokenKind::Symbol(Symbol::LeftParenthesis),
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

pub(super) fn prepare(tree: &Tree<'_>, options: &FormatOptions) -> io::Result<Prepared> {
    let mut syntax = collect_metadata(tree, options);
    let mut tokens = tokens(tree)?;
    insert_call_parentheses(&mut tokens, &mut syntax, options);
    normalize_calls(&mut tokens, options.calls.parentheses, &syntax.calls);

    for token in &mut tokens {
        if syntax.type_separators.binary_search(&token.start).is_ok() {
            let (text, symbol) = match options.types.tables.separator {
                crate::config::TypeTableSeparator::Comma => (",", Symbol::Comma),
                crate::config::TypeTableSeparator::Semicolon => (";", Symbol::Semicolon),
            };

            text.clone_into(&mut token.text);
            token.syntax_kind = TokenKind::Symbol(symbol);
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

    let tight_type = type_punctuation(&tokens, &syntax.type_spans, &syntax.optional_ends);

    let mut declared = Vec::new();

    if !syntax.declarations.is_empty() {
        declared.resize(tokens.len(), false);

        for &(start, end) in &syntax.declarations {
            let first = tokens.partition_point(|token| token.start < start);
            let last = tokens.partition_point(|token| token.start < end);
            declared[first..last].fill(true);
        }
    }

    let mut compact_ends = vec![false; tokens.len()];

    if matches!(
        options.blocks.simple_bodies,
        crate::config::SimpleBodies::CompactFunctions | crate::config::SimpleBodies::CompactAll
    ) {
        compact_bodies(
            &mut tokens,
            &syntax.function_bodies,
            &mut syntax.starts,
            &mut compact_ends,
        );
    }

    if matches!(
        options.blocks.simple_bodies,
        crate::config::SimpleBodies::CompactConditionals | crate::config::SimpleBodies::CompactAll
    ) {
        compact_bodies(
            &mut tokens,
            &syntax.branch_bodies,
            &mut syntax.starts,
            &mut compact_ends,
        );
    }

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
