use std::{io, path::PathBuf};

use crate::{native, resolution::Resolver};

/// Overrides of the pinned Luau CLI's checking defaults.
#[derive(Default)]
pub struct Options {
    pub strict: bool,
    pub old_solver: bool,
    pub annotations: bool,
}

pub struct Diagnostic {
    pub path: PathBuf,
    pub line: u32,
    pub column: u32,
    pub message: String,
    pub is_error: bool,
}

pub struct Annotation {
    pub path: PathBuf,
    pub bytes: Vec<u8>,
}

#[derive(Default)]
pub struct Report {
    pub diagnostics: Vec<Diagnostic>,
    pub annotations: Vec<Annotation>,
}

impl Report {
    #[must_use]
    pub fn has_errors(&self) -> bool {
        self.diagnostics
            .iter()
            .any(|diagnostic| diagnostic.is_error)
    }
}

/// Analyze original snapshots using the operation's shared module resolver.
///
/// # Errors
/// Returns source, configuration, resolution or native callback failures.
pub fn analyze(
    resolver: &mut Resolver<'_>,
    modules: &[PathBuf],
    options: &Options,
) -> io::Result<Report> {
    native::analyze(resolver, modules, options)
}
