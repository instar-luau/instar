use std::{
    collections::{BTreeMap, BTreeSet},
    fs, io,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::Arc,
};

use super::{Error, Workspace, instances, process};
use crate::settings::resolution::Generator;

impl Workspace {
    /// Return nonempty generators from all known configuration scopes.
    #[must_use]
    pub fn generators(&self) -> BTreeSet<Generator> {
        self.scopes
            .borrow()
            .values()
            .filter_map(|scope| scope.configuration.resolve.generator.as_ref())
            .filter(|generator| !generator.command.is_empty())
            .cloned()
            .collect()
    }

    /// Run a generator and validate its standard output without changing active maps.
    ///
    /// # Errors
    /// Returns process failures, invalid sourcemaps, or cancellation.
    pub fn generate(&self, generator: &Generator) -> Result<Arc<str>, Error> {
        process::interrupted(self.cancelled)?;

        let executable = generator.command.first().map_or("", String::as_str);

        let failure = |source| Error::Generate {
            executable: executable.to_owned(),
            source,
        };

        if executable.trim().is_empty() {
            return Err(failure(io::Error::new(
                io::ErrorKind::InvalidInput,
                "expected a nonempty executable",
            )));
        }

        let output = tempfile::NamedTempFile::new().map_err(failure)?;
        let diagnostics = tempfile::NamedTempFile::new().map_err(failure)?;

        let program = if Path::new(executable).components().count() > 1 {
            generator.directory.join(executable)
        } else {
            PathBuf::from(executable)
        };

        let mut child = Command::new(program)
            .args(&generator.command[1..])
            .current_dir(&generator.directory)
            .stdin(Stdio::null())
            .stdout(output.reopen().map_err(failure)?)
            .stderr(diagnostics.reopen().map_err(failure)?)
            .spawn()
            .map_err(failure)?;

        let status = process::wait(&mut child, self.cancelled, failure)?;

        if !status.success() {
            let diagnostic = fs::read(diagnostics.path()).map_err(failure)?;

            return Err(failure(io::Error::other(format!(
                "exited with {status}: {}",
                String::from_utf8_lossy(&diagnostic).trim()
            ))));
        }

        process::interrupted(self.cancelled)?;
        let source = fs::read_to_string(output.path()).map_err(failure)?;

        instances::validate(&source).map_err(|source| Error::GeneratedData {
            executable: executable.to_owned(),
            source,
        })?;

        Ok(source.into())
    }

    /// Return cached and preseeded sources, including lazily discovered generators.
    #[must_use]
    pub fn generated(&self) -> BTreeMap<Generator, Arc<str>> {
        self.instances.borrow().generated()
    }

    /// Seed generated sources and clear cached native instance maps.
    pub fn set_generated(&mut self, sources: BTreeMap<Generator, Arc<str>>) {
        self.instances.get_mut().set_generated(sources);
    }
}
