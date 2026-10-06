//! Native analysis snapshots adapted from host-owned resolution results.

use std::{
    collections::{BTreeMap, BTreeSet},
    io,
    time::Instant,
};

use instar_analysis::{
    Completion, Diagnostic, Location, Options,
    process::{Outcome, Process},
};

use serde::{Deserialize, Serialize};

use crate::{
    Configuration, Snapshot, boundary, flags,
    protocol::{Operation, Request, Response},
};

/// One host-extracted require site, anchored to immutable source bytes.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct Site {
    /// Entire call byte range, with exclusive end.
    pub call: [usize; 2],

    /// Argument byte range, with exclusive end.
    pub argument: [usize; 2],

    /// Whether the host statically identified this request, including failures.
    pub static_request: bool,

    /// Host-resolved target module name; `None` retains an unresolved site.
    pub target: Option<String>,
}

/// An agreed native graph edge backed by a host require site.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct Link {
    /// Opaque requiring module name.
    pub module: String,

    /// Source revision anchoring the site.
    pub revision: u64,

    /// Entire call byte range.
    pub call: [usize; 2],

    /// Require argument byte range.
    pub argument: [usize; 2],

    /// Opaque host-resolved target name.
    pub target: String,
}

/// Immutable declaration file used to establish a module's native environment.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct Definition {
    /// Opaque host declaration identity.
    pub name: String,

    /// Documentation namespace independent of the declaration's source identity.
    pub namespace: String,

    /// Source revision.
    pub revision: u64,

    /// Declaration source bytes.
    pub text: String,
}

/// A Roblox class capability supplied by the asset metadata.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct Class {
    /// Declared class name.
    pub name: String,

    /// Whether `GetService` accepts the class.
    pub service: bool,

    /// Whether Instance.new accepts the class.
    pub creatable: bool,

    /// Property access rules that require native application.
    pub properties: Vec<Property>,
}

/// Independent read and write access to a declared Roblox property.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct Property {
    /// Exact property name.
    pub name: String,

    /// Whether reading is permitted.
    pub read: bool,

    /// Whether writing is permitted.
    pub write: bool,
}

#[derive(Clone, Deserialize, Serialize)]
pub(crate) struct Source {
    pub(crate) configuration: Snapshot,
    pub(crate) text: String,
    pub(crate) revision: u64,
    pub(crate) sites: Vec<Site>,
    pub(crate) definitions: Vec<Definition>,
    pub(crate) classes: Vec<Class>,
    pub(crate) flags: BTreeMap<String, flags::Value>,
}

pub(crate) struct Cancellation(pub(crate) instar_analysis::Cancellation);

impl Cancellation {
    pub(crate) fn requested(&self) -> bool {
        self.0.requested()
    }
}

#[derive(Default)]
pub(crate) struct Host {
    pub(crate) sources: BTreeMap<String, Source>,
}

impl Host {
    pub(crate) fn read_source(&self, name: &str) -> boundary::NativeSource {
        let Some(source) = self.sources.get(name) else {
            let definition = self
                .sources
                .values()
                .flat_map(|source| &source.definitions)
                .find(|definition| definition.name == name);

            return boundary::NativeSource {
                found: definition.is_some(),
                text: definition.map_or_else(String::new, |definition| definition.text.clone()),
                revision: definition.map_or(0, |definition| definition.revision),
                sites: Vec::new(),
                definitions: Vec::new(),
                classes: Vec::new(),
            };
        };

        boundary::NativeSource {
            found: true,
            text: source.text.clone(),
            revision: source.revision,
            classes: source
                .classes
                .iter()
                .map(|class| boundary::NativeClass {
                    name: class.name.clone(),
                    service: class.service,
                    creatable: class.creatable,
                    properties: class
                        .properties
                        .iter()
                        .map(|property| boundary::NativeProperty {
                            name: property.name.clone(),
                            read: property.read,
                            write: property.write,
                        })
                        .collect(),
                })
                .collect(),
            definitions: source
                .definitions
                .iter()
                .map(|definition| boundary::NativeDefinition {
                    name: definition.name.clone(),
                    namespace: definition.namespace.clone(),
                    text: definition.text.clone(),
                    revision: definition.revision,
                })
                .collect(),
            sites: source
                .sites
                .iter()
                .map(|site| boundary::NativeSite {
                    call_start: site.call[0],
                    call_end: site.call[1],
                    argument_start: site.argument[0],
                    argument_end: site.argument[1],
                    static_request: site.static_request,
                    target: site.target.clone().unwrap_or_default(),
                })
                .collect(),
        }
    }

    pub(crate) fn resolve(&self, name: &str, start: usize, end: usize) -> String {
        self.sources
            .get(name)
            .and_then(|source| {
                source
                    .sites
                    .iter()
                    .find(|site| site.argument == [start, end])
            })
            .and_then(|site| site.target.clone())
            .unwrap_or_default()
    }
}

/// An upstream native warning anchored to a host source revision.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct Warning {
    /// Warning source range.
    pub location: Location<String>,

    /// Upstream warning code.
    pub code: i32,

    /// Upstream warning name.
    pub name: String,

    /// Upstream warning message.
    pub message: String,

    /// Whether native configuration promotes this warning to an error.
    pub fatal: bool,
}

/// An inferred semantic property used by Instar lint rules.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub enum FactKind {
    /// An unannotated local binding inferred as any.
    ImplicitAnyLocal,

    /// An unannotated function parameter inferred as any.
    ImplicitAnyParameter,
}

/// A detached semantic finding from the native type graph.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct Fact {
    /// Binding source range.
    pub location: Location<String>,

    /// Semantic property.
    pub kind: FactKind,

    /// Description of the semantic property.
    pub message: String,
}

/// Native lint output, retaining diagnostics when analysis is incomplete.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct LintResult {
    /// Selected entries and their host-resolved dependencies.
    pub modules: Vec<String>,

    /// Upstream warnings and native fatal promotion.
    pub warnings: Vec<Warning>,

    /// Requested inferred semantic properties.
    pub facts: Vec<Fact>,

    /// Syntax, declaration environment and analysis failures.
    pub diagnostics: Vec<Diagnostic<String>>,

    /// Whether all requested warning and semantic checks completed.
    pub completion: Completion,
}

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

/// Validates the namespace component of a native documentation identifier.
///
/// # Errors
/// Rejects empty namespaces and characters outside ASCII letters, digits, underscores and hyphens.
pub fn validate_namespace(namespace: &str) -> io::Result<()> {
    let valid = namespace.strip_prefix('@').is_some_and(|name| {
        !name.is_empty()
            && name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    });

    if !valid {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid documentation namespace",
        ));
    }

    Ok(())
}
