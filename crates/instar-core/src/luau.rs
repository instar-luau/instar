#![allow(unsafe_code)]

use std::{
    collections::BTreeMap,
    ffi::c_void,
    io,
    panic::{AssertUnwindSafe, catch_unwind},
    path::{Path, PathBuf},
    ptr, slice,
};

use crate::{
    analysis::{Annotation, Diagnostic, Options, Report},
    project::resolution::Resolver,
};

#[repr(C)]
#[derive(Clone, Copy)]
struct Bytes {
    data: *const u8,
    size: usize,
}

impl Bytes {
    fn new(bytes: &[u8]) -> Self {
        Self {
            data: bytes.as_ptr(),
            size: bytes.len(),
        }
    }
    fn absent() -> Self {
        Self {
            data: ptr::null(),
            size: 0,
        }
    }
    unsafe fn slice<'value>(self) -> &'value [u8] {
        if self.size == 0 {
            &[]
        } else {
            unsafe { slice::from_raw_parts(self.data, self.size) }
        }
    }
    unsafe fn string(self) -> io::Result<String> {
        std::str::from_utf8(unsafe { self.slice() })
            .map(str::to_owned)
            .map_err(io::Error::other)
    }
}

type Read = extern "C" fn(*mut c_void, Bytes) -> Bytes;
type Resolve = extern "C" fn(*mut c_void, Bytes, Bytes, u32) -> Bytes;
type Environment = extern "C" fn(*mut c_void, Bytes, Bytes, usize) -> Bytes;
type Configuration = extern "C" fn(*mut c_void, Bytes, usize) -> Bytes;
#[repr(C)]
struct Span {
    line: u32,
    column: u32,
    end_line: u32,
    end_column: u32,
    related: bool,
}

type Emit = extern "C" fn(*mut c_void, Bytes, Bytes, Span, bool);
type Annotate = extern "C" fn(*mut c_void, Bytes, Bytes);
type Alias = extern "C" fn(*mut c_void, Bytes, Bytes);

pub(crate) fn matches(pattern: &str, source: &str) -> io::Result<bool> {
    match unsafe {
        instar_matches(
            Bytes::new(pattern.as_bytes()),
            Bytes::new(source.as_bytes()),
        )
    } {
        0 => Ok(false),
        1 => Ok(true),
        _ => Err(io::Error::other("invalid lint ignore pattern")),
    }
}

unsafe extern "C" {
    fn instar_matches(pattern: Bytes, source: Bytes) -> i32;
    fn instar_aliases(
        source: Bytes,
        executable: bool,
        context: *mut c_void,
        alias: Alias,
        report: Emit,
    );
    fn instar_destroy(session: *mut c_void);
    fn instar_query(
        session: *mut c_void,
        context: *mut c_void,
        path: Bytes,
        line: u32,
        column: u32,
        operation: Bytes,
        output: Annotate,
    );
    fn instar_analyze(
        session: *mut *mut c_void,
        context: *mut c_void,
        read: Read,
        resolve: Resolve,
        configuration: Configuration,
        report: Emit,
        annotate: Annotate,
        modules: *const Bytes,
        count: usize,
        definitions: *const Bytes,
        definition_count: usize,
        configuration_types: Bytes,
        environment: Environment,
        mode: Bytes,
        old_solver: bool,
        annotations: bool,
        changed: *const Bytes,
        changed_count: usize,
    );
}

struct Context<'resolver, 'store> {
    resolver: &'resolver mut Resolver<'store>,
    buffer: Vec<u8>,
    environment: std::sync::Arc<crate::roblox::Environment>,
    report: Report,
    error: Option<io::Error>,
}

impl Context<'_, '_> {
    fn call(&mut self, operation: impl FnOnce(&mut Self) -> io::Result<Option<Vec<u8>>>) -> Bytes {
        if self.error.is_some() {
            return Bytes::absent();
        }

        match catch_unwind(AssertUnwindSafe(|| operation(self))) {
            Ok(Ok(Some(bytes))) => {
                self.buffer = bytes;

                Bytes::new(&self.buffer)
            }

            Ok(Ok(None)) => Bytes::absent(),

            Ok(Err(error)) => {
                self.error = Some(error);

                Bytes::absent()
            }

            Err(_) => {
                self.error = Some(io::Error::other("native callback panicked"));

                Bytes::absent()
            }
        }
    }
}

extern "C" fn read(context: *mut c_void, name: Bytes) -> Bytes {
    let context = unsafe { &mut *context.cast::<Context<'_, '_>>() };

    context.call(|context| {
        let name = unsafe { name.string()? };

        let path = Path::new(&name);

        if !context.environment.readable(path) {
            return Ok(None);
        }

        Ok(Some(
            context
                .resolver
                .load(&context.environment.source(path))?
                .bytes()
                .to_vec(),
        ))
    })
}

