use std::{
    collections::BTreeMap,
    io,
    path::{Path, PathBuf},
    sync::Arc,
};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{luau, project::resolution::Resolver, source::SourceStore};

/// Type-checking mode supplied to Luau.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    /// Check inferred and annotated types strictly.
    Strict,

    /// Use Luau's permissive type-checking mode.
    Nonstrict,

    /// Skip type checking while retaining parsing and enabled lint checks.
    Nocheck,
}

/// Per-operation overrides for native analysis.
#[derive(Default)]
pub struct Options {
    /// Override the configured checking mode.
    pub mode: Option<Mode>,

    /// Select Luau's legacy type solver.
    pub old_solver: bool,

    /// Produce source annotated with inferred types.
    pub annotations: bool,

    /// Retain the native type graph for editor queries without mutating the AST with annotations.
    pub retain_full_type_graphs: bool,

    /// Refresh downloaded Roblox assets for enabled environments.
    pub update: bool,
}

/// A source location providing context for another diagnostic.
pub struct RelatedDiagnostic {
    /// Source containing the related location.
    pub path: PathBuf,

    /// Native start line, start column, end line, and end column.
    pub range: [u32; 4],

    /// Explanation of the related location.
    pub message: String,
}

/// A native analysis error or warning with its source range.
pub struct Diagnostic {
    /// Source that produced the diagnostic.
    pub path: PathBuf,

    /// Native starting line.
    pub line: u32,

    /// Native starting column.
    pub column: u32,

    /// Native ending line.
    pub end_line: u32,

    /// Native ending column.
    pub end_column: u32,

    /// Diagnostic text returned by Luau.
    pub message: String,

    /// Whether the diagnostic contributes to analysis failure.
    pub is_error: bool,

    /// Additional locations explaining this diagnostic.
    pub related: Vec<RelatedDiagnostic>,
}

/// Source rewritten with inferred type annotations.
pub struct Annotation {
    /// Original source path.
    pub path: PathBuf,

    /// Annotated source bytes.
    pub bytes: Vec<u8>,
}

/// Documentation records indexed by their native symbol identifiers.
pub type Documentation = BTreeMap<String, serde_json::Value>;

/// Operation-specific data returned by a native editor query.
#[derive(Clone, Deserialize)]
pub struct EditorEntry {
    /// Symbol or binding name.
    pub name: Option<String>,

    /// Type display or serialized syntax payload for the requested operation.
    #[serde(rename = "type")]
    pub description: Option<String>,

    /// Symbol identifier used to look up documentation.
    pub documentation: Option<String>,

    /// Documentation supplied directly by the query.
    pub documentation_text: Option<String>,

    /// Source containing the result.
    pub path: Option<PathBuf>,

    /// Native source range for the result.
    pub range: Option<[u32; 4]>,

    /// Red, green, blue, and alpha components of a color literal.
    pub color: Option<[f32; 4]>,

    /// Whether the result represents an imported binding.
    pub imports: Option<bool>,

    /// Whether the result represents a require operation.
    pub require: Option<bool>,

    /// Native source range of the calling expression.
    pub caller: Option<[u32; 4]>,

    /// Native source range of the enclosing declaration.
    pub container: Option<[u32; 4]>,

    /// Operation-specific semantic modifier bits.
    pub modifiers: Option<u32>,

    /// Native source range selecting the symbol name.
    pub selection: Option<[u32; 4]>,

    /// Operation-specific symbol or token kind.
    pub kind: Option<u32>,

    /// Whether this occurrence declares the symbol.
    pub declaration: Option<bool>,

    /// Suggested completion insertion text.
    pub insert: Option<String>,

    /// Whether the symbol is deprecated.
    pub deprecated: Option<bool>,

    /// Display label for the result.
    pub label: Option<String>,

    /// Active signature parameter index.
    pub active: Option<u32>,

    /// Parameter labels or operation-specific binding names.
    pub parameters: Option<Vec<String>>,

    /// Query-specific failure description.
    pub error: Option<String>,
}

/// Single-result and list-result forms of native editor queries.
#[derive(Deserialize)]
#[serde(untagged)]
pub enum EditorResult {
    /// A query returning multiple entries.
    Entries(Vec<EditorEntry>),

    /// A query returning one entry.
    Entry(Box<EditorEntry>),
}

/// Diagnostics and optional outputs from an analysis operation.
#[derive(Default)]
pub struct Report {
    /// Errors and warnings produced while checking the requested modules.
    pub diagnostics: Vec<Diagnostic>,

    /// Modules for which native type checking reached a configured time limit.
    pub timeout_hits: Vec<PathBuf>,

    /// Annotated sources when annotation output was requested.
    pub annotations: Vec<Annotation>,

    /// Shared documentation indexed by source path.
    pub documentation: BTreeMap<PathBuf, Arc<Documentation>>,

    pub(crate) links: Vec<EditorEntry>,

    /// Result of an optional editor query.
    pub editor: Option<EditorResult>,
}

impl Report {
    /// Whether any diagnostic is classified as an error.
    #[must_use]
    pub fn has_errors(&self) -> bool {
        self.diagnostics
            .iter()
            .any(|diagnostic| diagnostic.is_error)
    }
}

/// Reusable native analysis state.
#[derive(Default)]
pub struct Session {
    native: luau::Session,
}

impl Session {
    pub(crate) fn environment(
        &mut self,
        sources: &mut SourceStore,
        path: &Path,
    ) -> io::Result<Arc<crate::project::roblox::Environment>> {
        let mut resolver = Resolver::new(sources);
        let settings = resolver.discovery.roblox(path)?;

        self.native
            .environment(&mut resolver, settings.as_ref(), Options::default().update)
    }

