use std::{
    collections::BTreeMap,
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
    process::ExitCode,
};

use clap::Args;
use instar_core::format::configuration::Configuration;

use crate::input::Input;

#[derive(Args)]
pub struct Format {
    #[command(flatten)]
    input: Input,

    /// Report files that need formatting without writing them.
    #[arg(long)]
    check: bool,

    /// Write formatted source to stdout instead of changing files.
    #[arg(long, conflicts_with = "check")]
    stdout: bool,

    /// Read formatter settings from this Instar configuration file.
    #[arg(long)]
    config: Option<PathBuf>,
}

fn configuration<'cache>(
    cache: &'cache mut BTreeMap<PathBuf, Configuration>,
    path: &Path,
    explicit: Option<&Path>,
) -> io::Result<&'cache Configuration> {
    let path = std::path::absolute(path)?;

    let directory = path
        .parent()
        .ok_or_else(|| io::Error::other("source has no parent"))?
        .to_owned();

    match cache.entry(directory) {
        std::collections::btree_map::Entry::Occupied(entry) => Ok(entry.into_mut()),

        std::collections::btree_map::Entry::Vacant(entry) => {
            Ok(entry.insert(Configuration::discover(&path, explicit)?))
        }
    }
}

impl Format {
    pub fn run(self) -> io::Result<ExitCode> {
        let mut configurations = BTreeMap::new();

        let input = self.input.load_selected(|path| {
            configuration(&mut configurations, path, self.config.as_deref())?
                .selection
                .includes(path)
        })?;

        if self.stdout && input.sources.len() != 1 {
            return Err(io::Error::other("--stdout requires exactly one source"));
        }

        let mut failed = false;

        for source in input.sources {
            let path = source.path();
            let standard_input = input.standard_input.as_deref() == Some(path);

            let result = (|| -> io::Result<bool> {
                let configuration =
                    configuration(&mut configurations, path, self.config.as_deref())?;

                let output = configuration.format(source.bytes())?;
                let changed = output != source.bytes();

                if !self.check && (self.stdout || standard_input) {
                    io::stdout().lock().write_all(&output)?;
                } else if changed && !self.check {
                    input.store.validate(&source).map_err(io::Error::other)?;

                    if fs::read(path)? != source.bytes() {
                        return Err(io::Error::other("source changed while formatting"));
                    }

                    fs::write(path, output)?;
                }

                Ok(changed)
            })();

            match result {
                Ok(true) if self.check => {
                    eprintln!("{}: would reformat", path.display());
                    failed = true;
                }

                Ok(_) => {}

                Err(error) => {
                    eprintln!("{}: {error}", path.display());
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
