use std::{collections::BTreeSet, io, path::PathBuf, slice, time::Instant};

use instar_analysis::{Completion, Edit, Location, Options, error::invalid};
use instar_bridge::frontend::{Namespace, Reference};
use instar_syntax::{identifier, literal::quote};
use vermis::token::TokenKind;

use crate::{
    analysis::{Entry, Origin, locate},
    project::Project,
    resolve::Identity,
};

impl Project {
    /// Prepares a writable, unambiguous source symbol for rename.
    ///
    /// # Errors
    /// Returns invalid positions, environment, source, or interruption failures.
    pub fn prepare_rename(
        &mut self,
        entry: &Entry,
        offset: usize,
        options: &Options,
    ) -> io::Result<Option<Reference<Origin>>> {
        let started = Instant::now();

        self.during(
            |project| {
                let references =
                    project.references(entry, offset, &[], &options.remaining(started))?;

                let origin = Origin::Module(project.module(&entry.source, entry.context.as_ref())?);

                let selected = references
                    .iter()
                    .find(|reference| {
                        reference.location.module == origin
                            && reference.location.range[0] <= offset
                            && offset < reference.location.range[1]
                    })
                    .or_else(|| {
                        references.iter().find(|reference| {
                            reference.location.module == origin
                                && reference.location.range[1] == offset
                        })
                    });

                let Some(selected) = selected else {
                    return Ok(None);
                };

                if references.iter().any(|reference| !writable(reference)) {
                    return Ok(None);
                }

                project.rename_edits(&references, &selected.name, options, started)?;

                Ok(Some(selected.clone()))
            },
            options,
        )
    }

    /// Builds revision-anchored edits after rechecking all indexed symbol identities.
    ///
    /// Requires complete, error-free analysis and rejects capture, ambiguous contexts,
    /// read-only declarations, and unsupported reference spellings. Files are never written.
    ///
    /// # Errors
    /// Returns unsafe renames, incomplete analysis, source or environment failures,
    /// interruption, or native agreement failures.
    pub fn rename(
        &mut self,
        entry: &Entry,
        offset: usize,
        name: &str,
        roots: &[PathBuf],
        options: &Options,
    ) -> io::Result<Vec<Edit<PathBuf>>> {
        let started = Instant::now();

        self.during(
            |project| {
                if !identifier(name) {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "rename requires one non-keyword Luau identifier",
                    ));
                }

                let selected = project
                    .prepare_rename(entry, offset, &options.remaining(started))?
                    .ok_or_else(|| invalid("symbol cannot be safely renamed"))?;

                if name == selected.name {
                    return Ok(Vec::new());
                }

                let roots = if selected.namespace == Namespace::Binding {
                    &[]
                } else {
                    roots
                };

                let paths = project.index_sources(roots, &options.remaining(started))?;
                let mut entries = project.entries(&paths, &options.remaining(started))?;
                entries.insert(0, entry.clone());
                let original = project.rename_index(&entries, options, started)?;

                let references =
                    project.references(entry, offset, roots, &options.remaining(started))?;

                let edits = project.rename_edits(&references, name, options, started)?;
                let mut validation = project.propose(&edits, options, started)?;

                let updated = validation.during(
                    |validation| {
                        let sources = entries
                            .iter()
                            .map(|entry| entry.source.clone())
                            .collect::<BTreeSet<_>>()
                            .into_iter()
                            .collect::<Vec<_>>();

                        let entries = validation.entries(&sources, &options.remaining(started))?;

                        validation.rename_index(&entries, options, started)
                    },
                    &options.remaining(started),
                )?;

                project.view.consult(&validation.watch_inputs());
                equivalent(&original, &updated, &edits, name, options, started)?;

                Ok(edits)
            },
            options,
        )
    }

    fn rename_index(
        &mut self,
        entries: &[Entry],
        options: &Options,
        started: Instant,
    ) -> io::Result<Vec<Reference<Origin>>> {
        for entry in entries {
            options.check(started)?;
            let (name, preparation) = self.semantic(entry, options, started)?;

            let frontend = self
                .frontend
                .as_mut()
                .ok_or_else(|| invalid("shared native frontend is unavailable"))?;

            let checked = frontend.check(&[name], &options.remaining(started))?;

            if let Completion::Incomplete(reason) = checked.completion {
                return Err(reason.error());
            }

            if let Some(diagnostic) = checked.diagnostics.first() {
                let location = locate(&diagnostic.location, &preparation.report.sources)?;

                return Err(invalid(format!(
                    "{}:{}..{}: rename requires error-free analysis: {}",
                    location.module.source().display(),
                    location.range[0],
                    location.range[1],
                    diagnostic.message,
                )));
            }
        }

        self.reference_entries(entries, &mut BTreeSet::new(), options, started)
    }

    fn rename_edits(
        &mut self,
        references: &[Reference<Origin>],
        name: &str,
        options: &Options,
        started: Instant,
    ) -> io::Result<Vec<Edit<PathBuf>>> {
        let mut edits = Vec::new();

        for reference in references {
            options.check(started)?;

            if !writable(reference) {
                return Err(invalid(
                    "rename includes an ambiguous or read-only declaration",
                ));
            }

            let path = reference.location.module.source().to_path_buf();

            if self
                .entries(slice::from_ref(&path), &options.remaining(started))?
                .len()
                != 1
            {
                return Err(invalid(
                    "rename includes a source with multiple module contexts",
                ));
            }

            let source = self.source(&path)?;

            if source.revision != reference.location.revision {
                return Err(invalid("rename source revision mismatch"));
            }

            let range = reference.location.range;

            let text = source
                .text
                .get(range[0]..range[1])
                .ok_or_else(|| invalid("invalid rename range"))?;

            let replacement = if text == reference.name {
                name.to_owned()
            } else if reference.literal
                && vermis::parse(text.as_bytes())
                    .tokens
                    .first()
                    .is_some_and(|token| {
                        matches!(token.kind, TokenKind::QuotedString | TokenKind::RawString)
                            && token.span.start == 0
                            && token.span.end == text.len()
                    })
            {
                quote(name)
            } else {
                return Err(invalid("reference spelling cannot be safely renamed"));
            };

            edits.push(Edit {
                location: Location {
                    module: path,
                    revision: source.revision,
                    range,
                },
                text: replacement,
            });
        }

        edits.sort_by(|left, right| {
            left.location
                .module
                .cmp(&right.location.module)
                .then_with(|| left.location.range.cmp(&right.location.range))
        });

        edits.dedup();

        if edits.windows(2).any(|pair| {
            pair[0].location.module == pair[1].location.module
                && pair[0].location.range[1] > pair[1].location.range[0]
        }) {
            return Err(invalid("rename edits overlap"));
        }

        Ok(edits)
    }
}

