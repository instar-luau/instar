use crate::input::Input;
use clap::Args;

use instar_core::{
    analysis,
    lint::{self, configuration::Level},
    project::{
        Configuration,
        selection::{Scope, Selection},
    },
    source::{PositionEncoding, Source, SourceStore},
};

use std::{
    collections::{BTreeMap, btree_map::Entry},
    io::{self, Write},
    path::PathBuf,
    process::ExitCode,
    sync::Arc,
};

#[derive(Args)]
pub(super) struct Lint {
    /// Files or directories to lint; '-' reads original bytes from stdin.
    #[arg(required_unless_present_any = ["list", "explain"])]
    files: Vec<PathBuf>,

    /// Module filename for stdin, used for imports and configuration.
    #[arg(long)]
    filename: Option<PathBuf>,

    /// Apply safe fixes and verify their resulting source.
    #[arg(long)]
    fix: bool,

    /// List built-in lint rules and their default levels.
    #[arg(long, conflicts_with = "explain")]
    list: bool,

    /// Show the description and default level of a built-in rule.
    #[arg(long)]
    explain: Option<String>,
}

fn level(level: Level) -> &'static str {
    match level {
        Level::Allow => "allow",
        Level::Info => "info",
        Level::Warn => "warn",
        Level::Deny => "deny",
    }
}

fn diagnostics(report: &lint::Report, source: &Source) -> io::Result<bool> {
    crate::analyze::diagnostics(&report.diagnostics);

    for finding in &report.findings {
        let position = source
            .position(
                u32::try_from(finding.start)
                    .map_err(io::Error::other)?
                    .into(),
                PositionEncoding::Utf8,
            )
            .map_err(io::Error::other)?;

        eprintln!(
            "{}({},{}): {}: {}: {}",
            source.path().display(),
            position.line + 1,
            position.col + 1,
            level(finding.level),
            finding.rule,
            finding.message
        );
    }

    Ok(!report.diagnostics.is_empty()
        || report
            .findings
            .iter()
            .any(|finding| finding.level == Level::Deny))
}

impl Lint {
    pub(super) fn run(self) -> io::Result<ExitCode> {
        if self.list {
            for rule in lint::registry::RULES {
                println!(
                    "{} [{}] {}: {}",
                    rule.name,
                    rule.group,
                    level(rule.level),
                    rule.description
                );
            }

            return Ok(ExitCode::SUCCESS);
        }

        if let Some(name) = self.explain {
            let rule = lint::registry::find(&name)
                .ok_or_else(|| io::Error::other(format!("unknown lint rule: {name}")))?;

            println!(
                "{}\nGroup: {}\nDefault: {}\n\n{}",
                rule.name,
                rule.group,
                level(rule.level),
                rule.description
            );

            return Ok(ExitCode::SUCCESS);
        }

        let mut configurations = BTreeMap::new();

        let mut input = Input::new(self.files, self.filename).load_selected(|path| {
            let path = std::path::absolute(path)?;

            let configuration_path = instar_core::project::nearest_configuration(&path)
                .or_else(|| path.parent().map(std::path::Path::to_owned))
                .ok_or_else(|| io::Error::other("source has no parent"))?;

            let (selection, configuration) = match configurations.entry(configuration_path) {
                Entry::Occupied(entry) => entry.into_mut(),

                Entry::Vacant(entry) => entry.insert((
                    Selection::discover(&path, Scope::Lint)?,
                    Configuration::discover_frontends(&path)?,
                )),
            };

            Ok(configuration.language(&path) && selection.includes(&path)?)
        })?;

        let mut session = analysis::Session::default();
        let mut failed = false;

        for source in &input.sources {
            let result = (|| -> io::Result<bool> {
                let mut report = lint::analyze(&mut session, &mut input.store, source.path())?;
                let mut measured = Arc::clone(source);

                if self.fix {
                    let edits = lint::edits(&report.findings)?;

                    if !edits.is_empty() {
                        let output = lint::apply(source, &edits)?;
                        let mut verification = SourceStore::default();

                        for original in &input.sources {
                            verification
                                .open_bytes(
                                    original.path(),
                                    0,
                                    if original.path() == source.path() {
                                        output.clone()
                                    } else {
                                        original.bytes().to_vec()
                                    },
                                )
                                .map_err(io::Error::other)?;
                        }

                        report = lint::analyze(
                            &mut analysis::Session::default(),
                            &mut verification,
                            source.path(),
                        )?;

                        if !report.diagnostics.is_empty() {
                            return Err(io::Error::other("lint fixes introduced a syntax error"));
                        }

                        if !lint::edits(&report.findings)?.is_empty() {
                            return Err(io::Error::other("lint fixes are not idempotent"));
                        }

                        measured = verification.read(source.path()).map_err(io::Error::other)?;

                        if input.standard_input.as_deref() != Some(source.path()) {
                            lint::write(source, &input.store, &output)?;
                        }
                    }

                    if input.standard_input.as_deref() == Some(source.path()) {
                        io::stdout().lock().write_all(measured.bytes())?;
                    }
                }

                diagnostics(&report, &measured)
            })();

            match result {
                Ok(errors) => failed |= errors,

                Err(error) => {
                    eprintln!("{}: {error}", source.path().display());
                    failed = true;
                }
            }
        }

        Ok(if failed {
            ExitCode::FAILURE
        } else {
            ExitCode::SUCCESS
        })
    }
}
