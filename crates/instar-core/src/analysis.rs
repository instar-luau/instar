//! Luau analysis using the project's resolver on demand.

use std::{
    collections::{BTreeSet, HashMap},
    io,
    path::{Path, PathBuf},
    rc::Rc,
};

use instar_bridge::{
    Callbacks, Checker, CheckerOptions, Configuration, ResolveRequest, Source,
    native::{DiagnosticSeverity, SourceKind},
};

use crate::{
    filter::Service,
    graph::{self, Request},
    invalid,
    project::{EffectiveConfig, Project, RobloxSettings},
    resolve::{Failure, Module, Resolver},
    roblox::Sourcemap,
};

/// Source context and zero-based line/byte-column range of a diagnostic.
#[derive(Debug)]
pub struct Location {
    /// Physical source and optional place/instance identity.
    pub module: Module,

    /// Start line, start column, end line, and end column.
    pub range: [u32; 4],
}

/// A diagnostic emitted by Luau, with module identities mapped back to source contexts.
#[derive(Debug)]
pub struct Diagnostic {
    /// Source location.
    pub location: Location,

    /// Whether this is an error rather than a warning.
    pub error: bool,

    /// Luau's explanation.
    pub message: String,

    /// Related source location and explanation, if present.
    pub related: Option<(Location, String)>,
}

struct LoadedModule {
    module: Module,
    configuration: Rc<EffectiveConfig>,
    source: Option<Rc<str>>,
    expressions: Option<HashMap<[usize; 2], Request>>,
    line_starts: Vec<usize>,
}

struct Host<'project> {
    project: &'project mut Project,
    resolver: Resolver,
    identities: HashMap<Module, String>,
    modules: HashMap<String, LoadedModule>,
    diagnostics: Vec<Diagnostic>,
    open: Option<&'project BTreeSet<PathBuf>>,
}

impl Host<'_> {
    fn service(&self) -> Service {
        if self.open.is_some() {
            Service::Lsp
        } else {
            Service::Analyze
        }
    }

    fn intern(&mut self, module: Module) -> io::Result<String> {
        if let Some(name) = self.identities.get(&module) {
            return Ok(name.clone());
        }

        let source = module
            .source
            .to_str()
            .ok_or_else(|| invalid("module identity requires UTF-8"))?;

        let name = if let Some(instance) = &module.instance {
            format!(
                "{source} [{}; {}; {}]",
                instance.full_name(),
                instance.sourcemap_path().display(),
                self.modules.len()
            )
        } else {
            source.to_owned()
        };

        let configuration = self.project.configuration(&module.source)?;
        self.identities.insert(module.clone(), name.clone());

        self.modules.insert(
            name.clone(),
            LoadedModule {
                module,
                configuration,
                source: None,
                expressions: None,
                line_starts: Vec::new(),
            },
        );

        Ok(name)
    }

    fn alias(&mut self, module: Module, name: &str) -> io::Result<()> {
        let configuration = self.project.configuration(&module.source)?;
        self.identities.insert(module.clone(), name.to_owned());

        self.modules.insert(
            name.to_owned(),
            LoadedModule {
                module,
                configuration,
                source: None,
                expressions: None,
                line_starts: Vec::new(),
            },
        );

        Ok(())
    }

    fn location(&self, name: &str, range: [u32; 4]) -> io::Result<Location> {
        let module = self
            .modules
            .get(name)
            .ok_or_else(|| invalid(format!("unknown analysis module {name}")))?;

        Ok(Location {
            module: module.module.clone(),
            range,
        })
    }
}

