use std::{
    collections::{BTreeMap, BTreeSet},
    io,
    sync::Arc,
    time::Instant,
};

use instar_analysis::{
    Completion, Location, Options,
    process::{Outcome, Process},
};

use super::{
    hierarchy::{Hierarchy, HierarchyKind},
    host::Host,
    references::Reference,
    result::{Hover, LintResult},
    signatures::Signatures,
    source::{
        Binding, Class, Declaration, Definition, Link, Site, Source, lines, validate_namespace,
    },
    suggestions::Suggestions,
};

use crate::{
    Configuration, flags,
    protocol::{Operation, Request, Response},
};

/// A host-resolved Luau frontend whose isolated worker starts on the first request.
#[derive(Default)]
pub struct Frontend {
    process: Option<Process<Request, Response>>,
    host: Host,
    flags: BTreeMap<String, flags::Value>,
}

impl Frontend {
    /// Installs an immutable source revision, effective native settings and host resolutions.
    ///
    /// # Errors
    /// Rejects invalid module names, source ranges and native allocation failures.
    pub fn insert(
        &mut self,
        name: &str,
        text: impl Into<Arc<str>>,
        revision: u64,
        configuration: &Configuration,
        sites: &[Site],
    ) -> io::Result<()> {
        let text = text.into();

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

        self.host.remove(name);

        self.host.sources.insert(
            name.to_owned(),
            Source {
                configuration: snapshot,
                text: Arc::clone(&text),
                revision,
                lines: lines(&text).into(),
                sites: sites.to_vec(),
                definitions: Vec::new(),
                environment: None,
                flags: BTreeMap::new(),
            },
        );

        Ok(())
    }

