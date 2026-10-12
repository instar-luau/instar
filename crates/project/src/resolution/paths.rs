use std::{
    fmt::Display,
    io,
    path::{Component, Path, PathBuf},
    sync::Arc,
};

use instar_luau::{ImportRequest, ImportSite, InstanceRoot, InstanceStep};
use instar_shared::Options;

use crate::{
    capture::{Capture, Listing},
    execution::Runtime,
    graph::ModuleNode,
    inputs::SourceOrigin,
    sourcemap::Map,
};

pub(crate) struct ResolvedTarget {
    pub(crate) path: PathBuf,
    pub(crate) map: Option<(Arc<Map>, usize)>,
}

impl Runtime {
    pub(crate) fn resolve(
        importer: &ModuleNode,
        site: &ImportSite,
        capture: &mut Capture,
        options: &Options,
    ) -> io::Result<Option<ResolvedTarget>> {
        capture.consult_source(&importer.source);

        for input in &importer.effective.inputs {
            capture.consult_input(input);
        }

        match &site.request {
            ImportRequest::Nonconstant => Ok(None),

            ImportRequest::Static(request) => {
                if request.is_empty() || request.contains('\0') {
                    return Err(invalid("require path must be nonempty and contain no NUL"));
                }

                let request = request.replace('\\', "/");

                if let Some(request) = request.strip_prefix("@game/")
                    && importer.effective.settings.environment.roblox
                {
                    let steps = request
                        .split('/')
                        .filter(|step| !step.is_empty() && *step != ".")
                        .map(|step| {
                            if step == ".." {
                                InstanceStep::Parent
                            } else {
                                InstanceStep::Child {
                                    name: step.to_owned(),
                                    recursive: false,
                                }
                            }
                        })
                        .collect::<Vec<_>>();

                    return instance(importer, InstanceRoot::Game, &steps, capture, options);
                }

                let path = capture
                    .snapshot
                    .origin(importer.source.version.identity)
                    .and_then(SourceOrigin::path)
                    .ok_or_else(|| invalid("source has no filesystem resolution context"))?;

                if path.file_name().is_some_and(|name| name == "instar.luau") {
                    return Err(invalid("instar.luau is not an importable module"));
                }

                let start = if let Some(alias) = request.strip_prefix('@') {
                    let (name, rest) = alias.split_once('/').unwrap_or((alias, ""));

                    importer
                        .effective
                        .settings
                        .resolution
                        .alias(name, &module_path(path))?
                        .join(rest)
                } else {
                    if !request.starts_with("./") && !request.starts_with("../") {
                        return Err(invalid("require path must start with ./, ../, or @"));
                    }

                    let module = module_path(path);

                    let parent = module
                        .parent()
                        .ok_or_else(|| invalid("module has no resolution parent"))?;

                    parent.join(request)
                };

                let base = path
                    .parent()
                    .and_then(|parent| {
                        parent
                            .ancestors()
                            .find(|ancestor| start.starts_with(ancestor))
                    })
                    .map(Path::to_path_buf);

                let (path, failure) = navigation(&start, base.as_deref(), capture, options)?;
                let target = probe(&path, capture, options);
                options.check()?;

                if let Some(error) = failure {
                    return Err(error);
                }

                let path = target?;

                let map = if let Some((map, caller)) = &importer.map {
                    capture.source(&map.path, options)?;

                    match map.placements(&path).as_slice() {
                        [node] => {
                            map.module_source(*node, true)?;

                            Some((Arc::clone(map), *node))
                        }

                        [] => Some((Arc::clone(map), *caller)),

                        _ => {
                            return Err(invalid(
                                "dependency maps to multiple instances in the caller's context",
                            ));
                        }
                    }
                } else {
                    None
                };

                Ok(Some(ResolvedTarget { path, map }))
            }

            ImportRequest::Instance(path) => {
                instance(importer, path.root, &path.steps, capture, options)
            }
        }
    }
}

