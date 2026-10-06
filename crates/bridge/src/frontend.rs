//! Native analysis snapshots adapted from host-owned resolution results.

use crate::{Configuration, boundary};

use std::{
    collections::{BTreeMap, BTreeSet},
    io,
};

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

struct Source {
    text: String,
    revision: u64,
    sites: Vec<Site>,
}

#[derive(Default)]
pub(crate) struct Host {
    sources: BTreeMap<String, Source>,
}

impl Host {
    pub(crate) fn read_source(&self, name: &str) -> boundary::NativeSource {
        let Some(source) = self.sources.get(name) else {
            return boundary::NativeSource {
                found: false,
                text: String::new(),
                revision: 0,
                sites: Vec::new(),
            };
        };

        boundary::NativeSource {
            found: true,
            text: source.text.clone(),
            revision: source.revision,
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

/// A native Luau Frontend whose resolution policy belongs exclusively to its host.
pub struct Frontend {
    native: cxx::UniquePtr<boundary::NativeFrontend>,
    host: Host,
}

impl Frontend {
    /// Allocates native analysis storage without running a checker.
    ///
    /// # Errors
    /// Returns native allocation errors.
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
                text: text.to_owned(),
                revision,
                sites: sites.to_vec(),
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