extern "C" fn resolve(context: *mut c_void, from: Bytes, specifier: Bytes, kind: u32) -> Bytes {
    let context = unsafe { &mut *context.cast::<Context<'_, '_>>() };

    context.call(|context| {
        let from = unsafe { from.string()? };

        let specifier = unsafe { specifier.string()? };

        let path = if kind == 0 {
            context
                .environment
                .require(context.resolver, Path::new(&from), &specifier)?
        } else {
            context
                .environment
                .resolve(Path::new(&from), &specifier, kind)
        };

        path.map(|path| module_name(&path).map(String::into_bytes))
            .transpose()
    })
}

extern "C" fn configuration(context: *mut c_void, name: Bytes, index: usize) -> Bytes {
    let context = unsafe { &mut *context.cast::<Context<'_, '_>>() };

    context.call(|context| {
        let name = unsafe { name.string()? };

        context
            .resolver
            .discovery
            .configurations(&context.environment.configuration(Path::new(&name)))?
            .get(index)
            .map(|configuration| -> io::Result<_> {
                let mut bytes = module_name(&configuration.path)?.into_bytes();
                bytes.push(0);
                bytes.extend_from_slice(&configuration.bytes);

                Ok(bytes)
            })
            .transpose()
    })
}

extern "C" fn metadata(context: *mut c_void, category: Bytes, name: Bytes, index: usize) -> Bytes {
    let context = unsafe { &mut *context.cast::<Context<'_, '_>>() };

    context.call(|context| {
        let category = unsafe { category.string()? };

        let name = unsafe { name.string()? };

        let environment = &context.environment;

        let value = match category.as_str() {
            "enabled" => environment.enabled.then(String::new),

            "definitions" => environment.enabled.then(|| environment.definitions.clone()),

            "enumeration" => environment.enumerations.get(index).cloned(),
            "class" => environment.classes.get(index).cloned(),

            "node" => environment.nodes.get(index).map(|node| {
                format!(
                    "{}\0{}\0{}",
                    node.name,
                    node.class_name,
                    node.parent
                        .map(|parent| parent.to_string())
                        .unwrap_or_default()
                )
            }),

            "script" => environment
                .node(Path::new(&name))
                .map(|index| index.to_string()),

            "kind" => environment.enabled.then(|| {
                environment.node(Path::new(&name)).map_or_else(
                    || {
                        if name.ends_with(".server.lua") || name.ends_with(".server.luau") {
                            "Script"
                        } else if name.ends_with(".client.lua") || name.ends_with(".client.luau") {
                            "LocalScript"
                        } else {
                            "ModuleScript"
                        }
                        .to_owned()
                    },
                    |index| environment.nodes[index].class_name.clone(),
                )
            }),

            "path" => Some(module_name(&environment.source(Path::new(&name)))?),
            _ => None,
        };

        Ok(value.map(String::into_bytes))
    })
}

extern "C" fn emit(context: *mut c_void, name: Bytes, message: Bytes, span: Span, is_error: bool) {
    let context = unsafe { &mut *context.cast::<Context<'_, '_>>() };

    context.call(|context| {
        if span.related {
            if let Some(diagnostic) = context.report.diagnostics.last_mut() {
                diagnostic.related.push(crate::analysis::RelatedDiagnostic {
                    path: context
                        .environment
                        .source(Path::new(&unsafe { name.string()? })),
                    range: [span.line, span.column, span.end_line, span.end_column],
                    message: String::from_utf8_lossy(unsafe { message.slice() }).into_owned(),
                });
            }

            return Ok(None);
        }

        context.report.diagnostics.push(Diagnostic {
            path: context
                .environment
                .source(Path::new(&unsafe { name.string()? })),
            line: span.line,
            column: span.column,
            end_line: span.end_line,
            end_column: span.end_column,
            message: String::from_utf8_lossy(unsafe { message.slice() }).into_owned(),
            is_error,
            related: Vec::new(),
        });

        Ok(None)
    });
}

extern "C" fn annotate(context: *mut c_void, name: Bytes, text: Bytes) {
    let context = unsafe { &mut *context.cast::<Context<'_, '_>>() };

    context.call(|context| {
        context.report.annotations.push(Annotation {
            path: context
                .environment
                .source(Path::new(&unsafe { name.string()? })),
            bytes: unsafe { text.slice() }.to_vec(),
        });

        Ok(None)
    });
}

fn module_name(path: &Path) -> io::Result<String> {
    path.to_str()
        .map(|path| {
            #[cfg(windows)]
            {
                path.replace('\\', "/")
            }

            #[cfg(not(windows))]
            {
                path.to_owned()
            }
        })
        .ok_or_else(|| {
            io::Error::other(format!(
                "{}: module identity requires UTF-8",
                path.display()
            ))
        })
}

