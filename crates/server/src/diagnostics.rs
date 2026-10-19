use std::{
    collections::{BTreeMap, BTreeSet, HashSet},
    path::Path,
    sync::Arc,
};

use instar_bridge::{Frontend, Position as NativePosition, Range as NativeRange};

use instar_core::{
    lint,
    settings::lint::Severity,
    workspace::{Access, InvalidSource, Origin, Purpose, SymbolOutcome},
};

use tower_lsp_server::{
    gen_lsp_types::{
        Code, Diagnostic, DiagnosticRelatedInformation, DiagnosticSeverity, Location, Message,
        PublishDiagnosticsParams, Range,
    },
    jsonrpc::Result,
};

use super::{
    configuration::Scope,
    document::{self, Document, Encoding},
    project::{Project, failure},
};

pub(super) fn collect(
    project: &mut Project,
    frontend: &mut Frontend<'_>,
) -> Result<Vec<PublishDiagnosticsParams>> {
    let open: BTreeSet<_> = project
        .documents
        .iter()
        .filter(|(module, document)| document.version.is_some() && project.owns(module))
        .map(|(module, _)| module.clone())
        .collect();

    let workspace = project.indexed
        && project.configuration.diagnostic_scope == Scope::Workspace
        && project
            .workspace
            .directory()
            .try_exists()
            .map_err(failure)?;

    let checked: BTreeSet<_> = if workspace {
        project
            .workspace
            .select(&[], Purpose::Check, Access::Read)
            .map_err(failure)?
            .into_keys()
            .chain(open.iter().cloned())
            .filter(|module| project.owns(module))
            .collect()
    } else {
        open.clone()
    };

    let linted: BTreeSet<_> = if workspace {
        project
            .workspace
            .select(&[], Purpose::Lint, Access::Read)
            .map_err(failure)?
            .into_keys()
            .chain(open.iter().cloned())
            .filter(|module| project.owns(module))
            .collect()
    } else {
        open.clone()
    };

    let mut findings: BTreeMap<String, Vec<Diagnostic>> = checked
        .union(&linted)
        .map(|module| (module.clone(), Vec::new()))
        .collect();

    let mut versions = BTreeMap::new();

    let environments: BTreeMap<_, _> = checked
        .union(&linted)
        .map(|module| {
            project
                .workspace
                .symbols(module)
                .map(|symbols| (module.clone(), symbols))
                .map_err(failure)
        })
        .collect::<Result<_>>()?;

    for (module, symbols) in &environments {
        if let SymbolOutcome::Invalid(invalid) = symbols
            && ((checked.contains(module)
                && !project
                    .workspace
                    .excluded(module, Purpose::Check)
                    .map_err(failure)?)
                || (linted.contains(module)
                    && !project
                        .workspace
                        .excluded(module, Purpose::Lint)
                        .map_err(failure)?))
        {
            environment(project, module, invalid, &mut findings, &mut versions)?;
        }
    }

    for module in &checked {
        frontend.queue(module).map_err(failure)?;
    }

    for module in &checked {
        for diagnostic in frontend.check(module).map_err(failure)? {
            let physical =
                !diagnostic.module.is_empty() && Path::new(&diagnostic.module).is_absolute();

            let target = if physical {
                diagnostic.module.as_str()
            } else {
                module
            };

            let invalid = match &environments[module] {
                SymbolOutcome::Invalid(invalid) if matches!(&invalid.origin, Origin::File(path) if path == Path::new(target)) => {
                    Some(invalid)
                }

                _ => None,
            };

            let conflicting = invalid.is_some_and(|invalid| {
                project
                    .documents
                    .get(target)
                    .or_else(|| project.borrowed.get(target))
                    .is_some_and(|document| {
                        document.version.is_some() && document.text != invalid.text
                    })
            });

            let destination = if conflicting { module.as_str() } else { target };

            if !project.owns(destination)
                || (!workspace && !open.contains(destination))
                || project
                    .workspace
                    .excluded(destination, Purpose::Check)
                    .map_err(failure)?
            {
                continue;
            }

            let source = if let Some(invalid) = invalid {
                Document::new(
                    document::uri(Path::new(target))?,
                    Arc::clone(&invalid.text),
                    None,
                    0,
                )
            } else {
                project.snapshot(target)?
            };

            let range = if physical {
                source.native_range(
                    NativeRange {
                        start: NativePosition {
                            line: diagnostic.start_line,
                            column: diagnostic.start_column,
                        },
                        end: NativePosition {
                            line: diagnostic.end_line,
                            column: diagnostic.end_column,
                        },
                    },
                    project.encoding,
                )?
            } else {
                Range::default()
            };

            let (range, message) = if conflicting {
                (
                    Range::default(),
                    format!(
                        "invalid environment at {target}:{}:{}: {}",
                        range.start.line + 1,
                        range.start.character + 1,
                        diagnostic.message,
                    ),
                )
            } else {
                (range, diagnostic.message)
            };

            findings
                .entry(destination.to_owned())
                .or_default()
                .push(Diagnostic {
                    range,
                    severity: Some(DiagnosticSeverity::Error),
                    code: Some(Code::Int(diagnostic.code)),
                    source: Some("luau".to_owned()),
                    message: Message::String(message),
                    ..Diagnostic::default()
                });
        }
    }

    for module in &linted {
        if project
            .workspace
            .excluded(module, Purpose::Lint)
            .map_err(failure)?
        {
            continue;
        }

        let source = project.snapshot(module)?;
        let settings = project.workspace.linting(module).map_err(failure)?;

        let symbols = match &environments[module] {
            SymbolOutcome::Ready(symbols) => symbols,
            SymbolOutcome::Invalid(_) => continue,
        };

        let result = lint::lint(&source.text, &settings, symbols);

        let diagnostics = findings.entry(module.clone()).or_default();

        match result {
            Ok(findings) => {
                for finding in findings {
                    let severity = match finding.severity {
                        Severity::Error => DiagnosticSeverity::Error,
                        Severity::Warning => DiagnosticSeverity::Warning,
                        Severity::Information => DiagnosticSeverity::Information,
                        Severity::Hint => DiagnosticSeverity::Hint,
                    };

                    diagnostics.push(Diagnostic {
                        range: source.range(finding.range, project.encoding)?,
                        severity: Some(severity),
                        code: Some(Code::String(finding.rule)),
                        source: Some("instar".to_owned()),
                        message: Message::String(finding.message),
                        ..Diagnostic::default()
                    });
                }
            }

            Err(failure) => {
                let (code, error) = match failure {
                    lint::Failure::Syntax(error) => ("syntax", error),
                    lint::Failure::Directive(error) => ("lint", error),
                };

                diagnostics.push(Diagnostic {
                    range: source.range(error.range, project.encoding)?,
                    severity: Some(DiagnosticSeverity::Error),
                    code: Some(Code::String(code.to_owned())),
                    source: Some("instar".to_owned()),
                    message: Message::String(error.message),
                    ..Diagnostic::default()
                });
            }
        }
    }

    for module in &project.published {
        findings.entry(module.clone()).or_default();
    }

    let mut publications = Vec::new();
    let mut published = BTreeSet::new();

    for (module, mut diagnostics) in findings {
        let mut unique = HashSet::new();
        diagnostics.retain(|diagnostic| unique.insert(diagnostic.clone()));
        let uri = document::uri(Path::new(&module))?;

        let version = versions.get(&module).copied().unwrap_or_else(|| {
            project
                .documents
                .get(&module)
                .and_then(|source| source.version)
        });

        if !diagnostics.is_empty() {
            published.insert(module);
        }

        publications.push(PublishDiagnosticsParams {
            uri,
            version,
            diagnostics,
        });
    }

    project.published = published;

    Ok(publications)
}

