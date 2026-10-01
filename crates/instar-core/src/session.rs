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
    diagnostic::{Diagnostic, Location, Severity},
    filter::Service,
    graph::{self, Request},
    invalid, lint,
    project::{EffectiveConfig, Project, RobloxSettings},
    resolve::{Failure, Module, Resolver},
    roblox::Sourcemap,
};

pub(crate) struct LoadedModule {
    pub(crate) module: Module,
    pub(crate) configuration: Rc<EffectiveConfig>,
    pub(crate) source: Option<Rc<str>>,
    expressions: Option<HashMap<[usize; 2], Request>>,
    pub(crate) line_starts: Vec<usize>,
}

pub(crate) struct Host<'project> {
    pub(crate) project: &'project mut Project,
    service: Service,
    resolver: Resolver,
    identities: HashMap<Module, String>,
    pub(crate) modules: HashMap<String, LoadedModule>,
    pub(crate) diagnostics: Vec<Diagnostic>,
    open: Option<&'project BTreeSet<PathBuf>>,
    pub(crate) lint_cache: &'project mut lint::Cache,
    pub(crate) declarations: &'project [Diagnostic],
    timeouts: &'project BTreeSet<String>,
}

impl Host<'_> {
    pub(crate) fn intern(&mut self, module: Module) -> io::Result<String> {
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

    pub(crate) fn location(&self, name: &str, range: [u32; 4]) -> io::Result<Location> {
        let module = self
            .modules
            .get(name)
            .ok_or_else(|| invalid(format!("unknown source module {name}")))?;

        Ok(Location {
            module: module.module.clone(),
            range,
        })
    }

    pub(crate) fn includes(&mut self, path: &Path, service: Service) -> io::Result<bool> {
        Ok(self.open.is_none_or(|open| open.contains(path))
            && self.project.includes(path, self.service)?
            && self.project.includes(path, service)?)
    }

    pub(crate) fn selected(&mut self, service: Service) -> io::Result<Vec<String>> {
        let candidates = self
            .modules
            .iter()
            .filter(|(_, state)| {
                state.source.is_some() && !is_declaration_source(&state.module.source)
            })
            .map(|(name, state)| (name.clone(), state.module.source.clone()))
            .collect::<Vec<_>>();

        let mut selected = Vec::new();

        for (name, path) in candidates {
            if self.includes(&path, service)? {
                selected.push(name);
            }
        }

        selected.sort();

        Ok(selected)
    }

    pub(crate) fn report_timeouts(&mut self, service: Service) -> io::Result<()> {
        for name in self.timeouts {
            let location = self.location(name, [0; 4])?;

            if self.includes(&location.module.source, service)? {
                self.diagnostics.push(Diagnostic {
                    location,
                    severity: Severity::Error,
                    message: "Luau analysis timed out".into(),
                    related: None,
                });
            }
        }

        Ok(())
    }
}

impl Callbacks for Host<'_> {
    fn source<'source>(&'source mut self, name: &str) -> io::Result<Source<'source>> {
        let state = self
            .modules
            .get_mut(name)
            .ok_or_else(|| invalid(format!("unknown source module {name}")))?;

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
            .ok_or_else(|| invalid(format!("unknown source module {name}")))?;

        Ok(state.configuration.native())
    }

    fn resolve(&mut self, request: &ResolveRequest<'_>) -> io::Result<Option<String>> {
        let state = self
            .modules
            .get_mut(request.from)
            .ok_or_else(|| invalid(format!("unknown source module {}", request.from)))?;

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
            .includes(&location.module.source, self.service)?
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
            severity: if diagnostic.severity == DiagnosticSeverity::DiagnosticError {
                Severity::Error
            } else {
                Severity::Warning
            },
            message: diagnostic.message.to_owned(),
            related,
        });

        Ok(())
    }
}

pub(crate) struct Environment {
    pub(crate) settings: RobloxSettings,
    pub(crate) definitions: Vec<(String, String)>,
    pub(crate) map: Option<Rc<Sourcemap>>,
    pub(crate) entries: Vec<Module>,
}

