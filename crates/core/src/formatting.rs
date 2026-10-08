use std::{io, path::Path};

use instar_analysis::{Options, selection};
use instar_format::Configuration;

use crate::project::Project;

impl Project {
    /// Resolves formatter settings, returning `None` for excluded sources.
    ///
    /// # Errors
    /// Returns configuration, selection, or interruption errors.
    pub fn formatting_configuration(
        &mut self,
        path: &Path,
        options: &Options,
    ) -> io::Result<Option<Configuration>> {
        self.during(
            |project| {
                let directory = path.parent().ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "source has no parent directory",
                    )
                })?;

                let settings = project.configuration(directory)?;
                let configuration = &settings.configuration.format;

                if selection::matches(path, configuration.include.as_deref(), true)?
                    && !selection::matches(path, configuration.exclude.as_deref(), false)?
                {
                    Ok(Some(configuration.clone()))
                } else {
                    Ok(None)
                }
            },
            options,
        )
    }
}
