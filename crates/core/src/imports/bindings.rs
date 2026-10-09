use std::{collections::BTreeSet, io, path::PathBuf, time::Instant};

use instar_analysis::{Edit, Location, Options, error::invalid};

use instar_bridge::frontend::{
    Notation, Parentheses, Suggestion, SuggestionContext, SuggestionKind,
};

use instar_syntax::{bindings::Bindings, identifier, literal};
use vermis::{token::TokenKind, tree::Tree};

use super::paths::{relative, unavailable};

use crate::{
    analysis::{Entry, Origin},
    project::Project,
    resolve::{Request, module_path},
    suggestions::{Candidate, Suggestions},
};

impl Project {
    pub(crate) fn automatic_imports(
        &mut self,
        entry: &Entry,
        offset: usize,
        roots: &[PathBuf],
        suggestions: &mut Suggestions,
        options: &Options,
        started: Instant,
    ) -> io::Result<()> {
        if roots.is_empty()
            || !suggestions.loader
            || !matches!(
                suggestions.context,
                SuggestionContext::Expression | SuggestionContext::Statement
            )
        {
            return Ok(());
        }

        let source = self.source(&entry.source)?;

        let prefix = source
            .text
            .get(suggestions.location.range[0]..offset)
            .ok_or_else(|| invalid("completion prefix is outside the source"))?
            .to_ascii_lowercase();

        if prefix.is_empty() {
            return Ok(());
        }

        let tree = vermis::parse(source.text.as_bytes());

        let Some(occupied) = occupied(&tree, suggestions, options, started)? else {
            return Ok(());
        };

        let insertion = prologue(&source.text, &tree, options, started)?;

        if insertion >= suggestions.location.range[0] {
            return Ok(());
        }

        let module = self.module(&entry.source, entry.context.as_ref())?;

        let parent = module
            .path
            .parent()
            .ok_or_else(|| invalid("module has no parent"))?;

        let ending = if source
            .text
            .find('\n')
            .is_some_and(|index| index > 0 && source.text.as_bytes()[index - 1] == b'\r')
        {
            "\r\n"
        } else {
            "\n"
        };

        for path in self.index_sources(roots, &options.remaining(started))? {
            options.check(started)?;

            if path == module.source {
                continue;
            }

            let logical = module_path(&path);

            let Some(name) = logical.file_name().and_then(|name| name.to_str()) else {
                continue;
            };

            if !name.to_ascii_lowercase().starts_with(&prefix)
                || occupied.contains(name)
                || !identifier(name)
            {
                continue;
            }

            let Some(request) = relative(parent, &logical) else {
                continue;
            };

            let target = match self
                .resolve(&module, &Request::String(request.clone()))
                .result
            {
                Ok(target) if target.source == path && target.identity != module.identity => target,
                Ok(_) => continue,
                Err(error) if unavailable(&error.error()) => continue,
                Err(error) => return Err(error.error()),
            };

            let reachable = match self.discover(&target.source, Some(&target.identity)) {
                Ok(reachable) => reachable,
                Err(error) if unavailable(&error) => continue,
                Err(error) => return Err(error),
            };

            if reachable.contains(&module.identity) {
                continue;
            }

            let location = Location {
                range: [insertion, insertion],
                ..suggestions.location.clone()
            };

            suggestions
                .entries
                .push(candidate(name, &request, location, ending));
        }

        options.check(started)
    }
}

fn candidate(name: &str, request: &str, location: Location<Origin>, ending: &str) -> Candidate {
    Candidate {
        entry: Suggestion {
            name: name.to_owned(),
            kind: SuggestionKind::Module,
            detail: format!("Auto import from {request}"),
            documentation: None,
            insert_text: None,
            parentheses: Parentheses::None,
            deprecated: false,
            callable: false,
            preferred: false,
            notation: Notation::Identifier,
        },
        edits: vec![Edit {
            location,
            text: format!(
                "local {name} = require({}){ending}",
                literal::quote(request)
            ),
        }],
    }
}

fn occupied(
    tree: &Tree<'_>,
    suggestions: &Suggestions,
    options: &Options,
    started: Instant,
) -> io::Result<Option<BTreeSet<String>>> {
    let bindings = Bindings::analyze(tree, Some((options, started)));

    if let Some(reason) = bindings.interruption {
        return Err(reason.error());
    }

    if bindings.global_assignments.contains("require")
        || bindings
            .declarations
            .values()
            .any(|binding| tree.text(binding.name) == b"require")
    {
        return Ok(None);
    }

    let mut occupied = BTreeSet::new();

    for candidate in &suggestions.entries {
        options.check(started)?;
        occupied.insert(candidate.entry.name.clone());
    }

    for binding in bindings.declarations.values() {
        options.check(started)?;
        occupied.insert(String::from_utf8_lossy(tree.text(binding.name)).into_owned());
    }

    occupied.extend(bindings.global_assignments);

    Ok(Some(occupied))
}

fn prologue(
    source: &str,
    tree: &Tree<'_>,
    options: &Options,
    started: Instant,
) -> io::Result<usize> {
    let mut offset = if source.starts_with('\u{feff}') {
        '\u{feff}'.len_utf8()
    } else {
        0
    };

    if source[offset..].starts_with("#!") {
        offset = source[offset..]
            .find('\n')
            .map_or(source.len(), |end| offset + end + 1);
    }

    for token in &tree.tokens {
        options.check(started)?;

        if token.span.end <= offset {
            continue;
        }

        if token.kind == TokenKind::Whitespace
            || (token.kind == TokenKind::Comment && token.bytes(tree.source).starts_with(b"--!"))
        {
            offset = token.span.end;
        } else {
            break;
        }
    }

    Ok(offset)
}
