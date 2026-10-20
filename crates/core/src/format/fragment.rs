use vermis::{
    parser::Parser,
    token::TokenKind,
    tree::{ListEntry, NodeKind, Tree},
};

use crate::format::{Error, Prepared, directives, layout, preservation};

#[derive(Copy, Clone)]
pub(crate) enum Grammar {
    Annotation,
    Declaration,
    Argument,
    Field,
}

pub(crate) fn format(
    source: &str,
    grammar: Grammar,
    configuration: &Prepared,
) -> Result<String, Error> {
    let original = parse(source, grammar)?;
    let directives = directives::collect(&original)?;
    let plan = layout::Builder::build(&original, configuration, None, &directives)?;
    let bytes = layout::emit(&original, configuration.settings(), &plan)?;

    let output = String::from_utf8(bytes).map_err(|error| {
        Error::internal(0..0, format!("Formatter produced invalid UTF-8. {error}"))
    })?;

    let proposed = parse(&output, grammar).map_err(|error| {
        Error::internal(
            0..0,
            format!("Formatter produced invalid fragment syntax. {error}"),
        )
    })?;

    let proposed_directives = directives::collect(&proposed)?;

    preservation::validate(
        &original,
        &proposed,
        configuration.settings(),
        None,
        &directives,
        &proposed_directives,
    )?;

    Ok(output)
}

pub(crate) fn nominal(source: &str) -> Result<(), Error> {
    let tree = parse(source, Grammar::Annotation)?;

    if tree
        .tokens
        .iter()
        .any(|token| matches!(token.kind, TokenKind::Comment | TokenKind::BlockComment))
    {
        return Err(Error::input(
            0..source.len(),
            "Nominal type spelling contains a comment.",
        ));
    }

    let NodeKind::Root { block, .. } = tree.node(tree.root).kind else {
        return Err(Error::internal(0..0, "Fragment has no root block."));
    };

    let NodeKind::Block { ref statements } = tree.node(block).kind else {
        return Err(Error::internal(0..0, "Fragment has no block."));
    };

    let [entry] = tree.list(statements) else {
        return Err(Error::internal(0..0, "Fragment has no unique annotation."));
    };

    let mut expression = match tree.node(entry.node).kind {
        NodeKind::TypeName {
            arguments: None, ..
        } => return Ok(()),

        NodeKind::TypeOf { expression, .. } => expression,

        _ => {
            return Err(Error::input(
                0..source.len(),
                "External type spelling is not nominal.",
            ));
        }
    };

    loop {
        match tree.node(expression).kind {
            NodeKind::Name { .. } => return Ok(()),
            NodeKind::Field { receiver, .. } => expression = receiver,

            _ => {
                return Err(Error::input(
                    0..source.len(),
                    "External type spelling is not a named value.",
                ));
            }
        }
    }
}

fn parse(source: &str, grammar: Grammar) -> Result<Tree<'_>, Error> {
    let mut parser = Parser::new(source.as_bytes());
    let start = parser.position();

    let node = match grammar {
        Grammar::Annotation => parser.annotation(),
        Grammar::Declaration => parser.declaration_annotation(),
        Grammar::Argument => parser.type_argument(),
        Grammar::Field => parser.type_field(true),
    };

    if !parser.at(TokenKind::EndOfFile) {
        let token = parser.current();

        return Err(Error::input(
            token.span.start..token.span.end,
            "Unexpected fragment suffix.",
        ));
    }

    let statements = parser.append_list([ListEntry {
        node,
        separator: None,
    }]);

    let block = parser.append_node(start, NodeKind::Block { statements });
    let tree = parser.finish(block);

    if let Some(diagnostic) = tree.diagnostics.first() {
        return Err(Error::input(
            diagnostic.span.start..diagnostic.span.end,
            diagnostic.message,
        ));
    }

    if let Some(node) = tree
        .nodes
        .iter()
        .find(|node| matches!(node.kind, NodeKind::Error | NodeKind::Missing { .. }))
    {
        return Err(Error::input(
            node.span.start..node.span.end,
            "Incomplete fragment syntax.",
        ));
    }

    Ok(tree)
}
