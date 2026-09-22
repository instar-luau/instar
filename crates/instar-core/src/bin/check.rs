//! Checks entry files with Luau using Instar's on-demand resolver.

use instar_core::{
    analysis::{self, Location},
    project::Project,
};

use std::{
    env,
    io::{self, Write},
    path::{Path, PathBuf},
    process::ExitCode,
};

fn write_location(output: &mut impl Write, location: &Location, base: &Path) -> io::Result<()> {
    let source = &location.module.source;

    write!(
        output,
        "{}:{}:{}",
        source.strip_prefix(base).unwrap_or(source).display(),
        location.range[0] + 1,
        location.range[1] + 1
    )?;

    if let Some(instance) = &location.module.instance {
        let map = instance.sourcemap_path();

        write!(
            output,
            " [{}; place: {}]",
            instance.full_name(),
            map.strip_prefix(base).unwrap_or(map).display()
        )?;
    }

    Ok(())
}

fn run() -> io::Result<ExitCode> {
    let paths = env::args_os()
        .skip(1)
        .map(PathBuf::from)
        .collect::<Vec<_>>();

    if paths.len() == 1 && matches!(paths[0].to_str(), Some("--help" | "-h")) {
        println!(
            "Usage: check <entry.lua[u]>...\nRuns Luau analysis with Instar's resolver, configuration, and sourcemap discovery."
        );

        return Ok(ExitCode::SUCCESS);
    }

    if paths.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "at least one entry file is required",
        ));
    }

    let diagnostics = analysis::check(&mut Project::new(), &paths)?;
    let base = env::current_dir()?;
    let mut output = io::BufWriter::new(io::stdout().lock());

    for diagnostic in &diagnostics {
        write_location(&mut output, &diagnostic.location, &base)?;

        writeln!(
            output,
            ": {}: {}",
            if diagnostic.error { "error" } else { "warning" },
            diagnostic.message
        )?;

        if let Some((location, message)) = &diagnostic.related {
            write_location(&mut output, location, &base)?;
            writeln!(output, ": note: {message}")?;
        }
    }

    if diagnostics.is_empty() {
        writeln!(output, "No diagnostics.")?;
    }

    output.flush()?;

    Ok(if diagnostics.iter().any(|diagnostic| diagnostic.error) {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    })
}

fn main() -> ExitCode {
    match run() {
        Ok(code) => code,

        Err(error) => {
            eprintln!("{error}");

            ExitCode::from(2)
        }
    }
}
