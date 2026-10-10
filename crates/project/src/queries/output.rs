use instar_luau::{
    CompletionCandidate, Completions, FunctionFact, InspectionResult, NominalFact, OccurrenceFact,
    SemanticIdentity,
};

use instar_shared::{Location, SourceSnapshot, Span, TextEdit};

use crate::{
    documentation::DocumentationEntry,
    report::{CheckReport, LintReport, ProjectLink},
    structure::Fold,
    symbols::Symbol,
    tokens::Token,
};

/// Detached results of the requested project query.
#[derive(Debug)]
pub enum QueryOutput {
    /// Requested checking and linting reports.
    Diagnostics {
        /// Checking report when checking was requested.
        check: Option<Box<CheckReport>>,
        /// Linting report when linting was requested.
        lint: Option<Box<LintReport>>,
    },

    /// Native semantic inspection information.
    Inspection(InspectionResult),

    /// Native and project-provided completion candidates.
    Completions(ProjectCompletions),

    /// Revision-anchored navigation targets.
    Locations(Vec<Location>),

    /// Indexed occurrences of the selected symbol.
    References(Vec<OccurrenceFact>),

    /// Source or workspace symbols matching the query.
    Symbols(Vec<ProjectSymbol>),

    /// Selected call or nominal-type relationships.
    Hierarchy(HierarchyOutput),

    /// Import links and their resolution targets.
    Links(Vec<ProjectLink>),

    /// Source presentation information.
    Presentation(PresentationOutput),

    /// Captured contents when the source identity is present.
    Content(Option<SourceSnapshot>),

    /// Source hints, lenses or inline values.
    Assistance(AssistanceOutput),

    /// Documentation matching the exact native identifier, when present.
    Documentation(Option<DocumentationEntry>),
}

/// A source symbol with its captured project location.
#[derive(Clone, Debug)]
pub struct ProjectSymbol {
    /// Revision-anchored symbol range.
    pub location: Location,

    /// Syntax-derived symbol information and children.
    pub symbol: Symbol,

    /// Optional enclosing symbol name.
    pub container: Option<String>,
}

/// Semantic facts and relationships returned for a hierarchy query.
#[derive(Clone, Debug, Default)]
pub struct HierarchyOutput {
    /// Function facts for selected and related call identities.
    pub functions: Vec<FunctionFact>,

    /// Nominal-type facts for selected and related type identities.
    pub types: Vec<NominalFact>,

    /// Indexed calls matching the requested relationship.
    pub calls: Vec<HierarchyCall>,

    /// Identities selected by the query.
    pub selected: Vec<SemanticIdentity>,
}

/// An indexed call between semantic identities.
#[derive(Clone, Debug)]
pub struct HierarchyCall {
    /// Calling function identity.
    pub from: SemanticIdentity,

    /// Target function identity.
    pub to: SemanticIdentity,

    /// Revision-anchored call-site range.
    pub location: Location,
}

/// Source information prepared for editor presentation.
#[derive(Clone, Debug)]
pub enum PresentationOutput {
    /// Classified tokens for the requested source span.
    Tokens {
        /// Captured source revision and requested byte span.
        location: Location,
        /// Token classifications and source byte ranges.
        entries: Vec<Token>,
    },

    /// Foldable syntax regions.
    Folds(Vec<Fold>),

    /// Enclosing spans, ordered from inner to outer, for each requested position.
    Selections(Vec<Vec<Span>>),

    /// Supported color expressions and their channel values.
    Colors(Vec<SourceColor>),

    /// Replacement spellings for the requested color.
    ColorRepresentations(Vec<ColorRepresentation>),
}

/// A supported color expression anchored to its source revision.
#[derive(Clone, Debug)]
pub struct SourceColor {
    /// Source range containing the color expression.
    pub location: Location,

    /// Red, green, blue and alpha channels in the range zero to one.
    pub rgba: [f64; 4],
}

/// An editor label and source edit for one color spelling.
#[derive(Clone, Debug)]
pub struct ColorRepresentation {
    /// Rendered color expression.
    pub label: String,

    /// Revision-anchored replacement of the original expression.
    pub edit: TextEdit,
}

/// Semantic assistance prepared for a captured source.
#[derive(Clone, Debug)]
pub enum AssistanceOutput {
    /// Inferred-type hints.
    Hints(Vec<ProjectHint>),

    /// Reference-count lenses.
    Lenses(Vec<ProjectLens>),

    /// Variables or expressions suitable for inline display.
    InlineValues(Vec<ProjectInlineValue>),
}

/// An inferred-type hint anchored to its declaration.
#[derive(Clone, Debug)]
pub struct ProjectHint {
    /// Declaration range where the hint is anchored.
    pub location: Location,

    /// Inferred-type label.
    pub label: String,

    /// Optional descriptive text.
    pub description: Option<String>,

    /// Optional native documentation identifier for deferred lookup.
    pub documentation: Option<String>,
}

/// A reference-count lens for an indexed function.
#[derive(Clone, Debug)]
pub struct ProjectLens {
    /// Source range where the lens is anchored.
    pub location: Location,

    /// Displayed reference-count label.
    pub title: String,

    /// Function identity associated with the lens.
    pub identity: SemanticIdentity,
}

/// A captured source value to display inline.
#[derive(Clone, Debug)]
pub struct ProjectInlineValue {
    /// Source range associated with the value.
    pub location: Location,

    /// Variable name for a case-sensitive runtime lookup, when supplied.
    pub variable: Option<String>,

    /// Expression to evaluate when no variable lookup is supplied.
    pub expression: Option<String>,
}

/// Native completion metadata and project-enriched candidates.
#[derive(Clone, Debug)]
pub struct ProjectCompletions {
    /// Native replacement range, context, diagnostics and completion outcome.
    pub native: Completions,

    /// Ordered candidates with optional automatic-import edits.
    pub candidates: Vec<ProjectCandidate>,
}

/// One completion candidate and its additional source edits.
#[derive(Clone, Debug)]
pub struct ProjectCandidate {
    /// Native or project-generated candidate details.
    pub entry: CompletionCandidate,

    /// Additional revision-anchored edits accompanying the completion insertion.
    pub edits: Vec<TextEdit>,
}
