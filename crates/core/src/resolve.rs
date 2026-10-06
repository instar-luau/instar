//! Filesystem require resolution using native configuration aliases.

use std::{
    fs, io,
    path::{Component, Path, PathBuf},
    time::Duration,
};

use crate::{configuration::invalid, project::Project};

/// An abstract module identity and its backing source file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Module {
    /// Absolute navigation path, without a source extension or trailing init filename.
    pub path: PathBuf,

    /// Absolute source file, preserving symlink spelling.
    pub source: PathBuf,
}

/// Resolves a require string from an absolute source filename, independently of cwd.
///
/// Native configuration execution uses the supplied per-file timeout. Resolution
/// performs filesystem navigation only; it does not execute or cache modules.
///
/// # Errors
/// Returns source-qualified errors for invalid callers, unsupported requests,
/// missing or ambiguous modules, invalid configuration, and alias cycles.
pub fn resolve(source: &Path, request: &str, timeout: Duration) -> io::Result<Module> {
    resolve_inner(source, request, timeout).map_err(|error| {
        io::Error::new(
            error.kind(),
            format!("{}: require {request:?}: {error}", source.display()),
        )
    })
}

fn resolve_inner(source: &Path, request: &str, timeout: Duration) -> io::Result<Module> {
    if !source.is_absolute() {
        return Err(invalid("requiring source must be absolute"));
    }

    if source
        .file_name()
        .is_some_and(|name| name == ".config.luau")
    {
        return Err(invalid(".config.luau is not an importable module"));
    }

    if !matches!(
        source.extension().and_then(|name| name.to_str()),
        Some("lua" | "luau")
    ) {
        return Err(invalid(
            "requiring source must have a .lua or .luau extension",
        ));
    }

    if request.contains('\0') {
        return Err(invalid("require path contains NUL"));
    }

    let request = request.replace('\\', "/");

    if !request.starts_with('@') && !request.starts_with("./") && !request.starts_with("../") {
        return Err(invalid("require path must start with ./, ../, or @"));
    }

    let source = normalize(source);
    let path = module_path(&source);

    if probe(&path)?.as_ref() != Some(&source) {
        return Err(invalid(format!(
            "source does not represent module {}",
            path.display()
        )));
    }

    let directory = source
        .parent()
        .ok_or_else(|| invalid("source has no parent"))?;

    let project = Project::load(directory, timeout)?;
    let snapshot = project.native.snapshot()?;

    let target = if let Some(aliased) = request.strip_prefix('@') {
        let (name, rest) = aliased.split_once('/').unwrap_or((aliased, ""));
        let start = alias(&snapshot, name, &path, timeout, &mut Vec::new())?;

        walk(start, rest)?
    } else {
        let parent = path
            .parent()
            .ok_or_else(|| invalid("module has no parent"))?;

        walk(parent.to_path_buf(), &request)?
    };

    let source = probe(&target)?.ok_or_else(|| {
        invalid(format!(
            "directory has no module source: {}",
            target.display()
        ))
    })?;

    Ok(Module {
        path: target,
        source,
    })
}

fn alias(
    snapshot: &instar_bridge::Snapshot,
    name: &str,
    module: &Path,
    timeout: Duration,
    stack: &mut Vec<(PathBuf, String)>,
) -> io::Result<PathBuf> {
    if name.is_empty()
        || !name
            .bytes()
            .all(|character| character.is_ascii_alphanumeric() || b".-_".contains(&character))
    {
        return Err(invalid(format!("invalid alias name @{name}")));
    }

    let name = name.to_ascii_lowercase();

    if name == "self" {
        return Ok(module.to_path_buf());
    }

    let definition = snapshot
        .aliases
        .get(&name)
        .ok_or_else(|| invalid(format!("unknown alias @{name}")))?;

    let key = (definition.directory.clone(), name.clone());

    if stack.contains(&key) {
        let mut cycle = stack
            .iter()
            .map(|(directory, name)| format!("{}:@{name}", directory.display()))
            .collect::<Vec<_>>();

        cycle.push(format!("{}:@{name}", definition.directory.display()));

        return Err(invalid(format!("alias cycle: {}", cycle.join(" -> "))));
    }

    stack.push(key);
    let value = definition.value.replace('\\', "/");

    if value.contains('\0') || value.is_empty() {
        return Err(invalid(format!(
            "alias @{name} target must be nonempty and contain no NUL"
        )));
    }

    let result = if let Some(aliased) = value.strip_prefix('@') {
        let (next, rest) = aliased.split_once('/').unwrap_or((aliased, ""));
        let project = Project::load(&definition.directory, timeout)?;
        let scope = project.native.snapshot()?;
        let start = alias(&scope, next, module, timeout, stack)?;

        walk(start, rest)
    } else if Path::new(&value).is_absolute() {
        let target = module_path(&normalize(Path::new(&value)));
        probe(&target)?;

        Ok(target)
    } else {
        walk(definition.directory.clone(), &value)
    };

    stack.pop();

    result
}

