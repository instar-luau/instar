use std::{
    io,
    path::{Path, PathBuf},
    time::Instant,
};

use instar_analysis::{Completion, Edit, Location, Options};

use vermis::{
    token::TokenKind,
    tree::{NodeKind, Tree},
};

use crate::{analysis::Entry, project::Project};

impl Project {
    pub(crate) fn extraction(
        &mut self,
        entry: &Entry,
        range: [usize; 2],
        options: &Options,
        started: Instant,
    ) -> io::Result<Vec<Edit<PathBuf>>> {
        let source = self.source(&entry.source)?;
        let tree = vermis::parse(source.text.as_bytes());

        if !tree.diagnostics.is_empty() {
            return Ok(Vec::new());
        }

        let mut pending = vec![tree.root];

        while let Some(node) = pending.pop() {
            options.check(started)?;

            if let NodeKind::Local {
                bindings, values, ..
            } = &tree.node(node).kind
                && tree.list(bindings).len() == 1
                && tree.list(values).len() == 1
            {
                let value = tree.node(tree.list(values)[0].node);

                if value.span.start <= range[0]
                    && range[1] <= value.span.end
                    && literal(&tree, &value.kind)
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
                        return Ok(Vec::new());
                    }

                    let name = available(&tree, options, started)?;

                    let ending = if source.text.find('\n').is_some_and(|offset| {
                        offset > 0 && source.text.as_bytes()[offset - 1] == b'\r'
                    }) {
                        "\r\n"
                    } else {
                        "\n"
                    };

                    let mut location = Location {
                        module: entry.source.clone(),
                        revision: source.revision,
                        range: [start, start],
                    };

                    let insertion = Edit {
                        location: location.clone(),
                        text: format!(
                            "local {name} = {}{ending}{indent}",
                            &source.text[value.span.start..value.span.end]
                        ),
                    };

                    location.range = [value.span.start, value.span.end];

                    let edits = vec![
                        insertion,
                        Edit {
                            location,
                            text: name,
                        },
                    ];

                    if !checked(self, &entry.source, options, started)? {
                        return Ok(Vec::new());
                    }

                    let mut validation = self.propose(&edits, options, started)?;

                    let verified = validation.during(
                        |validation| checked(validation, &entry.source, options, started),
                        &options.remaining(started),
                    );

                    self.view.consult(&validation.watch_inputs());

                    return Ok(if verified? { edits } else { Vec::new() });
                }
            }

            pending.extend(tree.children(node));
        }

        Ok(Vec::new())
    }
}

fn checked(
    project: &mut Project,
    path: &Path,
    options: &Options,
    started: Instant,
) -> io::Result<bool> {
    let entries = project.entries(&[path.to_path_buf()], &options.remaining(started))?;

    if entries.len() != 1 {
        return Ok(false);
    }

    let result = project.check(&entries, &options.remaining(started))?;

    if let Completion::Incomplete(reason) = result.completion {
        return Err(reason.error());
    }

    Ok(
        result.diagnostics.is_empty()
            && result.modules.iter().any(|module| module.source() == path),
    )
}

fn available(tree: &Tree<'_>, options: &Options, started: Instant) -> io::Result<String> {
    let mut number = 0_u64;

    loop {
        options.check(started)?;

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
