use crate::input::Input;
use clap::Args;
use instar_core::{
    analysis,
    lint::{self, configuration::Level},
    project::selection::Selection,
    source::{PositionEncoding, Source, SourceStore},
};
use std::{
    io::{self, Write},
    path::PathBuf,
    process::ExitCode,
};

#[derive(Args)]
pub struct Lint {
    #[arg(required_unless_present_any = ["list", "explain"])]
    files: Vec<PathBuf>,

    #[arg(long)]
    filename: Option<PathBuf>,

    #[arg(long)]
    fix: bool,

    #[arg(long, conflicts_with = "explain")]
    list: bool,

    #[arg(long)]
    explain: Option<String>,
}

fn diagnostics(report: &lint::Report, source: &Source) -> io::Result<bool> {
    for diagnostic in &report.diagnostics {
        eprintln!(
            "{}({},{}): {}",
            diagnostic.path.display(),
            diagnostic.line + 1,
            diagnostic.column + 1,
            diagnostic.message
        );
    }

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
            "{}({},{}): {:?}: {}: {}",
            source.path().display(),
            position.line + 1,
            position.col + 1,
            finding.level,
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
    pub fn run(self) -> io::Result<ExitCode> {
        if self.list {
            for rule in lint::registry::RULES {
                println!(
                    "{} [{}] {:?}: {}",
                    rule.name, rule.group, rule.level, rule.description
                );
            }

            return Ok(ExitCode::SUCCESS);
        }

        if let Some(name) = self.explain {
            let rule = lint::registry::find(&name)
                .ok_or_else(|| io::Error::other(format!("unknown lint rule: {name}")))?;

            println!(
                "{}\nGroup: {}\nDefault: {:?}\n\n{}",
                rule.name, rule.group, rule.level, rule.description
            );

            return Ok(ExitCode::SUCCESS);
        }

        let mut input = Input::new(self.files, self.filename)
            .load_selected(|path| Selection::discover(path)?.includes(path))?;

        let mut session = analysis::Session::default();
        let mut failed = false;

        for source in &input.sources {
            let result = (|| -> io::Result<bool> {
                let mut report = lint::analyze(&mut session, &mut input.store, source.path())?;
                let mut measured = source.clone();

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
