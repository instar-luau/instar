/// Inherited lint settings and rule-specific options.
pub mod configuration;

/// Built-in rule names, groups, defaults, and explanations.
pub mod registry;

mod rules;
mod syntax;

use crate::{
    analysis,
    source::{Source, SourceStore},
};

use configuration::Level;
use serde::{Deserialize, Serialize};
use std::{collections::BTreeSet, io, path::Path};

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
/// Replacement of a half-open byte range in the original source.
pub struct Edit {
    /// First byte replaced by this edit.
    pub start: usize,

    /// First byte after the replaced range.
    pub end: usize,

    /// Replacement source text.
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
/// A lint finding with its configured severity and optional fixes.
pub struct Finding {
    /// Built-in rule name or graft-qualified rule name.
    pub rule: String,

    /// Severity after configuration and suppression processing.
    pub level: Level,

    /// Explanation of the finding.
    pub message: String,

    /// Starting source byte offset.
    pub start: usize,

    /// Exclusive ending source byte offset.
    pub end: usize,

    /// Safe replacement edits offered for this finding.
    pub edits: Vec<Edit>,
}

#[derive(Default)]
/// Findings and native syntax diagnostics for a source snapshot.
pub struct Report {
    /// Normalized configured lint findings.
    pub findings: Vec<Finding>,

    /// Native syntax failures that can prevent safe fixes.
    pub diagnostics: Vec<analysis::Diagnostic>,
}

/// # Errors
/// Returns source, configuration, native analysis, or graft errors.
pub fn analyze(
    session: &mut analysis::Session,
    sources: &mut SourceStore,
    path: &Path,
) -> io::Result<Report> {
    let source = sources.read(path).map_err(io::Error::other)?;
    let configuration = configuration::discover(sources, source.path())?;

    if configuration.settings.enabled == Some(false) {
        return Ok(Report::default());
    }

    let native = session.query(
        sources,
        &[source.path().to_owned()],
        source.path(),
        line_index::LineCol { line: 0, col: 0 },
        "syntax",
    )?;

    let mut report = Report {
        diagnostics: native
            .diagnostics
            .into_iter()
            .filter(|diagnostic| diagnostic.message.starts_with("SyntaxError:"))
            .collect(),
        ..Report::default()
    };

    let entry = match native.editor {
        Some(analysis::EditorResult::Entry(entry)) => Some(*entry),
        Some(analysis::EditorResult::Entries(mut entries)) => entries.pop(),
        None => None,
    };

    let Some(entry) = entry.filter(|entry| entry.description.is_some()) else {
        if report.diagnostics.is_empty() {
            return Err(io::Error::other("native lint syntax unavailable"));
        }

        return Ok(report);
    };

    let document = syntax::decode(
        entry
            .description
            .as_deref()
            .ok_or_else(|| io::Error::other("native lint syntax unavailable"))?,
    )
    .map_err(io::Error::other)?;

    let environment = session.environment(sources, source.path())?;

    let mut globals = entry
        .parameters
        .unwrap_or_default()
        .into_iter()
        .collect::<BTreeSet<_>>();

    globals.extend(configuration.settings.globals.iter().cloned());

    if environment.enabled {
        globals.insert("script".into());
    }

    let mut context = syntax::Context::new(
        &source,
        &document,
        &configuration.settings,
        globals,
        environment.enabled,
    );

    rules::run(&mut context);

    for diagnostic in &report.diagnostics {
        if diagnostic.path == source.path()
            && diagnostic.message.to_ascii_lowercase().contains("escape")
        {
            let location = format!(
                "{},{} - {},{}",
                diagnostic.line, diagnostic.column, diagnostic.end_line, diagnostic.end_column
            );

            if let Some(range) = context.location(&location) {
                context.emit_range(
                    "invalid_string_escape",
                    range,
                    &diagnostic.message,
                    Vec::new(),
                );
            }
        }
    }

    for (name, graft) in configuration.grafts {
        for finding in graft.lint(source.bytes())? {
            context.emit_range(
                &format!("{name}/{}", finding.rule),
                finding.start..finding.end,
                finding.message,
                Vec::new(),
            );
        }
    }

    report.findings = context.findings;
    normalize(&mut report.findings);

    if !report.diagnostics.is_empty() {
        for finding in &mut report.findings {
            finding.edits.clear();
        }
    }

    Ok(report)
}

fn normalize(findings: &mut Vec<Finding>) {
    findings.sort_by(|left, right| {
        (left.start, left.end, &left.rule, &left.message).cmp(&(
            right.start,
            right.end,
            &right.rule,
            &right.message,
        ))
    });

    findings.dedup_by(|left, right| {
        left.start == right.start
            && left.end == right.end
            && left.rule == right.rule
            && left.message == right.message
    });
}

/// # Errors
/// Returns an error when proposed fixes overlap.
pub fn edits(findings: &[Finding]) -> io::Result<Vec<Edit>> {
    let mut edits = findings
        .iter()
        .flat_map(|finding| finding.edits.iter().cloned())
        .collect::<Vec<_>>();

    edits.sort_by(|left, right| {
        (left.start, left.end, &left.text).cmp(&(right.start, right.end, &right.text))
    });

    edits.dedup();

    if edits
        .windows(2)
        .any(|pair| pair[0].end > pair[1].start || pair[0].start == pair[1].start)
    {
        return Err(io::Error::other(
            "lint fixes overlap; apply individual fixes",
        ));
    }

    Ok(edits)
}

/// # Errors
/// Returns invalid source, edit range, overlap, or resulting syntax errors.
pub fn apply(source: &Source, edits: &[Edit]) -> io::Result<Vec<u8>> {
    let mut output = source.text().map_err(io::Error::other)?.to_owned();
    let mut previous = output.len();

    for edit in edits.iter().rev() {
        if edit.start > edit.end
            || edit.end > previous
            || output.get(edit.start..edit.end).is_none()
        {
            return Err(io::Error::other("invalid or overlapping lint edits"));
        }

        output.replace_range(edit.start..edit.end, &edit.text);
        previous = edit.start;
    }

    let tree = vermis::parse(output.as_bytes().into());

    if !tree.diagnostics.is_empty() {
        return Err(io::Error::other("lint fixes produce invalid syntax"));
    }

    Ok(output.into_bytes())
}

/// # Errors
/// Returns stale source, filesystem, or atomic replacement errors.
pub fn write(source: &Source, store: &SourceStore, output: &[u8]) -> io::Result<()> {
    use std::io::Write;
    store.validate(source).map_err(io::Error::other)?;

    if std::fs::read(source.path())? != source.bytes() {
        return Err(io::Error::other("source changed while linting"));
    }

    let directory = source
        .path()
        .parent()
        .ok_or_else(|| io::Error::other("source has no parent"))?;

    let permissions = std::fs::metadata(source.path())?.permissions();
    let mut file = tempfile::NamedTempFile::new_in(directory)?;
    file.write_all(output)?;
    file.as_file().set_permissions(permissions)?;
    file.as_file().sync_all()?;

    if std::fs::read(source.path())? != source.bytes() {
        return Err(io::Error::other(
            "source changed before applying lint fixes",
        ));
    }

    file.persist(source.path()).map_err(|error| error.error)?;

    Ok(())
}