impl Callbacks for Host<'_> {
    fn source<'source>(&'source mut self, name: &str) -> io::Result<Source<'source>> {
        let state = self
            .modules
            .get_mut(name)
            .ok_or_else(|| invalid(format!("unknown analysis module {name}")))?;

        state.source = Some(self.project.source(&state.module.source)?);

        let kind = if state
            .module
            .instance
            .as_ref()
            .is_some_and(|instance| matches!(instance.class_name(), "Script" | "LocalScript"))
        {
            SourceKind::SourceScript
        } else {
            SourceKind::SourceModule
        };

        Ok(Source {
            bytes: state
                .source
                .as_ref()
                .expect("source was populated")
                .as_bytes(),
            kind,
        })
    }

    fn configuration(&mut self, name: &str) -> io::Result<&Configuration> {
        let state = self
            .modules
            .get(name)
            .ok_or_else(|| invalid(format!("unknown analysis module {name}")))?;

        state.configuration.analysis_native(
            &state.module.source,
            self.service(),
            self.open
                .is_none_or(|open| open.contains(&state.module.source)),
        )
    }

    fn resolve(&mut self, request: &ResolveRequest<'_>) -> io::Result<Option<String>> {
        let state = self
            .modules
            .get_mut(request.from)
            .ok_or_else(|| invalid(format!("unknown analysis module {}", request.from)))?;

        let source = state
            .source
            .as_ref()
            .ok_or_else(|| invalid("resolution requested before loading source"))?;

        if state.expressions.is_none() {
            let map = if let Some(instance) = &state.module.instance {
                Ok(Some(Rc::clone(&instance.map)))
            } else {
                self.project
                    .sourcemap(&state.module.source)
                    .map_err(|error| Failure::Configuration(error.to_string()))
            };

            let extracted = graph::extract(source, state.module.instance.clone(), map);
            state.expressions = Some(extracted.expressions);
            state.line_starts = extracted.line_starts;
        }

        let offset = |line: u32, column: u32| -> io::Result<usize> {
            let line = usize::try_from(line).map_err(|error| invalid(error.to_string()))?;
            let column = usize::try_from(column).map_err(|error| invalid(error.to_string()))?;

            let start = state
                .line_starts
                .get(line)
                .ok_or_else(|| invalid("resolution line outside source"))?;

            let offset = start
                .checked_add(column)
                .ok_or_else(|| invalid("resolution column overflow"))?;

            let end = state
                .line_starts
                .get(line + 1)
                .map_or(source.len(), |next| next - 1);

            if offset > end {
                return Err(invalid("resolution column outside source line"));
            }

            Ok(offset)
        };

        let range = [
            offset(request.location[0], request.location[1])?,
            offset(request.location[2], request.location[3])?,
        ];

        let Some(target) = state
            .expressions
            .as_ref()
            .and_then(|expressions| expressions.get(&range))
        else {
            return Ok(None);
        };

        // Resolve the expression in its original lexical/place context, even when Luau has no intermediate context.
        let result = match target {
            Request::String(value) => {
                self.resolver
                    .resolve(self.project, &state.module, value)
                    .result
            }

            Request::Instance(instance) => Resolver::resolve_instance(instance).result,
        };

        match result {
            Ok(module) => self.intern(module).map(Some),
            Err(Failure::Configuration(message) | Failure::Io(message)) => Err(invalid(message)),
            Err(_) => Ok(None),
        }
    }

    fn diagnostic(&mut self, diagnostic: instar_bridge::Diagnostic<'_>) -> io::Result<()> {
        if diagnostic.path.starts_with('@') && !self.modules.contains_key(diagnostic.path) {
            return Err(invalid(format!(
                "{} declarations: {}",
                diagnostic.path, diagnostic.message
            )));
        }

        let location = self.location(diagnostic.path, diagnostic.location)?;

        if self
            .open
            .is_some_and(|open| !open.contains(&location.module.source))
        {
            return Ok(());
        }

        if !self
            .project
            .includes(&location.module.source, self.service())?
        {
            return Ok(());
        }

        let related = diagnostic
            .related
            .map(|related| {
                self.location(related.path, related.location)
                    .map(|location| (location, related.message.to_owned()))
            })
            .transpose()?;

        self.diagnostics.push(Diagnostic {
            location,
            error: diagnostic.severity == DiagnosticSeverity::DiagnosticError,
            message: diagnostic.message.to_owned(),
            related,
        });

        Ok(())
    }
}

struct Environment {
    settings: RobloxSettings,
    definitions: Vec<(String, String)>,
    map: Option<Rc<Sourcemap>>,
    entries: Vec<Module>,
}

