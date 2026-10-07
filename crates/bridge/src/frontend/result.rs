use std::io;

use instar_analysis::{Completion, Diagnostic, Kind, Location, Related};
use serde::{Deserialize, Serialize};

use crate::boundary;

/// An upstream native warning anchored to a host source revision.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct Warning {
    /// Warning source range.
    pub location: Location<String>,

    /// Upstream warning code.
    pub code: i32,

    /// Upstream warning name.
    pub name: String,

    /// Upstream warning message.
    pub message: String,

    /// Whether native configuration promotes this warning to an error.
    pub fatal: bool,
}

/// An inferred semantic property used by Instar lint rules.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub enum FactKind {
    /// An unannotated local binding inferred as any.
    ImplicitAnyLocal,

    /// An unannotated function parameter inferred as any.
    ImplicitAnyParameter,
}

/// A detached semantic finding from the native type graph.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct Fact {
    /// Binding source range.
    pub location: Location<String>,

    /// Semantic property.
    pub kind: FactKind,

    /// Description of the semantic property.
    pub message: String,
}

/// Native lint output, retaining diagnostics when analysis is incomplete.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct LintResult {
    /// Selected entries and their host-resolved dependencies.
    pub modules: Vec<String>,

    /// Upstream warnings and native fatal promotion.
    pub warnings: Vec<Warning>,

    /// Requested inferred semantic properties.
    pub facts: Vec<Fact>,

    /// Syntax, declaration environment and analysis failures.
    pub diagnostics: Vec<Diagnostic<String>>,

    /// Whether all requested warning and semantic checks completed.
    pub completion: Completion,
}

impl LintResult {
    pub(crate) fn from_native(
        modules: Vec<String>,
        result: boundary::NativeLintResult,
    ) -> io::Result<Self> {
        Ok(Self {
            modules,
            warnings: result
                .warnings
                .into_iter()
                .map(|warning| Warning {
                    location: location(warning.location),
                    code: warning.code,
                    name: warning.name,
                    message: warning.message,
                    fatal: warning.fatal,
                })
                .collect(),
            facts: result
                .facts
                .into_iter()
                .map(fact)
                .collect::<io::Result<_>>()?,
            diagnostics: result
                .diagnostics
                .into_iter()
                .map(diagnostic)
                .collect::<io::Result<_>>()?,
            completion: completion(result.completion)?,
        })
    }
}

pub(crate) fn checking(
    modules: Vec<String>,
    result: boundary::NativeCheck,
) -> io::Result<instar_analysis::Result<String>> {
    Ok(instar_analysis::Result {
        modules,
        diagnostics: result
            .diagnostics
            .into_iter()
            .map(diagnostic)
            .collect::<io::Result<_>>()?,
        completion: completion(result.completion)?,
    })
}

pub(crate) fn completion(value: boundary::NativeCompletion) -> io::Result<Completion> {
    use instar_analysis::Reason;

    match value {
        boundary::NativeCompletion::Complete => Ok(Completion::Complete),
        boundary::NativeCompletion::Cancelled => Ok(Completion::Incomplete(Reason::Cancelled)),
        boundary::NativeCompletion::Timeout => Ok(Completion::Incomplete(Reason::Timeout)),
        boundary::NativeCompletion::Environment => Ok(Completion::Incomplete(Reason::Environment)),
        boundary::NativeCompletion::Analysis => Ok(Completion::Incomplete(Reason::Analysis)),
        _ => Err(io::Error::other("unknown native completion status")),
    }
}

fn fact(value: boundary::NativeFact) -> io::Result<Fact> {
    let kind = match value.kind {
        boundary::NativeFactKind::ImplicitAnyLocal => FactKind::ImplicitAnyLocal,
        boundary::NativeFactKind::ImplicitAnyParameter => FactKind::ImplicitAnyParameter,
        _ => return Err(io::Error::other("unknown native semantic fact kind")),
    };

    Ok(Fact {
        location: location(value.location),
        kind,
        message: value.message,
    })
}

fn location(value: boundary::NativeLocation) -> Location<String> {
    Location {
        module: value.module,
        revision: value.revision,
        range: [value.start, value.end],
    }
}

fn diagnostic(value: boundary::NativeDiagnostic) -> io::Result<Diagnostic<String>> {
    let kind = match value.kind {
        boundary::NativeKind::Syntax => Kind::Syntax {
            code: Some(value.code),
        },

        boundary::NativeKind::Type => Kind::Type { code: value.code },

        boundary::NativeKind::Resolution => Kind::Resolution {
            code: Some(value.code),
        },

        boundary::NativeKind::Analysis => Kind::Analysis {
            code: (value.code != 0).then_some(value.code),
        },

        _ => return Err(io::Error::other("unknown native diagnostic kind")),
    };

    Ok(Diagnostic {
        location: location(value.location),
        kind,
        message: value.message,
        related: value
            .related
            .into_iter()
            .map(|related| Related {
                location: location(related.location),
                message: related.message,
            })
            .collect(),
    })
}