pub(crate) fn probe(path: &Path, capture: &mut Capture, options: &Options) -> io::Result<PathBuf> {
    if path.file_name().is_some_and(|name| name == "instar.luau") {
        return Err(invalid("instar.luau is not an importable module"));
    }

    if matches!(
        path.extension().and_then(|extension| extension.to_str()),
        Some("lua" | "luau")
    ) {
        return capture.source(path, options).map(|_| path.to_path_buf());
    }

    let mut candidates = Vec::new();

    for extension in ["luau", "lua"] {
        let mut name = path.as_os_str().to_os_string();
        name.push(format!(".{extension}"));
        candidates.push(PathBuf::from(name));
    }

    candidates.extend([path.join("init.luau"), path.join("init.lua")]);
    capture.listing(path, options)?;

    if let Some(parent) = path.parent() {
        capture.listing(parent, options)?;
    }

    let mut sources = Vec::new();
    let mut aliases = Vec::new();

    for candidate in &candidates {
        if capture.read(candidate, options)?.is_some() {
            if candidate
                .file_name()
                .is_some_and(|name| name == "instar.luau")
            {
                return Err(invalid("instar.luau is not an importable module"));
            }

            if module_path(candidate) == path {
                sources.push(candidate.clone());
            } else if path.file_name().is_some_and(|name| name == "init") {
                aliases.push(candidate.clone());
            }
        }
    }

    if sources.is_empty() {
        sources = aliases;
    }

    match sources.as_slice() {
        [source] => Ok(source.clone()),

        [] => Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!(
                "unresolved module {} (consulted {})",
                path.display(),
                candidates
                    .iter()
                    .map(|path| path.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        )),

        _ => Err(invalid(format!(
            "ambiguous module {}: {}",
            path.display(),
            sources
                .iter()
                .map(|source| source.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        ))),
    }
}

fn instance(
    importer: &ModuleNode,
    root: InstanceRoot,
    steps: &[InstanceStep],
    capture: &mut Capture,
    options: &Options,
) -> io::Result<Option<ResolvedTarget>> {
    let (map, script) = if let Some((map, script)) = &importer.map {
        capture.source(&map.path, options)?;

        (Arc::clone(map), Some(*script))
    } else {
        let maps = &importer.effective.settings.resolution.sourcemaps;

        if maps.len() != 1 {
            return Err(invalid("instance request has no unique sourcemap context"));
        }

        let source = capture.source(&maps[0], options)?;

        (Map::parse(&maps[0], &source)?, None)
    };

    let mut node = match root {
        InstanceRoot::Script => {
            script.ok_or_else(|| invalid("script has no mapped source context"))?
        }

        InstanceRoot::Game => map.root()?,
        InstanceRoot::Workspace => map.service(map.root()?, "Workspace")?,
    };

    for step in steps {
        options.check()?;

        node = match step {
            InstanceStep::Parent => map.parent(node)?,

            InstanceStep::Field(name) => {
                for input in &importer.environment.member_inputs {
                    capture.consult_input(input);
                }

                let assets = importer.environment.assets.as_ref().ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::Unsupported,
                        "instance field requires verified member precedence",
                    )
                })?;

                if assets.members.contains(map.class(node), name)? {
                    return Ok(None);
                }

                map.child(node, name, false)?
            }

            InstanceStep::Child { name, recursive } => map.child(node, name, *recursive)?,
            InstanceStep::Service(name) => map.service(node, name)?,
        };
    }

    let path = map.module_source(node, true)?;

    Ok(Some(ResolvedTarget {
        path,
        map: Some((map, node)),
    }))
}

pub(crate) fn module_path(source: &Path) -> PathBuf {
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

fn navigation(
    path: &Path,
    base: Option<&Path>,
    capture: &mut Capture,
    options: &Options,
) -> io::Result<(PathBuf, Option<io::Error>)> {
    let (mut current, relative) = if let Some(base) = base {
        (
            base.to_path_buf(),
            path.strip_prefix(base).map_err(invalid)?,
        )
    } else {
        (PathBuf::new(), path)
    };

    let mut components = relative.components().peekable();
    let mut failure = None;

    while let Some(component) = components.next() {
        options.check()?;

        match component {
            Component::Prefix(_) | Component::RootDir => current.push(component.as_os_str()),
            Component::CurDir => {}

            Component::ParentDir => {
                if !directory(&current, capture, options)? && failure.is_none() {
                    failure = Some(io::Error::new(
                        io::ErrorKind::NotFound,
                        format!("navigation directory not found: {}", current.display()),
                    ));
                }

                if !current.pop() {
                    return Err(invalid("module path navigates above its root"));
                }
            }

            Component::Normal(name) => {
                current.push(name);

                if components.peek().is_some()
                    && !directory(&current, capture, options)?
                    && failure.is_none()
                {
                    let message = match probe(&current, capture, options) {
                        Ok(_) => "navigation directory not found",

                        Err(error) if error.kind() == io::ErrorKind::NotFound => {
                            "module path not found"
                        }

                        Err(error) => {
                            failure = Some(error);
                            continue;
                        }
                    };

                    failure = Some(io::Error::new(
                        io::ErrorKind::NotFound,
                        format!("{message}: {}", current.display()),
                    ));
                }
            }
        }
    }

    Ok((current, failure))
}

fn directory(path: &Path, capture: &mut Capture, options: &Options) -> io::Result<bool> {
    match capture.listing(path, options)? {
        Listing::Directory(_) => return Ok(true),
        Listing::NonDirectory => return Ok(false),
        Listing::Missing => {}
    }

    let document = capture
        .snapshot
        .sources()
        .find(|source| {
            matches!(
                capture.snapshot.origin(source.version.identity),
                Some(SourceOrigin::Document {
                    context: Some(context),
                    ..
                }) if context != path && context.starts_with(path)
            )
        })
        .cloned();

    if let Some(document) = document {
        capture.consult_source(&document);

        return Ok(true);
    }

    Ok(false)
}

fn invalid(message: impl Display) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message.to_string())
}
