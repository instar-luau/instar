use std::{
    collections::{BTreeMap, btree_map},
    io,
    path::PathBuf,
    time::Instant,
};

use instar_analysis::{Edit, Options, error::invalid};

use crate::project::{Change, Project};

impl Project {
    pub(crate) fn propose(
        &mut self,
        edits: &[Edit<PathBuf>],
        options: &Options,
        started: Instant,
    ) -> io::Result<Self> {
        let mut edits = edits.iter().collect::<Vec<_>>();

        edits.sort_by(|left, right| {
            left.location
                .module
                .cmp(&right.location.module)
                .then_with(|| left.location.range.cmp(&right.location.range))
        });

        if edits.windows(2).any(|pair| {
            pair[0].location.module == pair[1].location.module
                && pair[0].location.range[1] > pair[1].location.range[0]
        }) {
            return Err(invalid("proposed source edits overlap"));
        }

        let mut replacements = BTreeMap::<PathBuf, String>::new();

        for edit in edits.into_iter().rev() {
            options.check(started)?;
            let source = self.source(&edit.location.module)?;

            if source.revision != edit.location.revision
                || source
                    .text
                    .get(edit.location.range[0]..edit.location.range[1])
                    .is_none()
            {
                return Err(invalid("proposed source revision mismatch"));
            }

            let text = match replacements.entry(edit.location.module.clone()) {
                btree_map::Entry::Occupied(entry) => entry.into_mut(),
                btree_map::Entry::Vacant(entry) => entry.insert(source.text.to_string()),
            };

            text.replace_range(edit.location.range[0]..edit.location.range[1], &edit.text);
        }

        let mut snapshot = self.view.frozen(options, started)?;

        for (path, text) in replacements {
            options.check(started)?;

            snapshot.change(Change::Overlay {
                path,
                text: Some(text),
            })?;
        }

        let mut project = Self::new(self.timeout);
        project.apply(&snapshot);

        Ok(project)
    }
}
