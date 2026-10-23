use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
    sync::Arc,
};

use instar_bridge::{Frontend, Position as NativePosition, Range as NativeRange};

use instar_core::{
    lint,
    workspace::{Access, InvalidSource, Purpose, SymbolOutcome},
};

use tower_lsp_server::{
    gen_lsp_types::{Diagnostic, PublishDiagnosticsParams, Range},
    jsonrpc::{Error, Result},
};

use crate::{
    configuration::Scope,
    document::{self, Document, Encoding},
    error::failure,
    output::diagnostics,
    project::Project,
};

pub(crate) struct Batch {
    checked: BTreeSet<String>,
    open: BTreeSet<String>,
    environments: BTreeMap<String, SymbolOutcome>,
    sources: BTreeMap<String, Arc<str>>,
    findings: BTreeMap<String, Vec<Diagnostic>>,
    syntax: BTreeMap<String, Vec<Diagnostic>>,
    versions: BTreeMap<String, Option<i32>>,
    should_check_workspace: bool,
}

pub(crate) fn prepare(project: &mut Project) -> Result<(Batch, Vec<PublishDiagnosticsParams>)> {
    let open: BTreeSet<_> = project
        .documents
        .iter()
        .filter(|(module, document)| document.version.is_some() && project.is_owner(module))
        .map(|(module, _)| module.clone())
        .collect();

    let should_check_workspace = project.should_index
        && project.configuration.diagnostic_scope == Scope::Workspace
        && project
            .workspace
            .directory()
            .try_exists()
            .map_err(failure)?;

    let checked: BTreeSet<_> = if should_check_workspace {
        project
            .workspace
            .select(&[], Purpose::Check, Access::Read)
            .map_err(failure)?
            .into_keys()
            .chain(open.iter().cloned())
            .filter(|module| project.is_owner(module))
            .collect()
    } else {
        open.clone()
    };

    let linted: BTreeSet<_> = if should_check_workspace {
        project
            .workspace
            .select(&[], Purpose::Lint, Access::Read)
            .map_err(failure)?
            .into_keys()
            .chain(open.iter().cloned())
            .filter(|module| project.is_owner(module))
            .collect()
    } else {
        open.clone()
    };

    let mut findings: BTreeMap<String, Vec<Diagnostic>> = checked
        .union(&linted)
        .map(|module| (module.clone(), Vec::new()))
        .collect();

    let mut syntax: BTreeMap<String, Vec<Diagnostic>> = BTreeMap::new();

    let mut versions = BTreeMap::new();
    let mut sources = BTreeMap::new();

    let mut environments = BTreeMap::new();

    for module in checked.union(&linted) {
        let symbols = project.workspace.symbols(module).map_err(failure)?;

        environments.insert(module.clone(), symbols);
    }

    for (module, symbols) in &environments {
        if let SymbolOutcome::Invalid(invalid) = symbols
            && ((checked.contains(module)
                && !project
                    .workspace
                    .is_excluded(module, Purpose::Check)
                    .map_err(failure)?)
                || (linted.contains(module)
                    && !project
                        .workspace
                        .is_excluded(module, Purpose::Lint)
                        .map_err(failure)?))
        {
            environment(project, module, invalid, &mut findings, &mut versions)?;
        }
    }

    for module in &linted {
        if project
            .workspace
            .is_excluded(module, Purpose::Lint)
            .map_err(failure)?
        {
            continue;
        }

        let source = project.snapshot(module)?;
        sources.insert(module.clone(), Arc::clone(&source.text));
        let settings = project.workspace.linting(module).map_err(failure)?;

        let symbols = match &environments[module] {
            SymbolOutcome::Ready(symbols) => symbols,
            SymbolOutcome::Invalid(_) => continue,
        };

        let result = source.findings(settings, Arc::clone(symbols));

        let diagnostics = syntax.entry(module.clone()).or_default();

        match result {
            Ok(findings) => {
                for finding in findings.iter() {
                    diagnostics.push(diagnostics::finding(
                        source.range(finding.range.clone(), project.encoding)?,
                        finding.rule.clone(),
                        finding.severity,
                        finding.message.clone(),
                    ));
                }
            }

            Err(failure) => {
                let (code, error) = match failure {
                    lint::Failure::Syntax(error) => ("syntax", error),
                    lint::Failure::Directive(error) => ("lint", error),
                };

                diagnostics.push(diagnostics::error(
                    source.range(error.range, project.encoding)?,
                    code,
                    error.message,
                ));
            }
        }
    }

    let publications = publications(project, findings.clone(), syntax.clone(), &versions)?;

    project.published.extend(
        findings
            .iter()
            .chain(&syntax)
            .filter(|(_, diagnostics)| !diagnostics.is_empty())
            .map(|(module, _)| module.clone()),
    );

    Ok((
        Batch {
            checked,
            open,
            environments,
            sources,
            findings,
            syntax,
            versions,
            should_check_workspace,
        },
        publications,
    ))
}

