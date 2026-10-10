use std::{io, time::Instant};

use instar_analysis::{Location, Options, error::invalid};

use crate::{
    analysis::{Entry, Origin, locate},
    project::Project,
};

impl Project {
    /// Resolves declarations or type declarations using exact native module identities.
    ///
    /// # Errors
    /// Returns invalid positions, ambiguous contexts, unavailable environments,
    /// interruption, or native source agreement failures.
    pub fn definition(
        &mut self,
        entry: &Entry,
        offset: usize,
        type_definition: bool,
        options: &Options,
    ) -> io::Result<Vec<Location<Origin>>> {
        let started = Instant::now();

        self.during(
            |project| {
                let (name, preparation) = project.semantic(entry, options, started)?;

                let frontend = project
                    .frontend
                    .as_mut()
                    .ok_or_else(|| invalid("shared native frontend is unavailable"))?;

                let locations = frontend.definition(
                    &name,
                    offset,
                    type_definition,
                    &options.remaining(started),
                )?;

                options.check(started)?;

                locations
                    .iter()
                    .map(|location| locate(location, &preparation.report.sources))
                    .collect()
            },
            options,
        )
    }
}
