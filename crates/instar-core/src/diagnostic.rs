//! Source locations, diagnostic severities, and deterministic ordering.

use crate::resolve::Module;

/// Source context and zero-based line/byte-column range of a diagnostic.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Location {
    /// Physical source and optional place/instance identity.
    pub module: Module,

    /// Start line, start column, end line, and end column.
    pub range: [u32; 4],
}

/// Diagnostic severity in CLI and editor output.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    /// Fails a diagnostic command.
    Error,

    /// Reports a non-fatal warning.
    Warning,

    /// Reports non-fatal information.
    Information,
}

/// A diagnostic mapped to its source context.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Diagnostic {
    /// Source location.
    pub location: Location,

    /// CLI and editor severity.
    pub severity: Severity,

    /// Explanation of the finding.
    pub message: String,

    /// Related source location and explanation.
    pub related: Option<(Location, String)>,
}

pub(crate) fn sort(diagnostics: &mut Vec<Diagnostic>) {
    diagnostics.sort_by(|a, b| {
        a.location
            .module
            .source
            .cmp(&b.location.module.source)
            .then_with(|| {
                a.location
                    .module
                    .instance
                    .as_ref()
                    .map(|instance| (instance.sourcemap_path(), instance.full_name()))
                    .cmp(
                        &b.location
                            .module
                            .instance
                            .as_ref()
                            .map(|instance| (instance.sourcemap_path(), instance.full_name())),
                    )
            })
            .then(a.location.range.cmp(&b.location.range))
            .then_with(|| a.message.cmp(&b.message))
    });

    diagnostics.dedup();
}
