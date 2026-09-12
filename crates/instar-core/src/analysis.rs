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

pub struct RelatedDiagnostic {
    pub path: PathBuf,
    pub range: [u32; 4],
    pub message: String,
}

pub struct Diagnostic {
    pub path: PathBuf,
    pub line: u32,
    pub column: u32,
    pub end_line: u32,
    pub end_column: u32,
    pub message: String,
    pub is_error: bool,
    pub related: Vec<RelatedDiagnostic>,
}

pub struct Annotation {
    pub path: PathBuf,
    pub bytes: Vec<u8>,
}

pub type Documentation = std::collections::BTreeMap<String, serde_json::Value>;

#[derive(Clone, serde::Deserialize)]
pub struct EditorEntry {
    pub name: Option<String>,

    #[serde(rename = "type")]
    pub description: Option<String>,

    pub documentation: Option<String>,
    pub documentation_text: Option<String>,
    pub path: Option<PathBuf>,
    pub range: Option<[u32; 4]>,
    pub color: Option<[f32; 4]>,
    pub imports: Option<bool>,
    pub caller: Option<[u32; 4]>,
    pub container: Option<[u32; 4]>,
    pub modifiers: Option<u32>,
    pub selection: Option<[u32; 4]>,
    pub kind: Option<u32>,
    pub declaration: Option<bool>,
    pub insert: Option<String>,
    pub deprecated: Option<bool>,
    pub label: Option<String>,
    pub active: Option<u32>,
    pub parameters: Option<Vec<String>>,
    pub error: Option<String>,
}

#[derive(serde::Deserialize)]
#[serde(untagged)]
pub enum EditorResult {
    Entries(Vec<EditorEntry>),
    Entry(Box<EditorEntry>),
}

#[derive(Default)]
pub struct Report {
    pub diagnostics: Vec<Diagnostic>,
    pub annotations: Vec<Annotation>,
    pub documentation: std::collections::BTreeMap<PathBuf, std::sync::Arc<Documentation>>,
    pub editor: Option<EditorResult>,
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
    /// Returns analysis or native query failures.
    pub fn query(
        &mut self,
        sources: &mut SourceStore,
        modules: &[PathBuf],
        path: &std::path::Path,
        position: line_index::LineCol,
        operation: &str,
    ) -> io::Result<Report> {
        self.native.query(
            &mut Resolver::new(sources),
            modules,
            path,
            position,
            operation,
        )
    }

    pub fn refresh(&mut self) {
        self.native.refresh();
    }

    pub fn change(&mut self, path: &std::path::Path) {
        self.native.change(path);
    }

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
