use std::{collections::BTreeSet, io, path::PathBuf, time::Instant};

use instar_analysis::{Options, error::invalid};
use instar_bridge::frontend::{Call, Hierarchy, HierarchyItem};

use crate::{
    analysis::{Entry, Origin, locate},
    project::Project,
    resolve::Identity,
};

impl Project {
    /// Returns native hierarchies for the entry and selected editor roots.
    ///
    /// Only the entry graph contains a position selection. Empty roots restrict
    /// discovery to that graph.
    ///
    /// # Errors
    /// Returns invalid positions, environment or indexing failures, source
    /// disagreement, interruption, or native worker failures.
    pub fn hierarchy(
        &mut self,
        entry: &Entry,
        offset: usize,
        roots: &[PathBuf],
        options: &Options,
    ) -> io::Result<Vec<Hierarchy<Origin>>> {
        let started = Instant::now();

        self.during(
            |project| {
                if !project.source(&entry.source)?.text.is_char_boundary(offset) {
                    return Err(invalid("invalid hierarchy position"));
                }

                let mut visited = BTreeSet::new();

                let selected =
                    project.hierarchy_index(entry, Some(offset), &mut visited, options, started)?;

                if selected.selected.is_empty() {
                    return Ok(vec![selected]);
                }

                let mut graphs = vec![selected];
                let paths = project.index_sources(roots, &options.remaining(started))?;
                let entries = project.entries(&paths, &options.remaining(started))?;

                for entry in entries {
                    options.check(started)?;
                    let module = project.module(&entry.source, entry.context.as_ref())?;

                    if !visited.contains(&module.identity) {
                        graphs.push(project.hierarchy_index(
                            &entry,
                            None,
                            &mut visited,
                            options,
                            started,
                        )?);
                    }
                }

                options.check(started)?;

                Ok(graphs)
            },
            options,
        )
    }

    fn hierarchy_index(
        &mut self,
        entry: &Entry,
        offset: Option<usize>,
        visited: &mut BTreeSet<Identity>,
        options: &Options,
        started: Instant,
    ) -> io::Result<Hierarchy<Origin>> {
        let (name, preparation) = self.semantic(entry, options, started)?;

        let frontend = self
            .frontend
            .as_mut()
            .ok_or_else(|| invalid("shared native frontend is unavailable"))?;

        let hierarchy = frontend.hierarchy(&name, offset, &options.remaining(started))?;
        let sources = &preparation.report.sources;
        let mut items = Vec::new();

        for item in hierarchy.items {
            options.check(started)?;

            items.push(HierarchyItem {
                name: item.name,
                kind: item.kind,
                location: locate(&item.location, sources)?,
                selection: item.selection,
            });
        }

        let mut calls = Vec::new();

        for call in hierarchy.calls {
            options.check(started)?;

            calls.push(Call {
                caller: call.caller,
                callee: call.callee,
                location: locate(&call.location, sources)?,
            });
        }

        for (origin, _) in sources.values() {
            if let Origin::Module(module) = origin {
                visited.insert(module.identity.clone());
            }
        }

        Ok(Hierarchy {
            items,
            calls,
            inheritance: hierarchy.inheritance,
            selected: hierarchy.selected,
        })
    }
}
