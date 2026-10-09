use std::{collections::BTreeSet, io, path::PathBuf};

use instar_analysis::{Options, error::invalid, selection};

use crate::{
    configuration::Operation,
    project::{Project, absolute},
};

impl Project {
    /// Selects sorted, deduplicated diagnostic targets while preserving configuration deadlines.
    ///
    /// # Errors
    /// Returns invalid paths, configuration, selection, or interruption failures.
    pub fn selected_sources(
        &mut self,
        candidates: &[PathBuf],
        operation: Operation,
        options: &Options,
    ) -> io::Result<Vec<PathBuf>> {
        self.during(
            |project| {
                let mut targets = BTreeSet::new();

                for candidate in candidates {
                    project.interrupted()?;
                    let path = absolute(candidate)?;

                    let settings = project.configuration(
                        path.parent()
                            .ok_or_else(|| invalid("selection source has no parent"))?,
                    )?;

                    let configuration = &settings.configuration;

                    let (include, exclude) = match operation {
                        Operation::Check => {
                            (&configuration.check.include, &configuration.check.exclude)
                        }

                        Operation::Lint => {
                            (&configuration.lint.include, &configuration.lint.exclude)
                        }
                    };

                    if selection::selected(&path, include.as_deref(), exclude.as_deref())? {
                        targets.insert(candidate.clone());
                    }
                }

                Ok(targets.into_iter().collect())
            },
            options,
        )
    }
}
