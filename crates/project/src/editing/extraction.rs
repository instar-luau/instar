use std::{io, sync::Arc};

use instar_shared::{
    Completion, EditIdentity, EditIntent, EditProposal, Location, Options, SourceSnapshot, Span,
    TextEdit,
};

use instar_syntax::{
    parse,
    token::TokenKind,
    tree::{NodeKind, Tree},
};

use super::request::{EditGuarantee, ExtractionRequest, ValidatedEdit};

use crate::{
    capture::Capture, execution::Runtime, graph::syntax_settings, queries::service::checked_source,
    report::EditReport, request::CheckRequest,
};

impl Runtime {
    pub(crate) fn extract(
        &mut self,
        request: ExtractionRequest,
        report: &mut EditReport,
        options: &Options,
    ) -> io::Result<Option<ValidatedEdit>> {
        let source = checked_source(&request.snapshot, request.source)?.clone();
        request.range.validate(&source)?;
        let mut capture = Capture::new(Arc::clone(&self.host), request.snapshot);

        let target = self
            .editor_targets(
                Some(source.version.identity),
                false,
                &mut capture,
                None,
                options,
            )?
            .pop()
            .ok_or_else(|| invalid("extraction source is absent"))?;

        let parsed = parse(&source, &syntax_settings(&target.effective), options);
        report.snapshot = capture.snapshot.clone();
        report.inputs = capture.guards();
        report.diagnostics.extend(parsed.diagnostics.clone());
        report.completion = report.completion.clone().combine(parsed.completion.clone());

        if parsed.completion != Completion::Complete {
            return Err(parsed.completion.error());
        }

        let tree = parsed.tree.syntax();

        if !tree.diagnostics.is_empty() {
            return Ok(None);
        }

        let mut pending = vec![tree.root];

        while let Some(node) = pending.pop() {
            options.check()?;

            if let NodeKind::Local {
                bindings, values, ..
            } = &tree.node(node).kind
                && tree.list(bindings).len() == 1
                && tree.list(values).len() == 1
            {
                let value = tree.node(tree.list(values)[0].node);

                if value.span.start <= request.range.start
                    && request.range.end <= value.span.end
                    && literal(tree, &value.kind)
                {
                    let start = tree.node(node).span.start;

                    let line = source.text[..start]
                        .rfind('\n')
                        .map_or(0, |offset| offset + 1);

                    let indent = &source.text[line..start];

                    if !indent.bytes().all(|byte| matches!(byte, b' ' | b'\t'))
                        || tree
                            .tokens
                            .iter()
                            .rev()
                            .find(|token| {
                                token.span.end <= start && token.kind != TokenKind::Whitespace
                            })
                            .is_some_and(|token| {
                                matches!(token.kind, TokenKind::Comment | TokenKind::BlockComment)
                            })
                    {
                        return Ok(None);
                    }

                    let name = available(tree, options)?;

                    let edits = extraction_edits(
                        &source,
                        Span {
                            start: value.span.start,
                            end: value.span.end,
                        },
                        start,
                        indent,
                        name,
                    )?;

                    let proposal = EditProposal {
                        identity: EditIdentity::new(self.identity()),
                        edits,
                        guards: vec![source.version],
                        intent: EditIntent::Extraction,
                    };

                    report.proposal = Some(proposal.clone());

                    let mut validated = self.validate_edit(
                        proposal,
                        &capture.snapshot,
                        &capture.guards(),
                        vec![
                            EditGuarantee::Ranges,
                            EditGuarantee::Syntax,
                            EditGuarantee::Bindings,
                            EditGuarantee::Diagnostics,
                            EditGuarantee::Resolution,
                        ],
                        &CheckRequest::default(),
                        options,
                    )?;

                    validated.guarantees.push(EditGuarantee::Effects);

                    return Ok(Some(validated));
                }
            }

            pending.extend(tree.children(node));
        }

        Ok(None)
    }
}

fn extraction_edits(
    source: &SourceSnapshot,
    value: Span,
    start: usize,
    indent: &str,
    name: String,
) -> io::Result<Vec<TextEdit>> {
    let ending = if source
        .text
        .find('\n')
        .is_some_and(|offset| offset > 0 && source.text.as_bytes()[offset - 1] == b'\r')
    {
        "\r\n"
    } else {
        "\n"
    };

    Ok(vec![
        TextEdit {
            location: Location::new(source, Span { start, end: start })?,
            replacement: format!(
                "local {name} = {}{ending}{indent}",
                &source.text[value.start..value.end]
            ),
        },
        TextEdit {
            location: Location::new(source, value)?,
            replacement: name,
        },
    ])
}

fn available(tree: &Tree<'_>, options: &Options) -> io::Result<String> {
    let mut number = 0_u64;

    loop {
        options.check()?;

        let name = if number == 0 {
            "extracted".to_owned()
        } else {
            format!("extracted{number}")
        };

        if !tree.tokens.iter().any(|token| {
            token.kind == TokenKind::Name && token.bytes(tree.source) == name.as_bytes()
        }) {
            return Ok(name);
        }

        number += 1;
    }
}

fn literal(tree: &Tree<'_>, kind: &NodeKind) -> bool {
    match kind {
        NodeKind::Number { .. } | NodeKind::Boolean { .. } | NodeKind::Nil { .. } => true,

        NodeKind::String { token } => matches!(
            tree.token(*token).kind,
            TokenKind::QuotedString | TokenKind::RawString
        ),

        _ => false,
    }
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}
