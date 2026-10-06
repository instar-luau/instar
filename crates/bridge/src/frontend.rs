//! Native analysis snapshots adapted from host-owned resolution results.

use std::{
    collections::{BTreeMap, BTreeSet},
    io,
};

use instar_analysis::{Completion, Diagnostic, Kind, Location, Options, Reason, Related};

use crate::{Configuration, Snapshot, boundary};

/// One host-extracted require site, anchored to immutable source bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
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
#[derive(Clone, Debug, PartialEq, Eq)]
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
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Definition {
    /// Opaque host declaration identity.
    pub name: String,

    /// Source revision.
    pub revision: u64,

    /// Declaration source bytes.
    pub text: String,
}

struct Source {
    configuration: Snapshot,
    text: String,
    revision: u64,
    sites: Vec<Site>,
    definitions: Vec<Definition>,
}

pub(crate) struct Cancellation(instar_analysis::Cancellation);

impl Cancellation {
    pub(crate) fn requested(&self) -> bool {
        self.0.requested()
    }
}

#[derive(Default)]
pub(crate) struct Host {
    sources: BTreeMap<String, Source>,
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
            };
        };

        boundary::NativeSource {
            found: true,
            text: source.text.clone(),
            revision: source.revision,
            definitions: source
                .definitions
                .iter()
                .map(|definition| boundary::NativeDefinition {
                    name: definition.name.clone(),
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
#[derive(Clone, Debug, PartialEq, Eq)]
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
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FactKind {
    /// An unannotated local binding inferred as any.
    ImplicitAnyLocal,

    /// An unannotated function parameter inferred as any.
    ImplicitAnyParameter,
}

/// A detached semantic finding from the native type graph.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fact {
    /// Binding source range.
    pub location: Location<String>,

    /// Semantic property.
    pub kind: FactKind,

    /// Description of the semantic property.
    pub message: String,
}

/// Native lint output, retaining diagnostics when analysis is incomplete.
#[derive(Clone, Debug, PartialEq, Eq)]
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

fn completion(value: boundary::NativeCompletion) -> io::Result<Completion> {
    match value {
        boundary::NativeCompletion::Complete => Ok(Completion::Complete),
        boundary::NativeCompletion::Cancelled => Ok(Completion::Incomplete(Reason::Cancelled)),
        boundary::NativeCompletion::Timeout => Ok(Completion::Incomplete(Reason::Timeout)),
        boundary::NativeCompletion::Environment => Ok(Completion::Incomplete(Reason::Environment)),
        boundary::NativeCompletion::Analysis => Ok(Completion::Incomplete(Reason::Analysis)),
        _ => Err(io::Error::other("unknown native completion status")),
    }
}

fn location(value: boundary::NativeLocation) -> Location<String> {
    Location {
        module: value.module,
        revision: value.revision,
        range: [value.start, value.end],
    }
}

fn diagnostic(value: boundary::NativeDiagnostic) -> io::Result<Diagnostic<String>> {
    let kind = match value.kind {
        boundary::NativeKind::Syntax => Kind::Syntax {
            code: Some(value.code),
        },

        boundary::NativeKind::Type => Kind::Type { code: value.code },

        boundary::NativeKind::Resolution => Kind::Resolution {
            code: Some(value.code),
        },

        boundary::NativeKind::Analysis => Kind::Analysis {
            code: (value.code != 0).then_some(value.code),
        },

        _ => return Err(io::Error::other("unknown native diagnostic kind")),
    };

    Ok(Diagnostic {
        location: location(value.location),
        kind,
        message: value.message,
        related: value
            .related
            .into_iter()
            .map(|related| Related {
                location: location(related.location),
                message: related.message,
            })
            .collect(),
    })
}

/// A native Luau Frontend whose resolution policy belongs exclusively to its host.
pub struct Frontend {
    native: cxx::UniquePtr<boundary::NativeFrontend>,
    host: Host,
}

impl Frontend {
    /// Allocates native analysis storage and registers standard Luau builtins.
    ///
    /// # Errors
    /// Returns native allocation or builtin initialization errors.
    pub fn new() -> io::Result<Self> {
        let native = boundary::create_frontend().map_err(io::Error::other)?;

        if native.is_null() {
            return Err(io::Error::other("native frontend allocation failed"));
        }

        Ok(Self {
            native,
            host: Host::default(),
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

        let configuration = configuration
            .native
            .as_ref()
            .ok_or_else(|| io::Error::other("native configuration is unavailable"))?;

        self.native
            .pin_mut()
            .configure(name, configuration)
            .map_err(io::Error::other)?;

        self.host.sources.insert(
            name.to_owned(),
            Source {
                configuration: snapshot,
                text: text.to_owned(),
                revision,
                sites: sites.to_vec(),
                definitions: Vec::new(),
            },
        );

        Ok(())
    }

    /// Parses a reachable host graph and validates static requests against native AST ranges.
    ///
    /// # Errors
    /// Returns missing host targets, native parsing failures or host/native agreement errors.
    pub fn parse(&mut self, entry: &str) -> io::Result<Vec<Link>> {
        let mut names = BTreeSet::new();
        let mut pending = vec![entry.to_owned()];

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

        let names = names.into_iter().collect::<Vec<_>>();

        let links = self
            .native
            .pin_mut()
            .prepare(&self.host, &names)
            .map_err(io::Error::other)?;

        Ok(links
            .into_iter()
            .map(|link| Link {
                module: link.module,
                revision: link.revision,
                call: [link.call_start, link.call_end],
                argument: [link.argument_start, link.argument_end],
                target: link.target,
            })
            .collect())
    }

    /// Installs declaration snapshots for an already inserted module.
    ///
    /// # Errors
    /// Rejects unavailable modules or invalid declaration identities.
    pub fn definitions(&mut self, name: &str, definitions: &[Definition]) -> io::Result<()> {
        let mut names = BTreeSet::new();

        for definition in definitions {
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
                    .any(|existing| existing.name == definition.name && existing != definition)
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

    /// Checks selected entries with their complete host-resolved dependency graph.
    ///
    /// Native lint checks are disabled. Declaration loading and parsing cannot be
    /// preempted upstream; overruns and cancellation are checked around those phases.
    ///
    /// # Errors
    /// Returns missing host targets, host/native disagreement or native failures.
    pub fn check(
        &mut self,
        entries: &[String],
        options: &Options,
    ) -> io::Result<instar_analysis::Result<String>> {
        let names = self.names(entries)?;

        let result = self
            .native
            .pin_mut()
            .check(
                &self.host,
                entries,
                options.timeout.as_secs_f64(),
                &names,
                &Cancellation(options.cancellation.clone()),
            )
            .map_err(io::Error::other)?;

        let completion = completion(result.completion)?;

        let diagnostics = result
            .diagnostics
            .into_iter()
            .map(diagnostic)
            .collect::<io::Result<Vec<_>>>()?;

        Ok(instar_analysis::Result {
            modules: names,
            diagnostics,
            completion,
        })
    }

    /// Runs enabled upstream warnings using the shared native analysis session.
    ///
    /// Parsing, declaration loading and upstream lint passes cannot be preempted;
    /// cancellation and timeout overruns are checked around these phases.
    ///
    /// # Errors
    /// Returns missing host targets, host/native disagreement or native failures.
    pub fn lint(&mut self, entries: &[String], options: &Options) -> io::Result<LintResult> {
        self.lint_inner(entries, &[], options)
    }

    /// Runs native warnings and extracts inferred-any facts for selected source modules.
    ///
    /// Nocheck semantic selections produce an explicit incomplete result. The same
    /// phase-boundary timeout and cancellation limits apply as for native lint.
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
        let modules = self.names(entries)?;

        if semantic_modules.iter().any(|name| !modules.contains(name)) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "semantic selections must belong to the reachable host graph",
            ));
        }

        let result = self
            .native
            .pin_mut()
            .lint(
                &self.host,
                entries,
                options.timeout.as_secs_f64(),
                &modules,
                &Cancellation(options.cancellation.clone()),
                semantic_modules,
            )
            .map_err(io::Error::other)?;

        Ok(LintResult {
            modules,
            warnings: result
                .warnings
                .into_iter()
                .map(|warning| Warning {
                    location: location(warning.location),
                    code: warning.code,
                    name: warning.name,
                    message: warning.message,
                    fatal: warning.fatal,
                })
                .collect(),
            facts: result
                .facts
                .into_iter()
                .map(|fact| {
                    let kind = match fact.kind {
                        boundary::NativeFactKind::ImplicitAnyLocal => FactKind::ImplicitAnyLocal,

                        boundary::NativeFactKind::ImplicitAnyParameter => {
                            FactKind::ImplicitAnyParameter
                        }

                        _ => return Err(io::Error::other("unknown native semantic fact kind")),
                    };

                    Ok(Fact {
                        location: location(fact.location),
                        kind,
                        message: fact.message,
                    })
                })
                .collect::<io::Result<Vec<_>>>()?,
            diagnostics: result
                .diagnostics
                .into_iter()
                .map(diagnostic)
                .collect::<io::Result<Vec<_>>>()?,
            completion: completion(result.completion)?,
        })
    }

    /// Removes changed module revisions from host and native analysis caches.
    ///
    /// # Errors
    /// Returns native invalidation failures.
    pub fn invalidate(&mut self, names: &[String]) -> io::Result<()> {
        self.native
            .pin_mut()
            .invalidate(names)
            .map_err(io::Error::other)?;

        for name in names {
            self.host.sources.remove(name);
        }

        Ok(())
    }
}
