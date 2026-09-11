use std::{
    collections::{BTreeMap, btree_map::Entry},
    io::{self, Write},
    process::ExitCode,
};

use clap::Args;
use instar_core::{analysis, project::selection::Selection};

use crate::input::Input;

#[derive(Args)]
pub struct Analyze {
    #[command(flatten)]
    input: Input,

    /// Override the configured checking mode for files without a mode directive.
    #[arg(long, value_parser = ["strict", "nonstrict", "nocheck"])]
    mode: Option<String>,

    /// Select the upstream type solver.
    #[arg(long, value_parser = ["new", "old"])]
    solver: Option<String>,

    /// Emit source with inferred type annotations.
    #[arg(long)]
    annotate: bool,

    #[arg(long, help = "Refresh downloaded Roblox assets before analysis")]
    update: bool,
}

impl Analyze {
    pub fn run(self) -> io::Result<ExitCode> {
        let mut selections = BTreeMap::new();

        let mut input = self.input.load_selected(|path| {
            let path = std::path::absolute(path)?;

            let directory = path
                .parent()
                .ok_or_else(|| io::Error::other("source has no parent"))?;

            let selection = match selections.entry(directory.to_owned()) {
                Entry::Occupied(entry) => entry.into_mut(),
                Entry::Vacant(entry) => entry.insert(Selection::discover(&path)?),
            };

            selection.includes(&path)
        })?;

        let modules = input
            .sources
            .iter()
            .map(|source| source.path().to_owned())
            .collect::<Vec<_>>();

        let options = analysis::Options {
            mode: self.mode.as_deref().map(|mode| match mode {
                "strict" => analysis::Mode::Strict,
                "nonstrict" => analysis::Mode::Nonstrict,
                "nocheck" => analysis::Mode::Nocheck,
                _ => unreachable!("validated checking mode"),
            }),
            old_solver: self.solver.as_deref() == Some("old"),
            annotations: self.annotate,
            update: self.update,
        };

        let report = analysis::Session::default().analyze(&mut input.store, &modules, &options)?;

        for diagnostic in &report.diagnostics {
            eprintln!(
                "{}({},{}): {}",
                diagnostic.path.display(),
                diagnostic.line + 1,
                diagnostic.column + 1,
                diagnostic.message
            );
        }

        let mut output = io::stdout().lock();

        for annotation in &report.annotations {
            output.write_all(&annotation.bytes)?;
        }

        Ok(if report.has_errors() {
            ExitCode::FAILURE
        } else {
            ExitCode::SUCCESS
        })
    }
}
