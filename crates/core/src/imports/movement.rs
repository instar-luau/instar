use std::{
    collections::{BTreeMap, BTreeSet},
    io,
    path::PathBuf,
    rc::Rc,
    time::Instant,
};

use instar_analysis::{Edit, Location, Options, error::invalid};
use instar_syntax::literal;

use super::paths::relative;

use crate::{
    graph::Node,
    project::{Change, Project, absolute},
    resolve::{Identity, Module, Request},
    source::Kind,
};

impl Project {
    /// Plans import replacements for unambiguous filesystem module moves without writing files.
    ///
    /// # Errors
    /// Rejects occupied destinations, ambiguous identities, incomplete syntax, changed resolution,
    /// invalid source paths, configuration failures, or interrupted operations.
    pub fn relocate(
        &mut self,
        movements: &[(PathBuf, PathBuf)],
        roots: &[PathBuf],
        options: &Options,
    ) -> io::Result<Vec<Edit<PathBuf>>> {
        let started = Instant::now();

        self.during(
            |project| {
                let movements = project.movements(movements, options, started)?;

                if movements.is_empty() {
                    return Ok(Vec::new());
                }

                let mut paths = project.index_sources(roots, &options.remaining(started))?;
                paths.extend(movements.keys().cloned());
                paths.sort();
                paths.dedup();
                let entries = project.entries(&paths, &options.remaining(started))?;
                let mut nodes = BTreeMap::new();

                for entry in entries {
                    options.check(started)?;

                    for identity in project.discover(&entry.source, entry.context.as_ref())? {
                        let node = project
                            .graph
                            .node(&identity)
                            .ok_or_else(|| invalid("move dependency snapshot is unavailable"))?;

                        if !node.problems.is_empty() {
                            return Err(invalid("import updates require complete source syntax"));
                        }

                        nodes.insert(identity, node);
                    }
                }

                let nodes = nodes.into_values().collect::<Vec<_>>();
                let mut validation = project.propose(&[], options, started)?;

                for (source, destination) in &movements {
                    options.check(started)?;
                    let text = project.source(source)?.text.to_string();

                    validation.change(Change::Overlay {
                        path: source.clone(),
                        text: None,
                    })?;

                    validation.change(Change::Overlay {
                        path: destination.clone(),
                        text: Some(text),
                    })?;
                }

                let planned = validation.during(
                    |validation| {
                        let edits =
                            validation.movement_edits(&nodes, &movements, options, started)?;

                        let replacements = edits
                            .iter()
                            .map(|edit| {
                                options.check(started)?;

                                let path = movements
                                    .get(&edit.location.module)
                                    .unwrap_or(&edit.location.module)
                                    .clone();

                                let source = validation.source(&path)?;

                                Ok(Edit {
                                    location: Location {
                                        module: path,
                                        revision: source.revision,
                                        range: edit.location.range,
                                    },
                                    text: edit.text.clone(),
                                })
                            })
                            .collect::<io::Result<Vec<_>>>()?;

                        let mut updated = validation.propose(&replacements, options, started)?;

                        let checked = updated.during(
                            |updated| updated.check_movements(&nodes, &movements, options, started),
                            &options.remaining(started),
                        );

                        validation.view.consult(&updated.watch_inputs());
                        checked?;

                        Ok(edits)
                    },
                    &options.remaining(started),
                );

                project.view.consult(&validation.watch_inputs());
                options.check(started)?;

                planned
            },
            options,
        )
    }

    fn movements(
        &mut self,
        movements: &[(PathBuf, PathBuf)],
        options: &Options,
        started: Instant,
    ) -> io::Result<BTreeMap<PathBuf, PathBuf>> {
        let mut destinations = BTreeSet::new();
        let mut sources = BTreeMap::new();

        for (source, destination) in movements {
            options.check(started)?;
            let source = absolute(source)?;
            let destination = absolute(destination)?;

            if source == destination {
                continue;
            }

            if ![&source, &destination].iter().all(|path| {
                matches!(
                    path.extension().and_then(|extension| extension.to_str()),
                    Some("lua" | "luau")
                )
            }) {
                return Err(invalid("import updates require Luau source files"));
            }

            if !destinations.insert(destination.clone())
                || sources
                    .insert(source.clone(), destination.clone())
                    .is_some()
            {
                return Err(invalid(
                    "file moves must have unique sources and destinations",
                ));
            }

            if self.view.kind(&source)? != Some(Kind::File)
                || self.view.kind(&destination)?.is_some()
            {
                return Err(invalid(
                    "file moves require an existing source and an unoccupied destination",
                ));
            }

            if !matches!(
                self.module(&source, None)?.identity,
                Identity::Filesystem(_)
            ) {
                return Err(invalid("file moves require unambiguous filesystem modules"));
            }
        }

        Ok(sources)
    }

