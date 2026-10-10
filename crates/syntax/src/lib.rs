//! Snapshot-based lossless Luau syntax operations.

pub mod bindings;
mod directives;
mod identifier;
mod imports;
pub mod literal;
mod parsing;
mod preservation;
mod selection;
mod settings;

pub use vermis::{emitter, token, tree};

pub use bindings::{Binding, BindingResult, Category, ExecutionScope, bindings};
pub use directives::{Directive, DirectiveKind, DirectiveResult, directives};
pub use identifier::{identifier, keyword};
pub use imports::{SyntaxImport, imports};
pub use parsing::{SyntaxResult, Tree, parse};
pub use preservation::{Comparison, PreservationResult, preserve, preserve_mapped};
pub use selection::{SelectionResult, SyntaxSelection, expand, selections};
pub use settings::SyntaxSettings;