fn names(resolver: &mut Resolver<'_>, paths: &[PathBuf]) -> io::Result<Vec<String>> {
    paths
        .iter()
        .map(|path| {
            resolver
                .load(path)
                .and_then(|source| module_name(source.path()))
        })
        .collect()
}

#[derive(Default)]
pub(crate) struct Session {
    handle: *mut c_void,
    changed: Vec<PathBuf>,
    refresh: bool,
    query: Option<(PathBuf, line_index::LineCol, String)>,

    environments:
        BTreeMap<crate::configuration::RobloxConfig, std::sync::Arc<crate::roblox::Environment>>,
}

impl Drop for Session {
    fn drop(&mut self) {
        unsafe { instar_destroy(self.handle) };
    }
}

impl Session {
    pub(crate) fn query(
        &mut self,
        resolver: &mut Resolver<'_>,
        modules: &[PathBuf],
        path: &Path,
        position: line_index::LineCol,
        operation: &str,
    ) -> io::Result<Report> {
        self.query = Some((path.to_owned(), position, operation.to_owned()));
        let result = self.analyze(resolver, modules, &Options::default());
        self.query = None;

        result
    }

    pub(crate) fn refresh(&mut self) {
        self.refresh = true;
    }

    pub(crate) fn change(&mut self, path: &Path) {
        self.changed.push(path.to_owned());
    }

    pub(crate) fn analyze(
        &mut self,
        resolver: &mut Resolver<'_>,
        modules: &[PathBuf],
        options: &Options,
    ) -> io::Result<Report> {
        let changed = std::mem::take(&mut self.changed);

        let incremental = !std::mem::take(&mut self.refresh)
            && !options.update
            && (!changed.is_empty() || self.query.is_some());

        if !incremental {
            self.environments.clear();
        }

        let mut groups = BTreeMap::new();

        for path in modules {
            let source = resolver.load(path)?;
            let path = source.path().to_owned();

            let configuration = crate::project::ConfigKind::from_path(&path)
                == Some(crate::project::ConfigKind::Luau);

            let mut definitions = if configuration {
                Vec::new()
            } else {
                resolver.discovery.definitions(&path)?
            };

            if path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.ends_with(".d.luau"))
                && !definitions.contains(&path)
            {
                definitions.push(path.clone());
            }

            let environment = if configuration {
                None
            } else {
                resolver.discovery.roblox(&path)?
            };

            groups
                .entry((definitions, environment))
                .or_insert_with(Vec::new)
                .push(path);
        }

        let mut report = Report::default();

        for ((definitions, environment), modules) in groups {
            let result = self.analyze_group(
                resolver,
                &modules,
                &definitions,
                environment.as_ref(),
                options,
                incremental.then_some(changed.as_slice()),
            )?;

            report.diagnostics.extend(result.diagnostics);
            report.annotations.extend(result.annotations);
            report.documentation.extend(result.documentation);

            if result.editor.is_some() {
                report.editor = result.editor;
            }
        }

        let mut seen = std::collections::BTreeSet::new();

        report.diagnostics.retain(|diagnostic| {
            seen.insert((
                diagnostic.path.clone(),
                diagnostic.line,
                diagnostic.column,
                diagnostic.message.clone(),
                diagnostic.is_error,
            ))
        });

