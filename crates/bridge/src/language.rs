use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    slice,
};

use crate::{Error, Mode, bridge::bindings};

/// Inherited native Luau settings parsed independently of a frontend.
#[derive(Clone, Debug)]
pub struct Language {
    native: bindings::Language,
}

impl Language {
    /// Parse a configuration file over its parent's native settings.
    ///
    /// # Errors
    /// Returns malformed configuration or execution failures.
    pub fn parse(
        previous: Option<&Self>,
        name: &str,
        source: &str,
        executable: bool,
    ) -> Result<Self, Error> {
        let previous = previous.map_or(&[][..], |previous| slice::from_ref(&previous.native));

        let native = bindings::configure(previous, name, source, executable).map_err(|error| {
            Error::Native {
                reason: error.what().to_owned(),
            }
        })?;

        Ok(Self { native })
    }

    /// Return the inherited language mode.
    #[must_use]
    pub fn mode(&self) -> Mode {
        match self.native.mode {
            bindings::Mode::Strict => Mode::Strict,
            bindings::Mode::NoCheck => Mode::NoCheck,
            _ => Mode::NonStrict,
        }
    }

    /// Return globals accepted by this configuration scope.
    #[must_use]
    pub fn globals(&self) -> &[String] {
        &self.native.globals
    }

    /// Resolve aliases relative to their defining configuration files.
    #[must_use]
    pub fn aliases(&self) -> BTreeMap<String, PathBuf> {
        self.native
            .aliases
            .iter()
            .map(|alias| {
                let configuration = Path::new(&alias.configuration);
                let directory = configuration.parent().unwrap_or(configuration);

                (alias.name.clone(), directory.join(&alias.value))
            })
            .collect()
    }

    /// Whether this scope reports type errors alongside syntax errors.
    #[must_use]
    pub fn type_errors(&self) -> bool {
        self.native.type_errors
    }
}
