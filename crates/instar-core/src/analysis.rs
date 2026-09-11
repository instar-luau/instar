use std::{io, path::PathBuf};

use crate::{luau, project::resolution::Resolver, source::SourceStore};

#[derive(
    Clone, Copy, Debug, PartialEq, Eq, serde::Deserialize, serde::Serialize, schemars::JsonSchema,
)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    Strict,
    Nonstrict,
    Nocheck,
}

#[derive(Default)]
pub struct Options {
    pub mode: Option<Mode>,
    pub old_solver: bool,
    pub annotations: bool,
    pub update: bool,
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

pub type Documentation = std::collections::BTreeMap<String, serde_json::Value>;

#[derive(Default)]
pub struct Report {
    pub diagnostics: Vec<Diagnostic>,
    pub annotations: Vec<Annotation>,
    pub documentation: std::collections::BTreeMap<PathBuf, std::sync::Arc<Documentation>>,
}

impl Report {
    #[must_use]
    pub fn has_errors(&self) -> bool {
        self.diagnostics
            .iter()
            .any(|diagnostic| diagnostic.is_error)
    }
}

#[derive(Default)]
pub struct Session {
    native: luau::Session,
}

impl Session {
    /// # Errors
    /// Returns source, configuration, resolution or native callback failures.
    pub fn analyze(
        &mut self,
        sources: &mut SourceStore,
        modules: &[PathBuf],
        options: &Options,
    ) -> io::Result<Report> {
        self.native
            .analyze(&mut Resolver::new(sources), modules, options)
    }
}

/// # Errors
/// Returns source, configuration, resolution or native callback failures.
pub fn analyze(
    resolver: &mut Resolver<'_>,
    modules: &[PathBuf],
    options: &Options,
) -> io::Result<Report> {
    luau::Session::default().analyze(resolver, modules, options)
}
