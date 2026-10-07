use std::{
    collections::{BTreeMap, BTreeSet},
    io,
    time::Instant,
};

use instar_analysis::{
    Completion, Options,
    process::{Outcome, Process},
};

use super::{
    host::Host,
    result::LintResult,
    source::{Class, Definition, Link, Site, Source, validate_namespace},
};

use crate::{
    Configuration, flags,
    protocol::{Operation, Request, Response},
};

/// A native Luau Frontend whose resolution policy belongs exclusively to its host.
pub struct Frontend {
    process: Option<Process<Request, Response>>,
    host: Host,
    flags: BTreeMap<String, flags::Value>,
}

impl Frontend {
    /// Creates a host snapshot store for an isolated native frontend.
    ///
    /// # Errors
    /// Returns worker startup failures.
    pub fn new() -> io::Result<Self> {
        Ok(Self {
            process: Some(Process::start("worker")?),
            host: Host::default(),
            flags: BTreeMap::new(),
        })
    }

    /// Installs an immutable source revision, effective native settings and host resolutions.
    ///
    /// # Errors
    /// Rejects invalid module names, source ranges and native allocation failures.
    pub fn insert(
        &mut self,
        name: &str,
        text: &str,
        revision: u64,
        configuration: &Configuration,
        sites: &[Site],
    ) -> io::Result<()> {
        if name.is_empty() || name.contains('\0') {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "module names must be nonempty and contain no NUL",
            ));
        }

        let mut calls = BTreeSet::new();

        for site in sites {
            if site.call[0] > site.argument[0]
                || site.argument[0] > site.argument[1]
                || site.argument[1] > site.call[1]
                || site.call[1] > text.len()
                || !text.is_char_boundary(site.call[0])
                || !text.is_char_boundary(site.call[1])
                || !text.is_char_boundary(site.argument[0])
                || !text.is_char_boundary(site.argument[1])
                || !calls.insert(site.call)
                || site.target.is_some() && !site.static_request
                || site
                    .target
                    .as_ref()
                    .is_some_and(|target| target.is_empty() || target.contains('\0'))
            {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "invalid host require site",
                ));
            }
        }

        let snapshot = configuration.snapshot()?;

        if self.host.sources.get(name).is_some_and(|source| {
            source.revision == revision
                && source.text == text
                && source.sites == sites
                && source.configuration == snapshot
        }) {
            return Ok(());
        }

        self.host.sources.insert(
            name.to_owned(),
            Source {
                configuration: snapshot,
                text: text.to_owned(),
                revision,
                sites: sites.to_vec(),
                definitions: Vec::new(),
                classes: Vec::new(),
                flags: BTreeMap::new(),
            },
        );

        Ok(())
    }

    /// Parses a reachable host graph and validates static requests against native AST ranges.
    ///
    /// # Errors
    /// Returns missing host targets, native parsing failures or host/native agreement errors.
    pub fn parse(&mut self, entry: &str, options: &Options) -> io::Result<Vec<Link>> {
        let started = Instant::now();
        let entries = [entry.to_owned()];
        let names = self.names(&entries)?;

        match self.request(&entries, &names, Operation::Parse, options, started)? {
            Outcome::Response(Response::Parsed(links)) => Ok(links),

            Outcome::Interrupted(reason) => Err(io::Error::new(
                if reason == instar_analysis::Reason::Timeout {
                    io::ErrorKind::TimedOut
                } else {
                    io::ErrorKind::Interrupted
                },
                format!("native parsing interrupted: {reason:?}"),
            )),

            Outcome::Response(_) => Err(io::Error::other("unexpected native worker response")),
        }
    }

    /// Installs declaration snapshots for an already inserted module.
    ///
    /// # Errors
    /// Rejects unavailable modules or invalid declaration identities.
    pub fn definitions(&mut self, name: &str, definitions: &[Definition]) -> io::Result<()> {
        let mut names = BTreeSet::new();

        for definition in definitions {
            validate_namespace(&definition.namespace)?;

            if definition.name.is_empty()
                || definition.name.contains('\0')
                || !names.insert(&definition.name)
                || self.host.sources.contains_key(&definition.name)
                || self
                    .host
                    .sources
                    .iter()
                    .filter(|(module, _)| module.as_str() != name)
                    .flat_map(|(_, source)| &source.definitions)
                    .any(|existing| {
                        existing.name == definition.name
                            && (existing.revision != definition.revision
                                || existing.text != definition.text)
                    })
            {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "invalid declaration identity",
                ));
            }
        }

        let source = self
            .host
            .sources
            .get_mut(name)
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "module is unavailable"))?;

        source.definitions = definitions.to_vec();

        Ok(())
    }

    /// Installs process-wide flag overrides for an inserted module.
    ///
    /// # Errors
    /// Rejects invalid flag names, types, and unavailable modules.
    pub fn flags(&mut self, name: &str, flags: &BTreeMap<String, flags::Value>) -> io::Result<()> {
        let flags = flags::normalize(flags)?;

        let source = self
            .host
            .sources
            .get_mut(name)
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "module is unavailable"))?;

        source.flags = flags;

        Ok(())
    }

    /// Installs Roblox class capabilities for a module environment.
    ///
    /// # Errors
    /// Rejects unknown modules and duplicate or invalid class names.
    pub fn classes(&mut self, name: &str, classes: &[Class]) -> io::Result<()> {
        let mut names = BTreeSet::new();

        for class in classes {
            if class.name.is_empty() || class.name.contains('\0') || !names.insert(&class.name) {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "invalid Roblox class metadata",
                ));
            }

            let mut properties = BTreeSet::new();

            if class.properties.iter().any(|property| {
                property.name.is_empty()
                    || property.name.contains('\0')
                    || !properties.insert(&property.name)
            }) {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "invalid Roblox property metadata",
                ));
            }
        }

        let source = self
            .host
            .sources
            .get_mut(name)
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "module is unavailable"))?;

        source.classes = classes.to_vec();

        Ok(())
    }

    /// Resolves a global or exported type path to its native documentation identifier.
    ///
    /// # Errors
    /// Returns incomplete analysis, invalid environments, or worker failures.
    pub fn documentation(
        &mut self,
        entry: &str,
        symbol: &str,
        options: &Options,
    ) -> io::Result<Option<String>> {
        let started = Instant::now();
        let entries = [entry.to_owned()];
        let modules = self.names(&entries)?;

        match self.request(
            &entries,
            &modules,
            Operation::Documentation(symbol.to_owned()),
            options,
            started,
        )? {
            Outcome::Response(Response::Documentation(symbol)) => Ok(symbol),

            Outcome::Interrupted(reason) => Err(io::Error::other(format!(
                "documentation analysis interrupted: {reason:?}"
            ))),

            Outcome::Response(_) => Err(io::Error::other("unexpected native worker response")),
        }
    }

    /// Checks selected entries with their complete host-resolved dependency graph.
    ///
    /// The worker is terminated if the deadline expires or cancellation is requested.
    ///
    /// # Errors
    /// Returns missing host targets, host/native disagreement or native failures.
    pub fn check(
        &mut self,
        entries: &[String],
        options: &Options,
    ) -> io::Result<instar_analysis::Result<String>> {
        let started = Instant::now();
        let modules = self.names(entries)?;

        match self.request(entries, &modules, Operation::Check, options, started)? {
            Outcome::Response(Response::Checked(result)) => Ok(result),

            Outcome::Interrupted(reason) => Ok(instar_analysis::Result {
                modules,
                diagnostics: Vec::new(),
                completion: Completion::Incomplete(reason),
            }),

            Outcome::Response(_) => Err(io::Error::other("unexpected native worker response")),
        }
    }

    /// Runs enabled upstream warnings using the shared native analysis session.
    ///
    /// The worker is terminated if the deadline expires or cancellation is requested.
    ///
    /// # Errors
    /// Returns missing host targets, host/native disagreement or native failures.
    pub fn lint(&mut self, entries: &[String], options: &Options) -> io::Result<LintResult> {
        self.lint_inner(entries, &[], options)
    }

    /// Runs native warnings and extracts inferred-any facts for selected source modules.
    ///
    /// Nocheck semantic selections produce an explicit incomplete result.
    ///
    /// # Errors
    /// Rejects semantic selections outside the reachable graph, missing host targets,
    /// host/native disagreement or native failures.
    pub fn lint_semantic(
        &mut self,
        entries: &[String],
        semantic_modules: &[String],
        options: &Options,
    ) -> io::Result<LintResult> {
        self.lint_inner(entries, semantic_modules, options)
    }

    fn names(&self, entries: &[String]) -> io::Result<Vec<String>> {
        let mut names = BTreeSet::new();
        let mut pending = entries.to_vec();

        while let Some(name) = pending.pop() {
            if !names.insert(name.clone()) {
                continue;
            }

            let source = self.host.sources.get(&name).ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::NotFound,
                    format!("host module is unavailable: {name}"),
                )
            })?;

            pending.extend(source.sites.iter().filter_map(|site| site.target.clone()));
        }

        Ok(names.into_iter().collect())
    }

    fn lint_inner(
        &mut self,
        entries: &[String],
        semantic_modules: &[String],
        options: &Options,
    ) -> io::Result<LintResult> {
        let started = Instant::now();
        let modules = self.names(entries)?;

        if semantic_modules.iter().any(|name| !modules.contains(name)) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "semantic selections must belong to the reachable host graph",
            ));
        }

        match self.request(
            entries,
            &modules,
            Operation::Lint(semantic_modules.to_vec()),
            options,
            started,
        )? {
            Outcome::Response(Response::Linted(result)) => Ok(result),

            Outcome::Interrupted(reason) => Ok(LintResult {
                modules,
                warnings: Vec::new(),
                facts: Vec::new(),
                diagnostics: Vec::new(),
                completion: Completion::Incomplete(reason),
            }),

            Outcome::Response(_) => Err(io::Error::other("unexpected native worker response")),
        }
    }

    fn request(
        &mut self,
        entries: &[String],
        modules: &[String],
        operation: Operation,
        options: &Options,
        started: Instant,
    ) -> io::Result<Outcome<Response>> {
        if let Some(reason) = options.interrupted(started) {
            return Ok(Outcome::Interrupted(reason));
        }

        let mut effective = None;

        for name in modules {
            let source =
                self.host.sources.get(name).ok_or_else(|| {
                    io::Error::new(io::ErrorKind::NotFound, "module is unavailable")
                })?;

            if let Some((previous, flags)) = effective {
                if flags != &source.flags {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        format!("conflicting Luau flags in analysis graph: {previous} and {name}"),
                    ));
                }
            } else {
                effective = Some((name, &source.flags));
            }
        }

        let flags = effective.map_or_else(BTreeMap::new, |(_, flags)| flags.clone());

        if self.flags != flags {
            self.process.take();
            self.flags.clone_from(&flags);
        }

        if self.process.is_none() {
            self.process = Some(Process::start("worker")?);
        }

        let request = Request {
            sources: self.host.sources.clone(),
            entries: entries.to_vec(),
            modules: modules.to_vec(),
            operation,
            timeout: options.timeout.saturating_sub(started.elapsed()),
            flags,
        };

        let result = self
            .process
            .as_mut()
            .ok_or_else(|| io::Error::other("native worker is unavailable"))?
            .request(request, options, started);

        if !matches!(result, Ok(Outcome::Response(_))) {
            self.process.take();
        }

        result
    }

    /// Removes changed module revisions from host and native analysis caches.
    ///
    /// # Errors
    /// Returns native invalidation failures.
    pub fn invalidate(&mut self, names: &[String]) -> io::Result<()> {
        for name in names {
            self.host.sources.remove(name);
        }

        Ok(())
    }
}