    fn movement_edits(
        &mut self,
        nodes: &[Rc<Node>],
        movements: &BTreeMap<PathBuf, PathBuf>,
        options: &Options,
        started: Instant,
    ) -> io::Result<Vec<Edit<PathBuf>>> {
        let mut edits = BTreeMap::<(PathBuf, [usize; 2]), Edit<PathBuf>>::new();

        for node in nodes {
            options.check(started)?;
            let module = moved(self, &node.module, movements)?;

            for site in &node.sites {
                options.check(started)?;

                let (Some(request), Some(target)) = (&site.request, &site.target) else {
                    continue;
                };

                let target = moved(self, target, movements)?;

                if self
                    .resolve(&module, request)
                    .result
                    .is_ok_and(|resolved| resolved == target)
                {
                    continue;
                }

                if !matches!(request, Request::String(_))
                    || !matches!(target.identity, Identity::Filesystem(_))
                {
                    return Err(invalid("import update would change a mapped identity"));
                }

                let parent = module
                    .path
                    .parent()
                    .ok_or_else(|| invalid("moved module has no parent"))?;

                let request = relative(parent, &target.path)
                    .ok_or_else(|| invalid("moved import has no relative path"))?;

                if !self
                    .resolve(&module, &Request::String(request.clone()))
                    .result
                    .is_ok_and(|resolved| resolved == target)
                {
                    return Err(invalid("moved import cannot be resolved unambiguously"));
                }

                let edit = Edit {
                    location: Location {
                        module: node.module.source.clone(),
                        revision: site.revision,
                        range: site.range,
                    },
                    text: literal::quote(&request),
                };

                if let Some(previous) =
                    edits.insert((node.module.source.clone(), site.range), edit.clone())
                    && previous != edit
                {
                    return Err(invalid("import update differs between module contexts"));
                }
            }
        }

        Ok(edits.into_values().collect())
    }

    fn check_movements(
        &mut self,
        nodes: &[Rc<Node>],
        movements: &BTreeMap<PathBuf, PathBuf>,
        options: &Options,
        started: Instant,
    ) -> io::Result<()> {
        for original in nodes {
            options.check(started)?;
            let module = moved(self, &original.module, movements)?;
            let updated = self.links(&module.source, Some(&module.identity))?;

            if !updated.problems.is_empty() || updated.sites.len() != original.sites.len() {
                return Err(invalid("file move changed require syntax"));
            }

            for (before, after) in original.sites.iter().zip(&updated.sites) {
                options.check(started)?;

                let expected = before
                    .target
                    .as_ref()
                    .map(|target| moved(self, target, movements))
                    .transpose()?;

                if expected != after.target || before.request.is_some() != after.request.is_some() {
                    return Err(invalid("file move changed require resolution"));
                }
            }
        }

        Ok(())
    }
}

fn moved(
    project: &mut Project,
    module: &Module,
    movements: &BTreeMap<PathBuf, PathBuf>,
) -> io::Result<Module> {
    let source = movements.get(&module.source).unwrap_or(&module.source);

    match &module.identity {
        Identity::Filesystem(_) => {
            let module = project.module(source, None)?;

            if !matches!(module.identity, Identity::Filesystem(_)) {
                return Err(invalid("file move changed a filesystem module context"));
            }

            Ok(module)
        }

        Identity::Instance { map, node, .. } => {
            if source != &module.source {
                return Err(invalid("mapped source moves require updated source maps"));
            }

            project.contexts(source)?.into_iter().find(|module| {
                matches!(&module.identity, Identity::Instance { map: current, node: index, .. } if current == map && index == node)
            }).ok_or_else(|| invalid("file move changed a mapped module context"))
        }
    }
}