fn walk(mut path: PathBuf, request: &str) -> io::Result<PathBuf> {
    for component in request.split('/') {
        match component {
            "" | "." => {}

            ".." => {
                if !path.pop() {
                    return Err(invalid("module navigation escaped the filesystem root"));
                }
            }

            ".config" | ".config.luau" => {
                return Err(invalid(".config.luau is not an importable module"));
            }

            name => {
                if name.contains(':') {
                    return Err(invalid("invalid module path component"));
                }

                path.push(name);
                probe(&path)?;
            }
        }
    }

    Ok(path)
}

fn probe(path: &Path) -> io::Result<Option<PathBuf>> {
    if path.file_name().is_some_and(|name| name == ".config") {
        return Err(invalid(".config.luau is not an importable module"));
    }

    let directory = metadata(path)?.is_some_and(|metadata| metadata.is_dir());
    let mut sources = Vec::new();

    if path.file_name().is_some_and(|name| name != "init") {
        for suffix in [".luau", ".lua"] {
            let mut filename = path.as_os_str().to_os_string();
            filename.push(suffix);
            let candidate = PathBuf::from(filename);

            if metadata(&candidate)?.is_some_and(|metadata| metadata.is_file()) {
                sources.push(candidate);
            }
        }
    }

    let sibling = !sources.is_empty();

    if directory {
        if sibling {
            sources.push(path.to_path_buf());
        } else {
            for filename in ["init.luau", "init.lua"] {
                let candidate = path.join(filename);

                if metadata(&candidate)?.is_some_and(|metadata| metadata.is_file()) {
                    sources.push(candidate);
                }
            }
        }
    }

    if sources.len() > 1 {
        return Err(invalid(format!(
            "ambiguous module {}: {}",
            path.display(),
            sources
                .iter()
                .map(|source| source.display().to_string())
                .collect::<Vec<_>>()
                .join(", "),
        )));
    }

    if let Some(source) = sources.pop() {
        return Ok(Some(source));
    }

    if directory {
        return Ok(None);
    }

    Err(io::Error::new(
        io::ErrorKind::NotFound,
        format!("module path not found: {}", path.display()),
    ))
}

fn metadata(path: &Path) -> io::Result<Option<fs::Metadata>> {
    match fs::metadata(path) {
        Ok(metadata) => Ok(Some(metadata)),

        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
            ) =>
        {
            Ok(None)
        }

        Err(error) => Err(io::Error::new(
            error.kind(),
            format!("{}: {error}", path.display()),
        )),
    }
}

fn module_path(source: &Path) -> PathBuf {
    if matches!(
        source.file_name().and_then(|name| name.to_str()),
        Some("init.lua" | "init.luau")
    ) {
        source.parent().unwrap_or(source).to_path_buf()
    } else if matches!(
        source.extension().and_then(|name| name.to_str()),
        Some("lua" | "luau")
    ) {
        source.with_extension("")
    } else {
        source.to_path_buf()
    }
}

fn normalize(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();

    for component in path.components() {
        match component {
            Component::CurDir => {}

            Component::ParentDir => {
                normalized.pop();
            }

            component => normalized.push(component.as_os_str()),
        }
    }

    normalized
}
