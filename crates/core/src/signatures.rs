use std::{io, time::Instant};

use instar_analysis::{Options, error::invalid};
use instar_bridge::frontend::Signatures;

use crate::{
    analysis::{Entry, Origin, locate},
    project::Project,
};

impl Project {
    /// Returns native call signatures in an exact module context.
    ///
    /// # Errors
    /// Returns ambiguous contexts, invalid positions, unavailable environments,
    /// interruption, or native source agreement failures.
    pub fn signatures(
        &mut self,
        entry: &Entry,
        offset: usize,
        options: &Options,
    ) -> io::Result<Option<Signatures<Origin>>> {
        let started = Instant::now();

        self.during(
            |project| {
                let (name, preparation) = project.semantic(entry, options, started)?;

                let frontend = project
                    .frontend
                    .as_mut()
                    .ok_or_else(|| invalid("shared native frontend is unavailable"))?;

                let result = frontend.signatures(&name, offset, &options.remaining(started))?;
                options.check(started)?;

                result
                    .map(|signatures| {
                        Ok(Signatures {
                            location: locate(&signatures.location, &preparation.report.sources)?,
                            entries: signatures.entries,
                            active: signatures.active,
                            argument: signatures.argument,
                            documentation: signatures.documentation,
                        })
                    })
                    .transpose()
            },
            options,
        )
    }
}
