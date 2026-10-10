use std::{
    fmt::Display,
    io,
    path::{Component, Path, PathBuf},
    sync::Arc,
};

use instar_luau::{ImportRequest, ImportSite, InstanceRoot, InstanceStep};
use instar_shared::Options;

use crate::{
    capture::Capture, execution::Runtime, graph::ModuleNode, inputs::SourceOrigin, sourcemap::Map,
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
            capture.consult_input(input.clone());
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

                let path = probe(&navigation(&start, capture, options)?, capture, options)?;

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

    for candidate in &candidates {
        if capture.read(candidate, options)?.is_some() {
            if candidate
                .file_name()
                .is_some_and(|name| name == "instar.luau")
            {
                return Err(invalid("instar.luau is not an importable module"));
            }

            sources.push(candidate.clone());
        }
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
                    capture.consult_input(input.clone());
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

fn navigation(path: &Path, capture: &mut Capture, options: &Options) -> io::Result<PathBuf> {
    let mut current = PathBuf::new();
    let mut components = path.components().peekable();

    while let Some(component) = components.next() {
        options.check()?;

        match component {
            Component::Prefix(_) | Component::RootDir => current.push(component.as_os_str()),
            Component::CurDir => {}

            Component::ParentDir => {
                if capture.listing(&current, options)?.is_none() {
                    return Err(io::Error::new(
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

                if components.peek().is_some() && capture.listing(&current, options)?.is_none() {
                    return Err(io::Error::new(
                        io::ErrorKind::NotFound,
                        format!("navigation directory not found: {}", current.display()),
                    ));
                }
            }
        }
    }

    Ok(current)
}

fn invalid(message: impl Display) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message.to_string())
}