fn writable(reference: &Reference<Origin>) -> bool {
    reference.namespace != Namespace::Receiver
        && reference.symbol.is_none()
        && matches!(reference.targets.as_slice(), [target] if matches!(target.module, Origin::Module(_)))
        && matches!(reference.location.module, Origin::Module(_))
}

fn equivalent(
    original: &[Reference<Origin>],
    updated: &[Reference<Origin>],
    edits: &[Edit<PathBuf>],
    name: &str,
    options: &Options,
    started: Instant,
) -> io::Result<()> {
    if original.len() != updated.len() {
        return Err(invalid("rename changes symbol resolution"));
    }

    for reference in original {
        options.check(started)?;
        let range = relocated(&reference.location, edits)?;

        let matching = updated
            .iter()
            .find(|candidate| {
                same_owner(&reference.location.module, &candidate.location.module)
                    && candidate.location.range == range
                    && candidate.namespace == reference.namespace
            })
            .ok_or_else(|| invalid("rename changes a source identity"))?;

        let renamed = edits.iter().any(|edit| {
            edit.location.module == reference.location.module.source()
                && edit.location.range == reference.location.range
        });

        if matching.name != if renamed { name } else { &reference.name }
            || matching.symbol != reference.symbol
            || matching.declaration != reference.declaration
            || matching.write != reference.write
            || matching.targets.len() != reference.targets.len()
        {
            return Err(invalid("rename changes a binding or member identity"));
        }

        for target in &reference.targets {
            let range = relocated(target, edits)?;

            if !matching.targets.iter().any(|candidate| {
                same_owner(&target.module, &candidate.module) && candidate.range == range
            }) {
                return Err(invalid("rename would capture or shadow another symbol"));
            }
        }
    }

    Ok(())
}

fn same_owner(left: &Origin, right: &Origin) -> bool {
    if left.source() != right.source() {
        return false;
    }

    match (left, right) {
        (Origin::Module(left), Origin::Module(right)) => match (&left.identity, &right.identity) {
            (
                Identity::Instance {
                    map: left_map,
                    node: left_node,
                    ..
                },
                Identity::Instance {
                    map: right_map,
                    node: right_node,
                    ..
                },
            ) => left_map == right_map && left_node == right_node,

            (left, right) => left == right,
        },

        (Origin::Definition(_), Origin::Definition(_)) => true,
        _ => false,
    }
}

fn relocated(location: &Location<Origin>, edits: &[Edit<PathBuf>]) -> io::Result<[usize; 2]> {
    let mut range = location.range;

    for edit in edits
        .iter()
        .filter(|edit| edit.location.module == location.module.source())
    {
        for (original, offset) in location.range.into_iter().zip(&mut range) {
            if edit.location.range[0] < original && original < edit.location.range[1] {
                return Err(invalid("rename crosses a native source boundary"));
            }

            if edit.location.range[1] <= original {
                *offset = offset
                    .checked_add(edit.text.len())
                    .and_then(|offset| {
                        offset.checked_sub(edit.location.range[1] - edit.location.range[0])
                    })
                    .ok_or_else(|| invalid("rename offset overflow"))?;
            }
        }
    }

    Ok(range)
}