pub(crate) fn check(
    project: &mut Project,
    frontend: &mut Frontend<'_>,
    batch: Batch,
) -> Result<Vec<PublishDiagnosticsParams>> {
    let Batch {
        checked,
        open,
        environments,
        sources,
        mut findings,
        syntax,
        versions,
        should_check_workspace,
    } = batch;

    for (module, source) in sources {
        if project.snapshot(&module)?.text != source {
            return Err(Error::content_modified());
        }
    }

    for module in &checked {
        frontend.queue(module).map_err(failure)?;
    }

    for module in &checked {
        for diagnostic in frontend.check(module).map_err(failure)? {
            let is_physical =
                !diagnostic.module.is_empty() && Path::new(&diagnostic.module).is_absolute();

            let target = if is_physical {
                diagnostic.module.as_str()
            } else {
                module
            };

            let invalid = match &environments[module] {
                SymbolOutcome::Invalid(invalid) if invalid.path == Path::new(target) => {
                    Some(invalid)
                }

                _ => None,
            };

            let is_conflicting = invalid.is_some_and(|invalid| {
                project
                    .documents
                    .get(target)
                    .or_else(|| project.borrowed.get(target))
                    .is_some_and(|document| {
                        document.version.is_some() && document.text != invalid.text
                    })
            });

            let destination = if is_conflicting {
                module.as_str()
            } else {
                target
            };

            if !project.is_owner(destination)
                || (!should_check_workspace && !open.contains(destination))
                || project
                    .workspace
                    .is_excluded(destination, Purpose::Check)
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
                )?
            } else {
                project.snapshot(target)?
            };

            let range = if is_physical {
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

            let (range, message) = if is_conflicting {
                (
                    Range::default(),
                    diagnostics::conflict(target, range.start, &diagnostic.message),
                )
            } else {
                (range, diagnostic.message)
            };

            findings
                .entry(destination.to_owned())
                .or_default()
                .push(diagnostics::native(range, diagnostic.code, message));
        }
    }

    let published = findings
        .iter()
        .chain(&syntax)
        .filter(|(_, diagnostics)| !diagnostics.is_empty())
        .map(|(module, _)| module.clone())
        .collect();

    let publications = publications(project, findings, syntax, &versions)?;
    project.published = published;

    Ok(publications)
}

fn publications(
    project: &Project,
    mut findings: BTreeMap<String, Vec<Diagnostic>>,
    syntax: BTreeMap<String, Vec<Diagnostic>>,
    versions: &BTreeMap<String, Option<i32>>,
) -> Result<Vec<PublishDiagnosticsParams>> {
    for (module, diagnostics) in syntax {
        findings.entry(module).or_default().extend(diagnostics);
    }

    for module in &project.published {
        findings.entry(module.clone()).or_default();
    }

    let mut publications = Vec::new();

    for (module, diagnostics) in findings {
        let uri = document::uri(Path::new(&module))?;

        let version = versions.get(&module).copied().unwrap_or_else(|| {
            project
                .documents
                .get(&module)
                .and_then(|source| source.version)
        });

        publications.push(diagnostics::publication(uri, version, diagnostics));
    }

    Ok(publications)
}

fn environment(
    project: &Project,
    module: &str,
    invalid: &InvalidSource,
    findings: &mut BTreeMap<String, Vec<Diagnostic>>,
    versions: &mut BTreeMap<String, Option<i32>>,
) -> Result<()> {
    let target = &invalid.path;
    let uri = document::uri(target)?;

    let snapshot = Document::new(uri.clone(), Arc::clone(&invalid.text), None, 0)?;
    let range = snapshot.range(invalid.error.range.clone(), project.encoding)?;
    let position = snapshot.position(invalid.error.range.start, Encoding::Scalars)?;

    let captured = target
        .to_str()
        .and_then(|module| {
            project
                .documents
                .get(module)
                .or_else(|| project.borrowed.get(module))
        })
        .filter(|document| document.version.is_some());

    let is_matching = captured.is_none_or(|document| document.text == invalid.text);

    if let Some(target) = target.to_str()
        && project.is_owner(target)
        && is_matching
    {
        versions.insert(
            target.to_owned(),
            captured.and_then(|document| document.version),
        );

        findings
            .entry(target.to_owned())
            .or_default()
            .push(diagnostics::error(
                range,
                "syntax",
                invalid.error.message.clone(),
            ));

        return Ok(());
    }

    let related = is_matching.then_some((uri, range));

    findings
        .entry(module.to_owned())
        .or_default()
        .push(diagnostics::environment(
            &invalid.path,
            position,
            &invalid.error.message,
            related,
        ));

    Ok(())
}
