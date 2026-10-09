use std::{io, path::PathBuf, time::Instant};

use instar_analysis::{Completion, Edit, Location, Options, error::invalid};
use instar_format::Formatter;
use instar_syntax::bindings::Bindings;

use vermis::{
    token::TokenKind,
    tree::{NodeIndex, NodeKind, Tree},
};

use crate::{analysis::Entry, project::Project};

impl Project {
    pub(crate) fn organize(
        &mut self,
        entry: &Entry,
        options: &Options,
        started: Instant,
    ) -> io::Result<Vec<Edit<PathBuf>>> {
        let Some(configuration) =
            self.formatting_configuration(&entry.source, &options.remaining(started))?
        else {
            return Ok(Vec::new());
        };

        let source = self.source(&entry.source)?;
        let tree = vermis::parse(source.text.as_bytes());

        if !tree.diagnostics.is_empty() {
            return Ok(Vec::new());
        }

        let bindings = Bindings::analyze(&tree, Some((options, started)));

        if let Some(reason) = bindings.interruption {
            return Err(reason.error());
        }

        let NodeKind::Root { block, .. } = &tree.node(tree.root).kind else {
            return Ok(Vec::new());
        };

        let NodeKind::Block { statements } = &tree.node(*block).kind else {
            return Ok(Vec::new());
        };

        let mut range: Option<[usize; 2]> = None;

        for statement in tree.list(statements) {
            options.check(started)?;

            if !import(&tree, statement.node, &bindings) {
                break;
            }

            let span = tree.node(statement.node).span;

            if let Some(range) = &mut range {
                range[1] = span.end;
            } else {
                range = Some([span.start, span.end]);
            }
        }

        let Some(range) = range else {
            return Ok(Vec::new());
        };

        let original = &source.text[range[0]..range[1]];

        let result = Formatter::default().format(
            original.as_bytes(),
            &configuration,
            &options.remaining(started),
        )?;

        if let Completion::Incomplete(reason) = result.completion {
            return Err(reason.error());
        }

        let Some(output) = result.output else {
            return Ok(Vec::new());
        };

        let output = String::from_utf8(output).map_err(invalid)?;
        let output = output.trim_end_matches(['\r', '\n']);

        if output == original {
            return Ok(Vec::new());
        }

        let before = vermis::parse(original.as_bytes());
        let after = vermis::parse(output.as_bytes());

        if !after.diagnostics.is_empty()
            || !before
                .tokens
                .iter()
                .filter(|token| token.kind != TokenKind::Whitespace)
                .map(|token| (token.kind, token.bytes(before.source)))
                .eq(after
                    .tokens
                    .iter()
                    .filter(|token| token.kind != TokenKind::Whitespace)
                    .map(|token| (token.kind, token.bytes(after.source))))
        {
            return Ok(Vec::new());
        }

        options.check(started)?;

        Ok(vec![Edit {
            location: Location {
                module: entry.source.clone(),
                revision: source.revision,
                range,
            },
            text: output.to_owned(),
        }])
    }
}

fn import(tree: &Tree<'_>, node: NodeIndex, bindings: &Bindings) -> bool {
    let NodeKind::Local {
        bindings: names,
        values,
        ..
    } = &tree.node(node).kind
    else {
        return false;
    };

    let ([_], [value]) = (tree.list(names), tree.list(values)) else {
        return false;
    };

    let NodeKind::Call { callee, arguments } = &tree.node(value.node).kind else {
        return false;
    };

    let NodeKind::Arguments { values, .. } = &tree.node(*arguments).kind else {
        return false;
    };

    bindings.global(tree, *callee)
        && tree.text(*callee) == b"require"
        && tree.list(values).len() == 1
}