    /// Installs declarations and atomically advances shared revisions across their consumers.
    ///
    /// # Errors
    /// Rejects unavailable modules, conflicting snapshots, stale revisions and invalid identities.
    pub fn definitions(&mut self, name: &str, definitions: &[Definition]) -> io::Result<()> {
        let current = self
            .host
            .sources
            .get(name)
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "module is unavailable"))?;

        let mut names = BTreeSet::new();

        for definition in definitions {
            validate_namespace(&definition.namespace)?;

            if definition.name.is_empty()
                || definition.name.contains('\0')
                || !names.insert(&definition.name)
                || self.host.sources.contains_key(&definition.name)
                || self
                    .host
                    .declarations
                    .get(&definition.name)
                    .is_some_and(|existing| {
                        existing.revision > definition.revision
                            || (existing.revision == definition.revision
                                && existing.text != definition.text)
                            || (existing.revision < definition.revision
                                && !current
                                    .definitions
                                    .iter()
                                    .any(|binding| binding.name == definition.name))
                    })
            {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "invalid declaration identity",
                ));
            }
        }

        self.host.detach(name);

        let mut bindings = Vec::with_capacity(definitions.len());

        for definition in definitions {
            let declaration = self
                .host
                .declarations
                .entry(definition.name.clone())
                .or_insert_with(|| Declaration {
                    revision: definition.revision,
                    text: Arc::clone(&definition.text),
                    lines: lines(&definition.text).into(),
                    consumers: BTreeSet::new(),
                });

            if declaration.revision != definition.revision {
                declaration.revision = definition.revision;
                declaration.text = Arc::clone(&definition.text);
                declaration.lines = lines(&definition.text).into();
            }

            declaration.consumers.insert(name.to_owned());

            bindings.push(Binding {
                name: definition.name.clone(),
                namespace: definition.namespace.clone(),
            });
        }

        self.host
            .sources
            .get_mut(name)
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "module is unavailable"))?
            .definitions = bindings;

        Ok(())
    }

    /// Installs process-wide flag overrides for an inserted module.
    ///
    /// # Errors
    /// Rejects unavailable modules.
    pub fn flags(&mut self, name: &str, flags: &flags::Overrides) -> io::Result<()> {
        let source = self
            .host
            .sources
            .get_mut(name)
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "module is unavailable"))?;

        source.flags.clone_from(&flags.0);

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

        self.host.environment(name, classes)
    }

    /// Removes changed module revisions from the host snapshot used by subsequent requests.
    pub fn invalidate(&mut self, names: &[String]) {
        for name in names {
            self.host.remove(name);
        }
    }

    /// Parses a reachable host graph and validates static requests against native AST ranges.
    ///
    /// # Errors
    /// Returns missing host targets, worker startup, parsing or host/native agreement failures.
    pub fn parse(&mut self, entry: &str, options: &Options) -> io::Result<Vec<Link>> {
        let started = Instant::now();
        let entries = [entry.to_owned()];
        let names = self.names(&entries)?;

        match self.request(&entries, &names, Operation::Parse, options, started)? {
            Outcome::Response(Response::Parsed(links)) => Ok(links),

            Outcome::Interrupted(reason) => Err(reason.error()),

            Outcome::Response(_) => Err(io::Error::other("unexpected native worker response")),
        }
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

            Outcome::Interrupted(reason) => Err(reason.error()),

            Outcome::Response(_) => Err(io::Error::other("unexpected native worker response")),
        }
    }

    /// Returns the native type and documentation reference at a UTF-8 byte offset.
    ///
    /// # Errors
    /// Returns invalid positions, incomplete analysis, stale results, or worker failures.
    pub fn hover(
        &mut self,
        entry: &str,
        offset: usize,
        options: &Options,
    ) -> io::Result<Option<Hover>> {
        let started = Instant::now();

        let [line, column] = self.position(entry, offset)?;
        let entries = [entry.to_owned()];
        let modules = self.names(&entries)?;

        match self.request(
            &entries,
            &modules,
            Operation::Hover { line, column },
            options,
            started,
        )? {
            Outcome::Response(Response::Hover(result)) => {
                if let Some(hover) = &result {
                    self.validate_location(&hover.location)?;

                    if hover.location.module != entry {
                        return Err(io::Error::other("invalid semantic source agreement"));
                    }
                }

                Ok(result)
            }

            Outcome::Interrupted(reason) => Err(reason.error()),
            Outcome::Response(_) => Err(io::Error::other("unexpected native worker response")),
        }
    }

    /// Resolves declarations or type declarations at a UTF-8 byte offset.
    ///
    /// # Errors
    /// Returns invalid positions, incomplete analysis, stale locations, or worker failures.
    pub fn definition(
        &mut self,
        entry: &str,
        offset: usize,
        type_definition: bool,
        options: &Options,
    ) -> io::Result<Vec<Location<String>>> {
        let started = Instant::now();
        let [line, column] = self.position(entry, offset)?;
        let entries = [entry.to_owned()];
        let modules = self.names(&entries)?;

        match self.request(
            &entries,
            &modules,
            Operation::Definition {
                line,
                column,
                type_definition,
            },
            options,
            started,
        )? {
            Outcome::Response(Response::Locations(locations)) => {
                for location in &locations {
                    self.validate_location(location)?;
                }

                Ok(locations)
            }

            Outcome::Interrupted(reason) => Err(reason.error()),
            Outcome::Response(_) => Err(io::Error::other("unexpected native worker response")),
        }
    }

    /// Returns native contextual completions at a UTF-8 byte offset.
    ///
    /// # Errors
    /// Returns invalid positions, incomplete analysis, stale locations, or worker failures.
    pub fn suggest(
        &mut self,
        entry: &str,
        offset: usize,
        options: &Options,
    ) -> io::Result<Suggestions> {
        let started = Instant::now();
        let [line, column] = self.position(entry, offset)?;
        let entries = [entry.to_owned()];
        let modules = self.names(&entries)?;

        match self.request(
            &entries,
            &modules,
            Operation::Suggest { line, column },
            options,
            started,
        )? {
            Outcome::Response(Response::Suggestions(suggestions)) => {
                self.validate_location(&suggestions.location)?;

                if suggestions.location.module != entry {
                    return Err(io::Error::other("invalid semantic source agreement"));
                }

                if let Some(range) = suggestions.index {
                    self.validate_location(&Location {
                        module: entry.to_owned(),
                        revision: suggestions.location.revision,
                        range,
                    })?;

                    if range[1] - range[0] != 1 || range[1] > suggestions.location.range[0] {
                        return Err(io::Error::other("invalid semantic member operator"));
                    }
                }

                Ok(suggestions)
            }

            Outcome::Interrupted(reason) => Err(reason.error()),
            Outcome::Response(_) => Err(io::Error::other("unexpected native worker response")),
        }
    }

    /// Returns native callable signatures and the selected argument at a UTF-8 byte offset.
    ///
    /// # Errors
    /// Returns invalid positions, incomplete analysis, stale locations, or worker failures.
    pub fn signatures(
        &mut self,
        entry: &str,
        offset: usize,
        options: &Options,
    ) -> io::Result<Option<Signatures>> {
        let started = Instant::now();
        let [line, column] = self.position(entry, offset)?;
        let entries = [entry.to_owned()];
        let modules = self.names(&entries)?;

        match self.request(
            &entries,
            &modules,
            Operation::Signatures { line, column },
            options,
            started,
        )? {
            Outcome::Response(Response::Signatures(result)) => {
                if let Some(signatures) = &result {
                    self.validate_location(&signatures.location)?;

                    if signatures.location.module != entry {
                        return Err(io::Error::other("invalid semantic source agreement"));
                    }
                }

                Ok(result)
            }

            Outcome::Interrupted(reason) => Err(reason.error()),
            Outcome::Response(_) => Err(io::Error::other("unexpected native worker response")),
        }
    }

    /// Returns native reference identities from an entry and its reachable sources.
    ///
    /// # Errors
    /// Returns incomplete analysis, source disagreement, interruption, or worker failures.
    pub fn references(&mut self, entry: &str, options: &Options) -> io::Result<Vec<Reference>> {
        let started = Instant::now();
        let entries = [entry.to_owned()];
        let modules = self.names(&entries)?;

        match self.request(&entries, &modules, Operation::References, options, started)? {
            Outcome::Response(Response::References(references)) => {
                for reference in &references {
                    options.check(started)?;

                    if !modules.contains(&reference.location.module) {
                        return Err(io::Error::other(
                            "native reference is outside the reachable graph",
                        ));
                    }

                    self.validate_location(&reference.location)?;

                    for target in &reference.targets {
                        self.validate_location(target)?;
                    }
                }

                Ok(references)
            }

            Outcome::Interrupted(reason) => Err(reason.error()),
            Outcome::Response(_) => Err(io::Error::other("unexpected native worker response")),
        }
    }

    /// Returns native function calls and nominal inheritance in the reachable graph.
    ///
    /// An optional UTF-8 byte offset selects the native declaration at that position.
    ///
    /// # Errors
    /// Returns invalid positions, incomplete analysis, invalid relationships, source
    /// disagreement, interruption, or worker failures.
    pub fn hierarchy(
        &mut self,
        entry: &str,
        offset: Option<usize>,
        options: &Options,
    ) -> io::Result<Hierarchy> {
        let started = Instant::now();

        let position = offset
            .map(|offset| self.position(entry, offset))
            .transpose()?;

        let entries = [entry.to_owned()];
        let modules = self.names(&entries)?;

        match self.request(
            &entries,
            &modules,
            Operation::Hierarchy { position },
            options,
            started,
        )? {
            Outcome::Response(Response::Hierarchy(hierarchy)) => {
                hierarchy.validate()?;

                for item in &hierarchy.items {
                    options.check(started)?;
                    self.validate_location(&item.location)?;

                    self.validate_location(&Location {
                        module: item.location.module.clone(),
                        revision: item.location.revision,
                        range: item.selection,
                    })?;

                    if item.kind != HierarchyKind::Class && !modules.contains(&item.location.module)
                    {
                        return Err(io::Error::other(
                            "native hierarchy is outside the reachable graph",
                        ));
                    }
                }

                for call in &hierarchy.calls {
                    options.check(started)?;
                    self.validate_location(&call.location)?;
                    let caller = &hierarchy.items[call.caller].location;

                    if call.location.module != caller.module
                        || call.location.revision != caller.revision
                        || call.location.range[0] < caller.range[0]
                        || call.location.range[1] > caller.range[1]
                    {
                        return Err(io::Error::other(
                            "native call is outside its containing scope",
                        ));
                    }
                }

                options.check(started)?;

                Ok(hierarchy)
            }

            Outcome::Interrupted(reason) => Err(reason.error()),
            Outcome::Response(_) => Err(io::Error::other("unexpected native worker response")),
        }
    }

    /// Checks selected entries with their complete host-resolved dependency graph.
    ///
    /// The worker is terminated if the deadline expires or cancellation is requested.
    ///
    /// # Errors
    /// Returns missing host targets, host/native disagreement or worker failures.
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
    /// Returns missing host targets, host/native disagreement or worker failures.
    pub fn lint(&mut self, entries: &[String], options: &Options) -> io::Result<LintResult> {
        self.lint_inner(entries, &[], options)
    }

    /// Runs native warnings and extracts inferred-any facts for selected source modules.
    ///
    /// Nocheck semantic selections produce an explicit incomplete result.
    ///
    /// # Errors
    /// Rejects semantic selections outside the reachable graph, missing host targets,
    /// host/native disagreement or worker failures.
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
            self.process = Some(Process::start("analysis")?);
        }

        let request = Request {
            sources: self.host.sources.clone(),
            declarations: self.host.declarations.clone(),
            environments: self.host.environments.clone(),
            entries: entries.to_vec(),
            modules: modules.to_vec(),
            operation,
            timeout: options.timeout.saturating_sub(started.elapsed()),
            flags,
            progress: options.progress.is_some(),
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

    fn position(&self, entry: &str, offset: usize) -> io::Result<[u32; 2]> {
        let source = self
            .host
            .sources
            .get(entry)
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "module is unavailable"))?;

        if !source.text.is_char_boundary(offset) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid semantic position",
            ));
        }

        let line = source.lines.partition_point(|start| *start <= offset) - 1;
        let column = offset - source.lines[line];

        Ok([
            u32::try_from(line).map_err(io::Error::other)?,
            u32::try_from(column).map_err(io::Error::other)?,
        ])
    }

    fn validate_location(&self, location: &Location<String>) -> io::Result<()> {
        let (revision, text) = self
            .host
            .sources
            .get(&location.module)
            .map(|source| (source.revision, source.text.as_ref()))
            .or_else(|| {
                self.host
                    .declarations
                    .get(&location.module)
                    .map(|source| (source.revision, source.text.as_ref()))
            })
            .ok_or_else(|| io::Error::other("semantic source is unavailable"))?;

        let [start, end] = location.range;

        if location.revision != revision
            || start > end
            || !text.is_char_boundary(start)
            || !text.is_char_boundary(end)
        {
            return Err(io::Error::other("invalid semantic source agreement"));
        }

        Ok(())
    }
}