fn environment(
    project: &Project,
    module: &str,
    invalid: &InvalidSource,
    findings: &mut BTreeMap<String, Vec<Diagnostic>>,
    versions: &mut BTreeMap<String, Option<i32>>,
) -> Result<()> {
    let (origin, target) = match &invalid.origin {
        Origin::File(path) => (path.display().to_string(), Some(path)),
        Origin::Builtins => ("<builtins>".to_owned(), None),
    };

    let uri = match target {
        Some(path) => document::uri(path)?,
        None => document::uri(Path::new(module))?,
    };

    let snapshot = Document::new(uri.clone(), Arc::clone(&invalid.text), None, 0);
    let range = snapshot.range(invalid.error.range.clone(), project.encoding)?;
    let position = snapshot.position(invalid.error.range.start, Encoding::Scalars)?;

    let captured = target
        .and_then(|path| path.to_str())
        .and_then(|module| {
            project
                .documents
                .get(module)
                .or_else(|| project.borrowed.get(module))
        })
        .filter(|document| document.version.is_some());

    let matching = captured.is_none_or(|document| document.text == invalid.text);

    if let Some(target) = target.and_then(|path| path.to_str())
        && project.owns(target)
        && matching
    {
        versions.insert(
            target.to_owned(),
            captured.and_then(|document| document.version),
        );

        findings
            .entry(target.to_owned())
            .or_default()
            .push(Diagnostic {
                range,
                severity: Some(DiagnosticSeverity::Error),
                code: Some(Code::String("syntax".to_owned())),
                source: Some("instar".to_owned()),
                message: Message::String(invalid.error.message.clone()),
                ..Diagnostic::default()
            });

        return Ok(());
    }

    let related_information = if target.is_some() && matching {
        Some(vec![DiagnosticRelatedInformation {
            location: Location { uri, range },
            message: invalid.error.message.clone(),
        }])
    } else {
        None
    };

    findings
        .entry(module.to_owned())
        .or_default()
        .push(Diagnostic {
            range: Range::default(),
            severity: Some(DiagnosticSeverity::Error),
            code: Some(Code::String("syntax".to_owned())),
            source: Some("instar".to_owned()),
            message: Message::String(format!(
                "invalid lint environment at {origin}:{}:{}: {}",
                position.line + 1,
                position.character + 1,
                invalid.error.message,
            )),
            related_information,
            ..Diagnostic::default()
        });

    Ok(())
}
