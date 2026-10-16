use std::ops::Range;

use instar_luau::Warning;

use crate::{Edit, Error, Finding, Prepared};

/// Apply scoped rule policies to findings from the persistent Luau frontend.
///
/// # Errors
/// Rejects native locations that do not identify valid UTF-8 source ranges.
pub fn analyze(
    source: &str,
    warnings: &[Warning],
    settings: &Prepared,
) -> Result<Vec<Finding>, Error> {
    let mut lines = vec![0];
    lines.extend(source.match_indices('\n').map(|(offset, _)| offset + 1));
    let mut findings = Vec::new();

    for warning in warnings {
        let Some(policy) = settings.enabled(&warning.rule) else {
            continue;
        };

        let subject = &warning.subject;

        let message = match warning.rule.as_str() {
            "unused_variable" => {
                if warning.category == "parameter" && !settings.unused.parameters
                    || warning.category == "loop" && !settings.unused.loop_variables
                    || settings.ignored.is_match(subject)
                {
                    continue;
                }

                warning.diagnostic.message.clone()
            }

            "prefer_const" => {
                if settings.immutable.mutated_tables_stay_local
                    && warning.category == "mutated_table"
                {
                    continue;
                }

                warning.diagnostic.message.clone()
            }

            "deprecated" => {
                let replacement = if warning.category == "method" {
                    settings
                        .deprecated
                        .ambiguous_methods
                        .then(|| {
                            settings
                                .deprecated
                                .paths
                                .iter()
                                .find_map(|(name, replacement)| {
                                    (name.rsplit('.').next() == Some(subject.as_str()))
                                        .then_some(replacement)
                                })
                        })
                        .flatten()
                } else {
                    settings.deprecated.paths.get(subject)
                };

                let Some(replacement) = replacement else {
                    continue;
                };

                format!("{subject} is deprecated; use {replacement}")
            }

            "restricted_globals" => {
                let Some(reason) = settings.globals.get(subject) else {
                    continue;
                };

                format!("global {subject} is restricted: {reason}")
            }

            "restricted_module_paths" => {
                let Some(reason) = settings.imports.get(subject) else {
                    continue;
                };

                format!("module path {subject} is restricted: {reason}")
            }

            _ => warning.diagnostic.message.clone(),
        };

        let diagnostic = &warning.diagnostic;

        let range = location(
            source,
            &lines,
            diagnostic.start_line,
            diagnostic.start_column,
        )?
            ..location(source, &lines, diagnostic.end_line, diagnostic.end_column)?;

        let original = source
            .get(range.clone())
            .ok_or_else(|| invalid(range.clone()))?;

        let edits = if warning.fixable && warning.rule == "self_assignment" {
            vec![Edit {
                range: range.clone(),
                expected: original.to_owned(),
                replacement: ";".to_owned(),
            }]
        } else {
            Vec::new()
        };

        findings.push(Finding {
            rule: warning.rule.clone(),
            severity: policy.severity,
            range,
            message,
            edits,
        });
    }

    let covered: Vec<_> = findings
        .iter()
        .map(|finding| {
            finding.rule == "deprecated"
                && findings.iter().any(|other| {
                    other.rule == "deprecated"
                        && other.range != finding.range
                        && other.range.start <= finding.range.start
                        && other.range.end >= finding.range.end
                })
        })
        .collect();

    Ok(findings
        .into_iter()
        .zip(covered)
        .filter_map(|(finding, covered)| (!covered).then_some(finding))
        .collect())
}

fn location(source: &str, lines: &[usize], line: u32, column: u32) -> Result<usize, Error> {
    let offset = lines
        .get(line as usize)
        .and_then(|start| start.checked_add(column as usize));

    let end = lines
        .get(line as usize + 1)
        .copied()
        .unwrap_or(source.len());

    offset
        .filter(|offset| *offset <= end && source.is_char_boundary(*offset))
        .ok_or_else(|| invalid(0..0))
}

fn invalid(range: Range<usize>) -> Error {
    Error {
        range,
        message: "native lint finding has an invalid source range".to_owned(),
    }
}