    /// # Errors
    /// Returns analysis or native query failures.
    pub fn query(
        &mut self,
        sources: &mut SourceStore,
        modules: &[PathBuf],
        path: &Path,
        position: line_index::LineCol,
        operation: &str,
    ) -> io::Result<Report> {
        self.query_with_diagnostics(sources, modules, path, position, operation, true)
    }

    pub(crate) fn editor_query(
        &mut self,
        sources: &mut SourceStore,
        modules: &[PathBuf],
        path: &Path,
        position: line_index::LineCol,
        operation: &str,
    ) -> io::Result<Report> {
        self.query_with_diagnostics(sources, modules, path, position, operation, false)
    }

    fn query_with_diagnostics(
        &mut self,
        sources: &mut SourceStore,
        modules: &[PathBuf],
        path: &Path,
        position: line_index::LineCol,
        operation: &str,
        collect_diagnostics: bool,
    ) -> io::Result<Report> {
        let mut resolver = Resolver::new(sources);

        let Some(position) = resolver.generated_position(path, position)? else {
            return Ok(Report::default());
        };

        let mut report = self.native.query(
            &mut resolver,
            modules,
            path,
            position,
            operation,
            collect_diagnostics,
        )?;

        map_report(&resolver, path, &mut report)?;

        Ok(report)
    }

    pub(crate) fn parse(&mut self, sources: &mut SourceStore, path: &Path) -> io::Result<Report> {
        self.parse_with_definitions(sources, path, false)
    }

    pub(crate) fn parse_with_definitions(
        &mut self,
        sources: &mut SourceStore,
        path: &Path,
        load_definitions: bool,
    ) -> io::Result<Report> {
        let mut resolver = Resolver::new(sources);
        let mut report = self.native.parse(&mut resolver, path, load_definitions)?;
        map_report(&resolver, path, &mut report)?;

        Ok(report)
    }

    /// Invalidate cached native state after environment or configuration changes.
    pub fn refresh(&mut self) {
        self.native.refresh();
    }

    /// # Errors
    /// Returns source, configuration, resolution or native callback failures.
    pub fn analyze(
        &mut self,
        sources: &mut SourceStore,
        modules: &[PathBuf],
        options: &Options,
    ) -> io::Result<Report> {
        let mut resolver = Resolver::new(sources);
        let mut report = self.native.analyze(&mut resolver, modules, options)?;

        map_report(
            &resolver,
            modules.first().map_or(Path::new(""), PathBuf::as_path),
            &mut report,
        )?;

        Ok(report)
    }
}

/// # Errors
/// Returns source, configuration, resolution or native callback failures.
pub fn analyze(
    resolver: &mut Resolver<'_>,
    modules: &[PathBuf],
    options: &Options,
) -> io::Result<Report> {
    let mut report = luau::Session.analyze_including_definitions(resolver, modules, options)?;

    map_report(
        resolver,
        modules.first().map_or(Path::new(""), PathBuf::as_path),
        &mut report,
    )?;

    Ok(report)
}

fn map_report(resolver: &Resolver<'_>, path: &Path, report: &mut Report) -> io::Result<()> {
    let mut diagnostics = Vec::with_capacity(report.diagnostics.len());

    for mut diagnostic in std::mem::take(&mut report.diagnostics) {
        if diagnostic.path.as_os_str().is_empty() {
            diagnostics.push(diagnostic);
            continue;
        }

        let Some([line, column, end_line, end_column]) = resolver.original_range(
            &diagnostic.path,
            [
                diagnostic.line,
                diagnostic.column,
                diagnostic.end_line,
                diagnostic.end_column,
            ],
        )?
        else {
            continue;
        };

        diagnostic.line = line;
        diagnostic.column = column;
        diagnostic.end_line = end_line;
        diagnostic.end_column = end_column;

        let mut related_diagnostics = Vec::with_capacity(diagnostic.related.len());

        for mut related in diagnostic.related {
            let Some(range) = resolver.original_range(&related.path, related.range)? else {
                continue;
            };

            related.range = range;
            related_diagnostics.push(related);
        }

        diagnostic.related = related_diagnostics;
        diagnostics.push(diagnostic);
    }

    report.diagnostics = diagnostics;

    let map_entry = |entry: &mut EditorEntry| -> io::Result<bool> {
        let path = entry.path.as_deref().unwrap_or(path);

        for coordinates in [
            &mut entry.range,
            &mut entry.caller,
            &mut entry.container,
            &mut entry.selection,
        ]
        .into_iter()
        .flatten()
        {
            let Some(mapped) = resolver.original_range(path, *coordinates)? else {
                return Ok(false);
            };

            *coordinates = mapped;
        }

        Ok(true)
    };

    match report.editor.take() {
        Some(EditorResult::Entries(entries)) => {
            let mut mapped = Vec::with_capacity(entries.len());

            for mut entry in entries {
                if map_entry(&mut entry)? {
                    mapped.push(entry);
                }
            }

            report.editor = Some(EditorResult::Entries(mapped));
        }

        Some(EditorResult::Entry(mut entry)) => {
            if map_entry(&mut entry)? {
                report.editor = Some(EditorResult::Entry(entry));
            }
        }

        None => {}
    }

    let mut links = std::mem::take(&mut report.links);
    let mut mapped_links = Vec::with_capacity(links.len());

    for mut entry in links.drain(..) {
        if map_entry(&mut entry)? {
            mapped_links.push(entry);
        }
    }

    report.links = mapped_links;

    Ok(())
}
