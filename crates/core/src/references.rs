use std::{collections::BTreeSet, io, path::PathBuf, time::Instant};

use instar_analysis::{Options, error::invalid};
use instar_bridge::frontend::{Namespace, Reference};

use crate::{
    analysis::{Entry, Origin, locate},
    project::Project,
    resolve::Identity,
};

impl Project {
    /// Finds native references in the entry graph and workspace roots.
    ///
    /// Empty roots restrict discovery to the entry's reachable graph.
    ///
    /// # Errors
    /// Returns invalid positions, ambiguous contexts, indexing or environment failures,
    /// interruption, or native source agreement failures.
    pub fn references(
        &mut self,
        entry: &Entry,
        offset: usize,
        roots: &[PathBuf],
        options: &Options,
    ) -> io::Result<Vec<Reference<Origin>>> {
        let started = Instant::now();

        self.during(
            |project| {
                let source = project.source(&entry.source)?;

                if !source.text.is_char_boundary(offset) {
                    return Err(invalid("invalid reference position"));
                }

                let mut visited = BTreeSet::new();

                let (origin, references) =
                    project.reference_index(entry, &mut visited, options, started)?;

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
                    })
                    .cloned();

                let Some(selected) = selected else {
                    return Ok(Vec::new());
                };

                let mut found = references
                    .into_iter()
                    .filter(|reference| related(reference, &selected))
                    .collect::<Vec<_>>();

                let paths = project.workspace_sources(roots, &options.remaining(started))?;
                let entries = project.entries(&paths, &options.remaining(started))?;

                found.extend(
                    project
                        .reference_entries(&entries, &mut visited, options, started)?
                        .into_iter()
                        .filter(|reference| related(reference, &selected)),
                );

                if selected.namespace != Namespace::Receiver {
                    for target in &selected.targets {
                        options.check(started)?;

                        if !found.iter().any(|reference| {
                            reference.declaration && reference.targets.contains(target)
                        }) {
                            found.push(Reference {
                                location: target.clone(),
                                targets: vec![target.clone()],
                                symbol: None,
                                name: selected.name.clone(),
                                namespace: selected.namespace,
                                declaration: true,
                                write: true,
                                literal: false,
                                classification: selected.classification,
                            });
                        }
                    }
                }

                found.sort_by(|left, right| {
                    left.location
                        .module
                        .source()
                        .cmp(right.location.module.source())
                        .then_with(|| left.location.range.cmp(&right.location.range))
                });

                found.dedup();
                options.check(started)?;

                Ok(found)
            },
            options,
        )
    }

    pub(crate) fn reference_entries(
        &mut self,
        entries: &[Entry],
        visited: &mut BTreeSet<Identity>,
        options: &Options,
        started: Instant,
    ) -> io::Result<Vec<Reference<Origin>>> {
        let mut references = Vec::new();

        for entry in entries {
            options.check(started)?;
            let module = self.module(&entry.source, entry.context.as_ref())?;

            if !visited.contains(&module.identity) {
                references.extend(self.reference_index(entry, visited, options, started)?.1);
            }
        }

        Ok(references)
    }

    pub(crate) fn reference_index(
        &mut self,
        entry: &Entry,
        visited: &mut BTreeSet<Identity>,
        options: &Options,
        started: Instant,
    ) -> io::Result<(Origin, Vec<Reference<Origin>>)> {
        let (name, preparation) = self.semantic(entry, options, started)?;

        let frontend = self
            .frontend
            .as_mut()
            .ok_or_else(|| invalid("shared native frontend is unavailable"))?;

        let references = frontend.references(&name, &options.remaining(started))?;

        let origin = preparation
            .report
            .sources
            .get(&name)
            .ok_or_else(|| invalid("reference entry is unavailable"))?
            .0
            .clone();

        let mut output = Vec::new();

        for reference in references {
            options.check(started)?;

            output.push(Reference {
                location: locate(&reference.location, &preparation.report.sources)?,
                targets: reference
                    .targets
                    .iter()
                    .map(|target| locate(target, &preparation.report.sources))
                    .collect::<io::Result<_>>()?,
                symbol: reference.symbol,
                name: reference.name,
                namespace: reference.namespace,
                declaration: reference.declaration,
                write: reference.write,
                literal: reference.literal,
                classification: reference.classification,
            });
        }

        for (origin, _) in preparation.report.sources.values() {
            if let Origin::Module(module) = origin {
                visited.insert(module.identity.clone());
            }
        }

        Ok((origin, output))
    }
}

fn related(reference: &Reference<Origin>, selected: &Reference<Origin>) -> bool {
    reference.namespace == selected.namespace
        && reference.name == selected.name
        && (reference
            .targets
            .iter()
            .any(|target| selected.targets.contains(target))
            || reference
                .symbol
                .as_ref()
                .is_some_and(|symbol| selected.symbol.as_ref() == Some(symbol)))
}
