use instar_luau::{ModuleIdentity, SemanticIdentity};
use instar_shared::{SourceIdentity, SourceVersion, Span};

use crate::{inputs::ProjectSnapshot, request::Input};

/// An editor query over an exact project snapshot.
#[derive(Clone, Debug)]
pub struct QueryRequest {
    /// Captured source revisions and project inputs.
    pub snapshot: ProjectSnapshot,

    /// Query to execute against the snapshot.
    pub operation: Query,
}

/// A read-only project query.
#[derive(Clone, Debug)]
pub enum Query {
    /// Run the requested checking and linting analyses.
    Diagnostics(DiagnosticQuery),

    /// Inspect semantic information at the requested position.
    Inspect(PositionQuery),

    /// Complete the requested source position.
    Complete(CompletionQuery),

    /// Find occurrences referring to the selected symbol.
    References(ReferenceQuery),

    /// List source symbols or search workspace symbols.
    Symbols(SymbolQuery),

    /// Inspect call or nominal-type relationships.
    Hierarchy(HierarchyQuery),

    /// Find import links in the source revision.
    Links(LinkQuery),

    /// Derive source presentation information.
    Presentation(PresentationQuery),

    /// Retrieve captured contents for the source identity.
    Content(SourceIdentity),

    /// Resolve an exact native documentation identifier.
    Documentation {
        /// Source revision whose effective environment supplies documentation.
        source: SourceVersion,
        /// Namespaced native documentation identifier.
        symbol: String,
    },
}

/// Checking and linting requested for selected project inputs.
#[derive(Clone, Debug)]
pub struct DiagnosticQuery {
    /// Source or discovery inputs; an empty list selects project roots.
    pub inputs: Vec<Input>,

    /// Run the effective lint rules.
    pub lint: bool,

    /// Run source checking.
    pub checking: bool,
}

/// A semantic query anchored to one source revision and module context.
#[derive(Clone, Debug)]
pub struct PositionQuery {
    /// Logical module context containing the source.
    pub module: ModuleIdentity,

    /// Exact source revision to inspect.
    pub source: SourceVersion,

    /// UTF-8 byte offset in the source.
    pub offset: usize,

    /// Requested inspection or assistance operation.
    pub kind: PositionQueryKind,
}

/// Semantic information requested at a captured source position.
#[derive(Clone, Copy, Debug)]
pub enum PositionQueryKind {
    /// Inspect the selected occurrence's type and documentation.
    Hover,

    /// Inspect callable signatures and active arguments.
    Signatures,

    /// Locate the selected occurrence's declarations.
    Declaration,

    /// Locate the selected occurrence's definitions.
    Definition,

    /// Locate declarations of the selected occurrence's type.
    TypeDefinition,

    /// Locate indexed implementations.
    Implementation,

    /// Inspect the selected occurrence for rename preparation.
    PrepareRename,

    /// Find inferred-type hints in the source.
    Hints,

    /// Find reference-count lenses in the source.
    Lenses,

    /// Find source variables suitable for inline display.
    InlineValues,
}

/// Completion requested at a captured source position.
#[derive(Clone, Debug)]
pub struct CompletionQuery {
    /// Source revision, module context and cursor offset.
    pub position: PositionQuery,

    /// Request inline completion without automatic-import suggestions.
    pub inline: bool,
}

/// Reference lookup for the symbol at a captured source position.
#[derive(Clone, Debug)]
pub struct ReferenceQuery {
    /// Occurrence selecting the referenced symbol.
    pub position: PositionQuery,

    /// Include declaration occurrences.
    pub include_declaration: bool,

    /// Index sources across the workspace.
    pub workspace: bool,

    /// Restrict returned occurrences to the selected source.
    pub highlights: bool,
}

/// Source symbol listing or workspace symbol search.
#[derive(Clone, Debug)]
pub struct SymbolQuery {
    /// Selected source, or the workspace when absent.
    pub source: Option<SourceIdentity>,

    /// Case-insensitive name substring; empty matches every symbol.
    pub search: String,
}

/// Call or type hierarchy lookup for selected semantic identities.
#[derive(Clone, Debug)]
pub struct HierarchyQuery {
    /// Position selecting identities when no explicit identity is supplied.
    pub position: Option<PositionQuery>,

    /// Explicit selected identity, taking precedence over the position.
    pub identity: Option<SemanticIdentity>,

    /// Requested relationship or hierarchy preparation.
    pub kind: HierarchyQueryKind,
}

/// Call or nominal-type hierarchy operation.
#[derive(Clone, Copy, Debug)]
pub enum HierarchyQueryKind {
    /// Select functions for call hierarchy exploration.
    PrepareCalls,

    /// Find calls targeting the selected functions.
    IncomingCalls,

    /// Find calls made by the selected functions.
    OutgoingCalls,

    /// Select nominal types for type hierarchy exploration.
    PrepareTypes,

    /// Find parents of the selected nominal types.
    Supertypes,

    /// Find children of the selected nominal types.
    Subtypes,
}

/// Import-link lookup for one captured source revision.
#[derive(Clone, Debug)]
pub struct LinkQuery {
    /// Exact source revision containing the imports.
    pub source: SourceVersion,
}

/// Presentation information requested for one source revision.
#[derive(Clone, Debug)]
pub struct PresentationQuery {
    /// Exact source revision to present.
    pub source: SourceVersion,

    /// Requested presentation operation.
    pub kind: PresentationKind,
}

/// Source presentation operation.
#[derive(Clone, Debug)]
pub enum PresentationKind {
    /// Derive semantic token classifications.
    Tokens {
        /// Optional UTF-8 byte span; absent selects the whole source.
        range: Option<Span>,
    },

    /// Find foldable syntax regions.
    Folds,

    /// Find enclosing selection spans for each supplied UTF-8 byte offset.
    Selections(Vec<usize>),

    /// Find supported Roblox color expressions.
    Colors,

    /// Render replacement spellings for a supported color expression.
    Color {
        /// UTF-8 byte span of the original color expression.
        range: Span,
        /// Requested red, green, blue and alpha channels in the range zero to one.
        rgba: [f64; 4],
    },
}
