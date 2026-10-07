use instar_analysis::{Diagnostic as AnalysisDiagnostic, Kind as AnalysisKind, Location, Related};

use crate::{Level, Rule};

/// Lint diagnostic identity, independent of presentation text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Kind {
    /// A configured Instar rule.
    Rule(Rule),

    /// An upstream warning retaining its native identity.
    Native {
        /// Upstream warning code.
        code: i32,
        /// Upstream warning name.
        name: String,
    },

    /// Shared analysis diagnostic with its original category and native code.
    Analysis(AnalysisKind),
}

/// A revision-anchored lint finding or analysis error.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Diagnostic<Module> {
    /// Exact host source identity and byte range.
    pub location: Location<Module>,

    /// Structured rule or analysis identity.
    pub kind: Kind,

    /// Effective Instar severity or native warning policy.
    pub level: Level,

    /// Explanation of the finding.
    pub message: String,

    /// Additional source context.
    pub related: Vec<Related<Module>>,
}

impl<Module> From<AnalysisDiagnostic<Module>> for Diagnostic<Module> {
    fn from(diagnostic: AnalysisDiagnostic<Module>) -> Self {
        Self {
            location: diagnostic.location,
            kind: Kind::Analysis(diagnostic.kind),
            level: Level::Deny,
            message: diagnostic.message,
            related: diagnostic.related,
        }
    }
}