fn environments(
    project: &mut Project,
    paths: &[PathBuf],
    service: Service,
) -> io::Result<Vec<Environment>> {
    let mut resolver = Resolver::new();
    let mut environments = HashMap::<_, Environment>::new();

    for path in paths {
        if !project.includes(path, service)? {
            continue;
        }

        let modules = if is_declaration_source(path) {
            let source = crate::absolute(path)?;

            vec![Module {
                path: crate::resolve::module_path(&source),
                source,
                instance: None,
            }]
        } else {
            resolver
                .entries(project, path)
                .map_err(|error| invalid(error.to_string()))?
        };

        for module in modules {
            let config = project.configuration(&module.source)?;

            if config
                .settings
                .definitions
                .values()
                .any(|path| Path::new(path) == module.source)
                && !is_declaration_source(&module.source)
            {
                continue;
            }

            let settings = config.roblox.clone();

            let definitions: Vec<_> = config
                .definition_order
                .iter()
                .map(|name| (name.clone(), config.settings.definitions[name].clone()))
                .collect();

            let map = if let Some(instance) = &module.instance {
                Some(Rc::clone(&instance.map))
            } else {
                let maps = project.sourcemaps_for(&module.source)?;

                if maps.len() == 1 {
                    maps.into_iter().next()
                } else {
                    None
                }
            };

            let key = (
                settings.enabled,
                settings.security,
                definitions.clone(),
                map.as_ref().map(|map| map.path.clone()),
            );

            environments
                .entry(key)
                .or_insert_with(|| Environment {
                    settings,
                    definitions,
                    map,
                    entries: Vec::new(),
                })
                .entries
                .push(module);
        }
    }

    Ok(environments.into_values().collect())
}

/// Checks CLI entries and dependencies with all eligible lint passes.
///
/// # Errors
/// Returns source, configuration, asset, or native callback failures.
pub fn check(project: &mut Project, paths: &[PathBuf]) -> io::Result<Vec<Diagnostic>> {
    let environments = environments(project, paths, Service::Analyze)?;

    let mut diagnostics = Vec::new();

    for environment in environments {
        diagnostics.extend(check_environment(project, environment)?);
    }

    diagnostics.sort_by(|a, b| {
        a.location
            .module
            .source
            .cmp(&b.location.module.source)
            .then_with(|| {
                a.location
                    .module
                    .instance
                    .as_ref()
                    .map(|instance| (instance.sourcemap_path(), instance.full_name()))
                    .cmp(
                        &b.location
                            .module
                            .instance
                            .as_ref()
                            .map(|instance| (instance.sourcemap_path(), instance.full_name())),
                    )
            })
            .then(a.location.range.cmp(&b.location.range))
            .then_with(|| a.message.cmp(&b.message))
    });

    Ok(diagnostics)
}

fn load_entry_declarations(
    host: &mut Host<'_>,
    checker: &mut Checker,
    entries: &BTreeSet<String>,
    definitions: &[(String, String)],
) -> io::Result<Option<Vec<Diagnostic>>> {
    for module in entries {
        let module = host
            .modules
            .get(module)
            .expect("entry was interned")
            .module
            .clone();

        if is_declaration_source(&module.source)
            && !definitions.iter().any(|(_, path)| {
                crate::absolute(Path::new(path)).is_ok_and(|path| path == module.source)
            })
        {
            let name = host.intern(module)?;
            let source = host.project.source(&host.modules[&name].module.source)?;
            let before = host.diagnostics.len();

            if let Err(error) = checker.load_definition(host, source.as_bytes(), &name) {
                if host.diagnostics.len() != before {
                    host.project.commit_declarations(definitions);

                    return Ok(Some(std::mem::take(&mut host.diagnostics)));
                }

                return Err(error);
            }
        }
    }

    Ok(None)
}

fn add_analysis_timeouts(host: &mut Host<'_>, timeouts: BTreeSet<String>) -> io::Result<()> {
    for name in timeouts {
        let location = host.location(&name, [0; 4])?;

        if !host
            .project
            .includes(&location.module.source, Service::Analyze)?
        {
            continue;
        }

        host.diagnostics.push(Diagnostic {
            location,
            error: true,
            message: "Luau analysis timed out".into(),
            related: None,
        });
    }

    Ok(())
}