        Ok(report)
    }

    pub(crate) fn environment(
        &mut self,
        resolver: &mut Resolver<'_>,
        settings: Option<&crate::configuration::RobloxConfig>,
        update: bool,
    ) -> io::Result<std::sync::Arc<crate::roblox::Environment>> {
        let Some(settings) = settings else {
            return Ok(std::sync::Arc::default());
        };

        if let Some(environment) = self.environments.get(settings) {
            return Ok(std::sync::Arc::clone(environment));
        }

        let environment = std::sync::Arc::new(crate::roblox::Environment::load(
            resolver, settings, update,
        )?);

        self.environments
            .insert(settings.clone(), std::sync::Arc::clone(&environment));

        Ok(environment)
    }

    fn analyze_group(
        &mut self,
        resolver: &mut Resolver<'_>,
        modules: &[PathBuf],
        definitions: &[PathBuf],
        settings: Option<&crate::configuration::RobloxConfig>,
        options: &Options,
        changed: Option<&[PathBuf]>,
    ) -> io::Result<Report> {
        let environment = self.environment(resolver, settings, options.update)?;
        let mut changed_names = Vec::new();

        for path in changed.unwrap_or_default() {
            changed_names.push(module_name(path)?);

            if let Some(index) = environment.node(path) {
                changed_names.push(module_name(&environment.identity(index))?);
            }
        }

        let changed_values = changed_names
            .iter()
            .map(|name| Bytes::new(name.as_bytes()))
            .collect::<Vec<_>>();

        let definition_names = names(resolver, definitions)?;

        let definitions: Vec<_> = definition_names
            .iter()
            .map(|name| Bytes::new(name.as_bytes()))
            .collect();

        let names = names(resolver, modules)?;

        let modules: Vec<_> = names
            .iter()
            .map(|name| Bytes::new(name.as_bytes()))
            .collect();

        let mut context = Context {
            resolver,
            buffer: Vec::new(),
            environment,
            report: Report::default(),
            error: None,
        };

        unsafe {
            instar_analyze(
                ptr::from_mut(&mut self.handle),
                ptr::from_mut(&mut context).cast(),
                read,
                resolve,
                configuration,
                emit,
                annotate,
                modules.as_ptr(),
                modules.len(),
                definitions.as_ptr(),
                definitions.len(),
                Bytes::new(include_bytes!("../bridge/configuration.d.luau")),
                metadata,
                options.mode.map_or_else(Bytes::absent, |mode| {
                    Bytes::new(match mode {
                        crate::analysis::Mode::Strict => b"strict",
                        crate::analysis::Mode::Nonstrict => b"nonstrict",
                        crate::analysis::Mode::Nocheck => b"nocheck",
                    })
                }),
                options.old_solver,
                options.annotations,
                changed.map_or(ptr::null(), |_| changed_values.as_ptr()),
                changed_values.len(),
            );
        }

        if context.error.is_none()
            && self
                .query
                .as_ref()
                .is_some_and(|(path, _, _)| names.iter().any(|name| Path::new(name) == path))
            && let Some((path, position, operation)) = self.query.take()
        {
            let name = module_name(&path)?;

            unsafe {
                instar_query(
                    self.handle,
                    ptr::from_mut(&mut context).cast(),
                    Bytes::new(name.as_bytes()),
                    position.line,
                    position.col,
                    Bytes::new(operation.as_bytes()),
                    editor,
                );
            }
        }

        if let Some(error) = context.error {
            *self = Self::default();

            return Err(error);
        }

        if !context.environment.documentation.is_empty() {
            let documentation = std::sync::Arc::clone(&context.environment.documentation);

            for name in names {
                context
                    .report
                    .documentation
                    .insert(PathBuf::from(name), std::sync::Arc::clone(&documentation));
            }
        }

        Ok(context.report)
    }
}

extern "C" fn editor(context: *mut c_void, _: Bytes, payload: Bytes) {
    let context = unsafe { &mut *context.cast::<Context<'_, '_>>() };

    context.call(|context| {
        context.report.editor =
            serde_json::from_slice(unsafe { payload.slice() }).map_err(io::Error::other)?;

        let normalize = |entry: &mut crate::analysis::EditorEntry| {
            if let Some(path) = &mut entry.path {
                *path = context.environment.source(path);
            }
        };

        match &mut context.report.editor {
            Some(crate::analysis::EditorResult::Entries(entries)) => {
                entries.iter_mut().for_each(normalize);
            }

            Some(crate::analysis::EditorResult::Entry(entry)) => normalize(entry),
            None => {}
        }

        Ok(None)
    });
}

#[derive(Default)]
struct Aliases {
    values: BTreeMap<String, String>,
    error: Option<String>,
}

extern "C" fn alias(context: *mut c_void, name: Bytes, target: Bytes) {
    let context = unsafe { &mut *context.cast::<Aliases>() };

    let result = catch_unwind(AssertUnwindSafe(|| -> io::Result<()> {
        context
            .values
            .insert(unsafe { name.string()? }, unsafe { target.string()? });

        Ok(())
    }));

    match result {
        Ok(Ok(())) => {}
        Ok(Err(error)) => context.error = Some(error.to_string()),
        Err(_) => context.error = Some("alias callback panicked".into()),
    }
}

extern "C" fn alias_error(context: *mut c_void, _: Bytes, message: Bytes, _: Span, _: bool) {
    let context = unsafe { &mut *context.cast::<Aliases>() };

    let result = catch_unwind(AssertUnwindSafe(|| {
        String::from_utf8_lossy(unsafe { message.slice() }).into_owned()
    }));

    context.error = Some(result.unwrap_or_else(|_| "configuration callback panicked".into()));
}

pub(crate) fn aliases(source: &[u8], executable: bool) -> io::Result<BTreeMap<String, String>> {
    let mut aliases = Aliases::default();

    unsafe {
        instar_aliases(
            Bytes::new(source),
            executable,
            ptr::from_mut(&mut aliases).cast(),
            alias,
            alias_error,
        );
    }

    if let Some(error) = aliases.error {
        return Err(io::Error::other(error));
    }

    Ok(aliases.values)
}