pub(crate) fn environments(
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

pub(crate) struct Session {
    pub(crate) settings: RobloxSettings,
    pub(crate) definitions: Vec<(String, String)>,
    pub(crate) map: Option<Rc<Sourcemap>>,
    pub(crate) entries: BTreeSet<PathBuf>,
    pub(crate) declaration_paths: BTreeSet<PathBuf>,
    pub(crate) declaration_diagnostics: Vec<Diagnostic>,
    pub(crate) checker: Checker,
    pub(crate) resolver: Resolver,
    pub(crate) identities: HashMap<Module, String>,
    pub(crate) modules: HashMap<String, LoadedModule>,
    pub(crate) lint_cache: lint::Cache,
    timeouts: BTreeSet<String>,
}

impl Session {
    pub(crate) fn new(
        project: &mut Project,
        environment: &Environment,
        service: Service,
        options: &CheckerOptions,
    ) -> io::Result<Self> {
        let declarations = project.declarations(&environment.definitions)?;

        let declaration_modules = environment
            .entries
            .iter()
            .filter(|module| is_declaration_source(&module.source))
            .cloned()
            .collect::<Vec<_>>();

        let declaration_paths = declaration_modules
            .iter()
            .map(|module| module.source.clone())
            .collect::<BTreeSet<_>>();

        let mut session = Self {
            settings: environment.settings.clone(),
            definitions: environment.definitions.clone(),
            map: environment.map.clone(),
            entries: BTreeSet::new(),
            declaration_paths: declaration_paths.clone(),
            declaration_diagnostics: Vec::new(),
            checker: Checker::new(options)?,
            resolver: Resolver::new(),
            identities: HashMap::new(),
            modules: HashMap::new(),
            lint_cache: lint::Cache::default(),
            timeouts: BTreeSet::new(),
        };

        let mut diagnostics = Vec::new();

        session.with_host(project, service, None, |checker, host| {
            for (package, location) in &environment.definitions {
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
                load_declaration(
                    checker,
                    host,
                    definition.source.as_bytes(),
                    package,
                    &mut diagnostics,
                )?;
            }

            for module in declaration_modules {
                if environment.definitions.iter().any(|(_, location)| {
                    crate::absolute(Path::new(location)).is_ok_and(|path| path == module.source)
                }) {
                    continue;
                }

                let name = host.intern(module.clone())?;
                let source = host.project.source(&module.source)?;
                load_declaration(checker, host, source.as_bytes(), &name, &mut diagnostics)?;
            }

            let mut registered = false;

            for (_, definition) in declarations {
                registered |= environment.settings.enabled && definition.register(checker)?;
            }

            if registered && let Some(map) = &environment.map {
                map.register(checker, |module| host.intern(module))?;
            }

            checker.freeze()
        })?;

        project.commit_declarations(&session.definitions);
        session.declaration_diagnostics = diagnostics;

        Ok(session)
    }

    pub(crate) fn invalidate(&mut self, path: &Path, topology_changed: bool) -> io::Result<()> {
        self.lint_cache.remove(path);
        self.resolver = Resolver::new();
        self.timeouts.clear();

        if topology_changed {
            self.checker.clear_sources()?;
        }

        for (name, state) in &mut self.modules {
            if state.module.source == path {
                if !topology_changed {
                    self.checker.mark_dirty(Path::new(name))?;
                }

                state.source = None;
                state.expressions = None;
                state.line_starts.clear();
            }
        }

        Ok(())
    }

    pub(crate) fn with_host<T>(
        &mut self,
        project: &mut Project,
        service: Service,
        open: Option<&BTreeSet<PathBuf>>,
        operation: impl FnOnce(&mut Checker, &mut Host<'_>) -> io::Result<T>,
    ) -> io::Result<T> {
        let mut host = Host {
            project,
            service,
            resolver: std::mem::take(&mut self.resolver),
            identities: std::mem::take(&mut self.identities),
            modules: std::mem::take(&mut self.modules),
            diagnostics: Vec::new(),
            open,
            lint_cache: &mut self.lint_cache,
            declarations: &self.declaration_diagnostics,
            timeouts: &self.timeouts,
        };

        let result = operation(&mut self.checker, &mut host);
        self.resolver = host.resolver;
        self.identities = host.identities;
        self.modules = host.modules;

        result
    }

    pub(crate) fn prepare(
        &mut self,
        project: &mut Project,
        service: Service,
        open: Option<&BTreeSet<PathBuf>>,
        entries: &[Module],
        progress: &mut dyn FnMut(&Path) -> io::Result<()>,
    ) -> io::Result<()> {
        self.entries = entries.iter().map(|module| module.source.clone()).collect();

        self.timeouts = self.with_host(project, service, open, |checker, host| {
            let mut timeouts = BTreeSet::new();
            let mut seen = BTreeSet::new();

            for module in entries {
                progress(&module.source)?;

                if is_declaration_source(&module.source) {
                    continue;
                }

                let name = host.intern(module.clone())?;

                if seen.insert(name.clone()) {
                    timeouts.extend(checker.prepare(host, Path::new(&name))?);
                }
            }

            Ok(timeouts)
        })?;

        Ok(())
    }
}

fn load_declaration(
    checker: &mut Checker,
    host: &mut Host<'_>,
    source: &[u8],
    name: &str,
    diagnostics: &mut Vec<Diagnostic>,
) -> io::Result<()> {
    let before = host.diagnostics.len();

    if let Err(error) = checker.load_definition(host, source, name) {
        if error.get_ref().is_some_and(
            <dyn std::error::Error + Send + Sync>::is::<instar_bridge::DefinitionFailure>,
        ) && host.diagnostics.len() > before
        {
            diagnostics.append(&mut host.diagnostics);
        } else {
            return Err(error);
        }
    }

    Ok(())
}

pub(crate) fn is_declaration_source(path: &Path) -> bool {
    path.file_name()
        .is_some_and(|name| name.to_string_lossy().ends_with(".d.luau"))
}