fn collect_analysis_results(
    host: &mut Host<'_>,
    checker: &mut Checker,
    entries: BTreeSet<String>,
) -> io::Result<BTreeSet<String>> {
    let mut timeouts = BTreeSet::new();

    for name in entries {
        if is_declaration_source(&host.modules[&name].module.source) {
            continue;
        }

        timeouts.extend(checker.check(host, Path::new(&name))?);
    }

    let mut loaded = host
        .modules
        .iter()
        .filter(|(_, state)| state.source.is_some())
        .map(|(name, _)| name.clone())
        .collect::<Vec<_>>();

    loaded.sort();

    for name in loaded {
        timeouts.extend(checker.result(host, Path::new(&name))?);
    }

    Ok(timeouts)
}

fn check_environment(
    project: &mut Project,
    environment: Environment,
) -> io::Result<Vec<Diagnostic>> {
    let declarations = project.declarations(&environment.definitions)?;

    let mut host = Host {
        project,
        resolver: Resolver::new(),
        identities: HashMap::new(),
        modules: HashMap::new(),
        diagnostics: Vec::new(),
        open: None,
    };

    let mut entries = BTreeSet::new();

    for module in environment.entries {
        entries.insert(host.intern(module)?);
    }

    for name in &entries {
        let module = host.modules[name].module.clone();

        if is_declaration_source(&module.source) {
            for (package, path) in &environment.definitions {
                if crate::absolute(Path::new(path))? == module.source {
                    host.alias(module.clone(), package)?;
                }
            }
        }
    }

    let mut checker = Checker::new(&CheckerOptions {
        run_lint_checks: 1,
        ..CheckerOptions::default()
    })?;

    let mut registered = false;

    for (package, definition) in &declarations {
        let mut source = Rc::clone(&definition.source);

        for (configured_package, location) in &environment.definitions {
            if configured_package == package {
                let path = crate::absolute(Path::new(location))?;

                if entries
                    .iter()
                    .any(|name| host.modules[name].module.source == path)
                {
                    source = host.project.source(&path)?;
                }
            }
        }

        let before = host.diagnostics.len();

        if let Err(error) = checker.load_definition(&mut host, source.as_bytes(), package) {
            if host.diagnostics.len() != before {
                host.project.commit_declarations(&environment.definitions);

                return Ok(host.diagnostics);
            }

            return Err(error);
        }
    }

    if let Some(diagnostics) =
        load_entry_declarations(&mut host, &mut checker, &entries, &environment.definitions)?
    {
        return Ok(diagnostics);
    }

    for (_, definition) in declarations {
        registered |= environment.settings.enabled && definition.register(&mut checker)?;
    }

    if registered && let Some(map) = environment.map {
        map.register(&mut checker, |module| host.intern(module))?;
    }

    checker.freeze()?;
    host.project.commit_declarations(&environment.definitions);
    let timeouts = collect_analysis_results(&mut host, &mut checker, entries)?;
    add_analysis_timeouts(&mut host, timeouts)?;

    Ok(host.diagnostics)
}

fn is_declaration_source(path: &Path) -> bool {
    path.file_name()
        .is_some_and(|name| name.to_string_lossy().ends_with(".d.luau"))
}

/// Persistent editor analysis. Keep this value on its owning worker thread.
#[derive(Default)]
pub struct Editor {
    project: Project,
    open: BTreeSet<PathBuf>,
    sessions: Vec<EditorSession>,
}

struct EditorSession {
    settings: RobloxSettings,
    definitions: Vec<(String, String)>,
    map: Option<Rc<Sourcemap>>,
    declaration_paths: BTreeSet<PathBuf>,
    checker: Checker,
    resolver: Resolver,
    identities: HashMap<Module, String>,
    modules: HashMap<String, LoadedModule>,
}

