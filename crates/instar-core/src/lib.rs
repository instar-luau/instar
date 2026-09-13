//! Luau analysis, formatting, linting, builds, and language-server services.

/// Native type checking and editor queries over source snapshots.
pub mod analysis;

/// Build planning, compilation, and artifact publication.
pub mod build;

/// Project configuration and its JSON schema.
pub mod configuration;

/// Source-preserving Luau formatting.
pub mod format;

/// Extension runtimes for formatting, linting, and compilation.
pub mod graft;

/// Configurable lint findings and source edits.
pub mod lint;

/// Language Server Protocol transport and request handling.
pub mod lsp;

mod luau;

/// Configuration discovery, source selection, and module resolution.
pub mod project;

mod roblox;

/// Source snapshots, revisions, and position conversion.
pub mod source;
