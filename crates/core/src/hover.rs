use std::{io, time::Instant};

use instar_analysis::{Options, error::invalid};
use instar_bridge::frontend::Hover;

use crate::{
    analysis::{Entry, Origin, locate},
    project::Project,
};

impl Project {
    /// Returns a native type and documentation reference in an exact module context.
    ///
    /// # Errors
    /// Returns ambiguous contexts, invalid positions, unavailable environments,
    /// interruption, or native source agreement failures.
    pub fn hover(
        &mut self,
        entry: &Entry,
        offset: usize,
        options: &Options,
    ) -> io::Result<Option<Hover<Origin>>> {
        let started = Instant::now();

        self.during(
            |project| {
                let (name, preparation) = project.semantic(entry, options, started)?;

                let frontend = project
                    .frontend
                    .as_mut()
                    .ok_or_else(|| invalid("shared native frontend is unavailable"))?;

                let result = frontend.hover(&name, offset, &options.remaining(started))?;

                options.check(started)?;

                result
                    .map(|hover| {
                        Ok(Hover {
                            location: locate(&hover.location, &preparation.report.sources)?,
                            text: hover.text,
                            documentation: hover.documentation,
                            deprecated: hover.deprecated,
                        })
                    })
                    .transpose()
            },
            options,
        )
    }
}