impl EditorSession {
    fn with_host<T>(
        &mut self,
        project: &mut Project,
        open: &BTreeSet<PathBuf>,
        operation: impl FnOnce(&mut Checker, &mut Host<'_>) -> io::Result<T>,
    ) -> io::Result<T> {
        let mut host = Host {
            project,
            resolver: std::mem::take(&mut self.resolver),
            identities: std::mem::take(&mut self.identities),
            modules: std::mem::take(&mut self.modules),
            diagnostics: Vec::new(),
            open: Some(open),
        };

        let result = operation(&mut self.checker, &mut host);
        self.resolver = host.resolver;
        self.identities = host.identities;
        self.modules = host.modules;

        result
    }
}

impl Editor {
    /// Updates or closes an unsaved document and invalidates its dependents.
    ///
    /// # Errors
    /// Returns invalid-path or native invalidation errors.
    pub fn set_source(&mut self, path: &Path, text: Option<&str>) -> io::Result<()> {
        let path = crate::absolute(path)?;
        self.project.set_source(&path, text)?;

        if is_declaration_source(&path) {
            self.sessions.clear();
        }

        if text.is_some() {
            self.open.insert(path.clone());
        } else {
            self.open.remove(&path);
        }

        for session in &mut self.sessions {
            session.resolver = Resolver::new();

            for (name, state) in &mut session.modules {
                if state.module.source == path {
                    session.checker.mark_dirty(Path::new(name))?;
                    state.source = None;
                    state.expressions = None;
                    state.line_starts.clear();
                }
            }
        }

        Ok(())
    }

    /// Reloads disk/configuration state while preserving unsaved buffers.
    pub fn refresh(&mut self) {
        self.sessions.clear();
        self.project.refresh();
    }

    /// Checks open files incrementally. Closed dependencies supply types, not lint diagnostics.
    ///
    /// # Errors
    /// Returns source, configuration, or analysis errors.
    pub fn check(&mut self) -> io::Result<Vec<Diagnostic>> {
        self.check_roots(&[])
    }

    /// Checks additional workspace roots for navigation without enabling their lint passes.
    ///
    /// # Errors
    /// Returns source, configuration, or analysis errors.
    pub fn index(&mut self, paths: &[PathBuf]) -> io::Result<()> {
        self.check_roots(paths).map(drop)
    }

    /// Checks workspace roots with lint diagnostics and cancellable per-module progress.
    ///
    /// # Errors
    /// Returns analysis errors or `Interrupted` when progress returns false.
    pub fn check_workspace(
        &mut self,
        paths: &[PathBuf],
        progress: &mut dyn FnMut(usize, usize) -> bool,
    ) -> io::Result<Vec<Diagnostic>> {
        let open = self.open.clone();
        self.open.extend(paths.iter().cloned());
        let result = self.check_roots_progress(paths, progress);
        self.open = open;

        result
    }

    fn check_roots(&mut self, additional: &[PathBuf]) -> io::Result<Vec<Diagnostic>> {
        self.check_roots_progress(additional, &mut |_, _| true)
    }

    fn check_roots_progress(
        &mut self,
        additional: &[PathBuf],
        progress: &mut dyn FnMut(usize, usize) -> bool,
    ) -> io::Result<Vec<Diagnostic>> {
        if !progress(0, 0) {
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "request cancelled",
            ));
        }

        let paths = self
            .open
            .iter()
            .cloned()
            .chain(additional.iter().cloned())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();

        let mut environments = environments(&mut self.project, &paths, Service::Lsp)?;

        environments.sort_by(|a, b| {
            a.map
                .as_ref()
                .map(|map| &map.path)
                .cmp(&b.map.as_ref().map(|map| &map.path))
        });

        let total = environments
            .iter()
            .map(|environment| environment.entries.len())
            .sum();

        let mut completed = 0;
        let mut diagnostics = Vec::new();

        for environment in environments {
            diagnostics.extend(self.check_environment_progress(
                environment,
                &mut completed,
                total,
                progress,
            )?);

            if !progress(completed, total) {
                return Err(io::Error::new(
                    io::ErrorKind::Interrupted,
                    "request cancelled",
                ));
            }
        }

