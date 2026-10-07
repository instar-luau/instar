use std::{
    collections::BTreeSet,
    fs,
    io::{self, Read},
    path::{self, Path, PathBuf},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

use eyre::{Result, WrapErr, ensure};
use instar_analysis::Options;
use instar_core::project::{Change, Project};

pub(crate) fn load(
    files: &[PathBuf],
    context: Option<&Path>,
    project: &mut Project,
    options: &Options,
    started: Instant,
) -> Result<(Vec<PathBuf>, bool)> {
    if files.iter().any(|path| path == Path::new("-")) {
        ensure!(
            files.len() == 1,
            "stdin cannot be combined with file inputs"
        );

        let path = path::absolute(context.unwrap_or_else(|| Path::new("stdin.luau")))?;
        let text = stdin(options, started)?;
        options.check(started)?;

        project.change(Change::Overlay {
            path: path.clone(),
            text: Some(text),
        })?;

        return Ok((vec![path], true));
    }

    ensure!(
        context.is_none(),
        "--stdin-filepath requires '-' as the input"
    );

    let defaults = [PathBuf::from(".")];
    let mut selected = BTreeSet::new();

    for input in if files.is_empty() {
        &defaults[..]
    } else {
        files
    } {
        options.check(started)?;
        let path = path::absolute(input)?;

        match fs::metadata(&path) {
            Ok(metadata) if metadata.is_file() => {
                selected.insert(path);
            }

            Ok(metadata) if metadata.is_dir() => {
                walk(&path, None, &mut selected, options, started)?;
            }

            Ok(_) => eyre::bail!(
                "input is not a regular file or directory: {}",
                path.display()
            ),

            Err(error)
                if error.kind() == io::ErrorKind::NotFound
                    || path.to_string_lossy().contains(['*', '?', '[']) =>
            {
                let text = path
                    .to_str()
                    .ok_or_else(|| eyre::eyre!("glob paths must be UTF-8"))?;

                ensure!(
                    text.contains(['*', '?', '[']),
                    "input does not exist: {}",
                    path.display()
                );

                let pattern = glob::Pattern::new(text)
                    .wrap_err_with(|| format!("invalid glob: {}", input.display()))?;

                let mut base = PathBuf::new();

                for component in path.components() {
                    if component
                        .as_os_str()
                        .to_string_lossy()
                        .contains(['*', '?', '['])
                    {
                        break;
                    }

                    base.push(component);
                }

                let mut matches = BTreeSet::new();
                walk(&base, Some(&pattern), &mut matches, options, started)?;

                ensure!(
                    !matches.is_empty(),
                    "glob matched no source files: {}",
                    input.display()
                );

                selected.extend(matches);
            }

            Err(error) => {
                return Err(error).wrap_err_with(|| format!("reading {}", path.display()));
            }
        }
    }

    Ok((selected.into_iter().collect(), false))
}

fn stdin(options: &Options, started: Instant) -> Result<String> {
    options.check(started)?;
    let (sender, receiver) = mpsc::sync_channel(1);

    thread::Builder::new()
        .name("stdin".to_owned())
        .spawn(move || {
            let mut input = io::stdin().lock();
            let mut buffer = [0; 8192];

            loop {
                let result = match input.read(&mut buffer) {
                    Ok(count) => Ok(buffer[..count].to_vec()),
                    Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                    Err(error) => Err(error),
                };

                let finished = result.as_ref().map_or(true, Vec::is_empty);

                if sender.send(result).is_err() || finished {
                    break;
                }
            }
        })?;

    let mut bytes = Vec::new();

    loop {
        options.check(started)?;
        let remaining = options.timeout.saturating_sub(started.elapsed());

        match receiver.recv_timeout(remaining.min(Duration::from_millis(10))) {
            Ok(result) => {
                options.check(started)?;
                let chunk = result?;

                if chunk.is_empty() {
                    return String::from_utf8(bytes)
                        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error).into());
                }

                bytes.extend(chunk);
            }

            Err(mpsc::RecvTimeoutError::Timeout) => {}

            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return Err(io::Error::other("stdin reader stopped").into());
            }
        }
    }
}

fn walk(
    root: &Path,
    pattern: Option<&glob::Pattern>,
    selected: &mut BTreeSet<PathBuf>,
    options: &Options,
    started: Instant,
) -> Result<()> {
    for entry in ignore::WalkBuilder::new(root).require_git(false).build() {
        options.check(started)?;
        let entry = entry?;

        if !entry.file_type().is_some_and(|kind| kind.is_file()) {
            continue;
        }

        let path = entry.path();

        if matches!(
            path.extension().and_then(|extension| extension.to_str()),
            Some("lua" | "luau")
        ) && pattern.is_none_or(|pattern| {
            pattern.matches_path_with(
                path,
                glob::MatchOptions {
                    require_literal_separator: true,
                    ..glob::MatchOptions::new()
                },
            )
        }) {
            selected.insert(path.to_owned());
        }
    }

    Ok(())
}
