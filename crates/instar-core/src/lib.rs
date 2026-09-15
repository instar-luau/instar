//! Luau analysis, formatting, linting, builds, and language-server services.

/// Native type checking and editor queries over source snapshots.
pub mod analysis;

/// Build planning, compilation, and artifact publication.
pub mod build;

/// Source-preserving Luau formatting.
pub mod format;

/// Extension runtimes for formatting, linting, and compilation.
pub mod graft;

/// Configurable lint findings and source edits.
pub mod lint;

/// Language Server Protocol transport and request handling.
pub mod lsp;

mod emit;
mod luau;
mod syntax;

/// Configuration discovery, source selection, module resolution, and project environment.
pub mod project;

/// Source snapshots, revisions, and position conversion.
pub mod source;