        Ok(diagnostics)
    }

    fn check_environment_progress(
        &mut self,
        environment: Environment,
        completed: &mut usize,
        total: usize,
        progress: &mut dyn FnMut(usize, usize) -> bool,
    ) -> io::Result<Vec<Diagnostic>> {
        let declaration_paths = environment
            .entries
            .iter()
            .filter(|module| is_declaration_source(&module.source))
            .map(|module| module.source.clone())
            .collect::<BTreeSet<_>>();

        let index = self.sessions.iter().position(|session| {
            session.settings == environment.settings
                && session.definitions == environment.definitions
                && session.declaration_paths == declaration_paths
                && session.map.as_ref().map(|map| &map.path)
                    == environment.map.as_ref().map(|map| &map.path)
        });

        let (index, mut diagnostics) = if let Some(index) = index {
            (index, Vec::new())
        } else {
            self.create_session(
                environment.settings.clone(),
                &environment.definitions,
                environment.map.clone(),
                environment
                    .entries
                    .iter()
                    .filter(|module| is_declaration_source(&module.source))
                    .cloned()
                    .collect(),
            )?
        };

        let found =
            self.sessions[index].with_host(&mut self.project, &self.open, |checker, host| {
                let mut timeouts = BTreeSet::new();

                let entries = environment
                    .entries
                    .into_iter()
                    .map(|module| host.intern(module))
                    .collect::<io::Result<Vec<_>>>()?;

                for name in &entries {
                    if !progress(*completed, total) {
                        return Err(io::Error::new(
                            io::ErrorKind::Interrupted,
                            "request cancelled",
                        ));
                    }

                    if !is_declaration_source(&host.modules[name].module.source) {
                        timeouts.extend(checker.check(host, Path::new(name))?);
                    }

                    *completed += 1;
                }

                for name in &entries {
                    if !is_declaration_source(&host.modules[name].module.source) {
                        timeouts.extend(checker.result(host, Path::new(name))?);
                    }
                }

                for name in timeouts {
                    let location = host.location(&name, [0; 4])?;

                    if host
                        .open
                        .is_some_and(|open| open.contains(&location.module.source))
                    {
                        host.diagnostics.push(Diagnostic {
                            location,
                            error: true,
                            message: "Luau analysis timed out".into(),
                            related: None,
                        });
                    }
                }

                Ok(std::mem::take(&mut host.diagnostics))
            })?;

        diagnostics.extend(found);

        Ok(diagnostics)
    }

    fn create_session(
        &mut self,
        settings: RobloxSettings,
        definitions: &[(String, String)],
        map: Option<Rc<Sourcemap>>,
        declaration_modules: Vec<Module>,
    ) -> io::Result<(usize, Vec<Diagnostic>)> {
        let declarations = self.project.declarations(definitions)?;
        let mut declaration_diagnostics = Vec::new();

        let declaration_paths: BTreeSet<_> = declaration_modules
            .iter()
            .map(|module| module.source.clone())
            .collect();

        let mut session = EditorSession {
            settings,
            definitions: definitions.to_vec(),
            map,
            declaration_paths: declaration_paths.clone(),
            checker: Checker::new(&CheckerOptions {
                retain_full_type_graphs: 1,
                run_lint_checks: 1,
                ..CheckerOptions::default()
            })?,
            resolver: Resolver::new(),
            identities: HashMap::new(),
            modules: HashMap::new(),
        };

        let map = session.map.clone();
        let roblox = session.settings.enabled;

        session.with_host(&mut self.project, &self.open, |checker, host| {
            let mut registered = false;

            for (package, location) in definitions {
                let path = crate::absolute(Path::new(location))?;

                if declaration_paths.contains(&path) {
                    host.alias(
                        Module {
                            path: crate::resolve::module_path(&path),
                            source: path,
                            instance: None,
                        },
                        package,
                    )?;
                }
            }

            for (package, definition) in &declarations {
                let source = definitions
                    .iter()
                    .find(|(name, location)| {
                        name == package
                            && crate::absolute(Path::new(location))
                                .is_ok_and(|path| declaration_paths.contains(&path))
                    })
                    .map(|(_, location)| crate::absolute(Path::new(location)))
                    .transpose()?
                    .map(|path| host.project.source(&path))
                    .transpose()?
                    .unwrap_or_else(|| Rc::clone(&definition.source));

                let before = host.diagnostics.len();

                if let Err(error) = checker.load_definition(host, source.as_bytes(), package) {
                    if error.to_string() == "definition loading failed"
                        && host.diagnostics.len() > before
                    {
                        declaration_diagnostics.append(&mut host.diagnostics);
                    } else {
                        return Err(error);
                    }
                }
            }

            for module in declaration_modules {
                if definitions.iter().any(|(_, location)| {
                    crate::absolute(Path::new(location)).is_ok_and(|path| path == module.source)
                }) {
                    continue;
                }

                let name = host.intern(module.clone())?;
                let source = host.project.source(&module.source)?;
                let before = host.diagnostics.len();

                if let Err(error) = checker.load_definition(host, source.as_bytes(), &name) {
                    if error.to_string() == "definition loading failed"
                        && host.diagnostics.len() > before
                    {
                        declaration_diagnostics.append(&mut host.diagnostics);
                    } else {
                        return Err(error);
                    }
                }
            }

            for (_, definition) in declarations {
                registered |= roblox && definition.register(checker)?;
            }

            if registered && let Some(map) = map {
                map.register(checker, |module| host.intern(module))?;
            }

            checker.freeze()
        })?;

        self.project.commit_declarations(&session.definitions);
        self.sessions.push(session);

        Ok((self.sessions.len() - 1, declaration_diagnostics))
    }

    /// Runs an editor query against a checked module in its first place context.
    ///
    /// # Errors
    /// Returns an error for unavailable modules or failed native queries.
    pub fn query<T>(
        &mut self,
        path: &Path,
        operation: impl FnOnce(&mut Checker, &mut dyn Callbacks, &str) -> io::Result<T>,
    ) -> io::Result<T> {
        let path = crate::absolute(path)?;

        for session in &mut self.sessions {
            let name = session
                .identities
                .iter()
                .filter(|(module, _)| module.source == path)
                .map(|(_, name)| name)
                .min()
                .cloned();

            if let Some(name) = name {
                return session.with_host(&mut self.project, &self.open, |checker, host| {
                    operation(checker, host, &name)
                });
            }
        }

        Err(invalid(format!("{} has no checked module", path.display())))
    }

    /// Runs a query in every checked place and instance context of a source.
    ///
    /// # Errors
    /// Returns an error for unavailable modules or failed native queries.
    pub fn query_all<T>(
        &mut self,
        path: &Path,
        mut operation: impl FnMut(&mut Checker, &mut dyn Callbacks, &str) -> io::Result<T>,
    ) -> io::Result<Vec<T>> {
        let path = crate::absolute(path)?;
        let mut results = Vec::new();

        for session in &mut self.sessions {
            let mut names: Vec<_> = session
                .identities
                .iter()
                .filter(|(module, _)| module.source == path)
                .map(|(_, name)| name.clone())
                .collect();

            names.sort_unstable();
            names.dedup();

            for name in names {
                results.push(session.with_host(
                    &mut self.project,
                    &self.open,
                    |checker, host| operation(checker, host, &name),
                )?);
            }
        }

        if results.is_empty() {
            return Err(invalid(format!("{} has no checked module", path.display())));
        }

        Ok(results)
    }

    /// Returns native identities and source paths for all modules loaded in place contexts.
    #[must_use]
    pub fn module_identities(&self) -> Vec<(String, PathBuf)> {
        let mut identities = self
            .sessions
            .iter()
            .flat_map(|session| {
                session
                    .identities
                    .iter()
                    .map(|(module, name)| (name.clone(), module.source.clone()))
            })
            .collect::<Vec<_>>();

        identities.sort_unstable();
        identities.dedup();

        identities
    }

    /// Resolves a native identity to its physical source file.
    ///
    /// # Errors
    /// Returns an error for non-source identities such as built-in declarations.
    pub fn source_path(&self, name: &str) -> io::Result<PathBuf> {
        self.sessions
            .iter()
            .find_map(|session| session.modules.get(name))
            .map(|state| state.module.source.clone())
            .ok_or_else(|| invalid(format!("unknown source identity {name}")))
    }

    /// Reads the current overlay or disk source.
    ///
    /// # Errors
    /// Returns source-loading errors.
    pub fn source(&mut self, path: &Path) -> io::Result<Rc<str>> {
        self.project.source(path)
    }

    /// Lists services from the editor's cached platform declarations.
    ///
    /// # Errors
    /// Returns configuration or declaration-loading errors.
    pub fn services(&mut self, path: &Path) -> io::Result<Vec<String>> {
        self.project.services(path)
    }

    /// Loads merged documentation for a source's platform configuration.
    ///
    /// # Errors
    /// Returns documentation-loading errors.
    pub fn documentation(
        &mut self,
        path: &Path,
        symbol: &str,
    ) -> io::Result<Option<Rc<serde_json::Value>>> {
        self.project.documentation(path, symbol)
    }
}
