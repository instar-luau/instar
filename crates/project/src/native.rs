//! Native analysis preparation from the Rust-owned graph.

use std::{
    collections::{BTreeMap, BTreeSet},
    io,
    path::Path,
    rc::Rc,
    sync::Arc,
    time::Instant,
};

use instar_analysis::{Completion, Options, error::invalid};
use instar_bridge::frontend::{Frontend, Link, Site};

use crate::{
    analysis::Origin,
    environment::Environment,
    graph::Node,
    project::{Project, Settings},
    resolve::{Identity, Module},
};

impl Project {
    /// Prepares reachable native ASTs using Rust sources, settings and resolved sites.
    ///
    /// No project source is typechecked. Native analysis receives exact identities and
    /// validates every static Rust request against its parsed argument and call ranges.
    ///
    /// # Errors
    /// Returns graph discovery, source/configuration or native agreement failures.
    pub fn prepare(
        &mut self,
        source: &Path,
        context: Option<&Identity>,
        options: &Options,
    ) -> io::Result<Vec<Link>> {
        self.during(
            |project| project.preparation(source, context, options),
            options,
        )
    }

    pub(crate) fn install(
        &mut self,
        source: &Path,
        context: Option<&Identity>,
        installed: &mut BTreeSet<Identity>,
    ) -> io::Result<Module> {
        let entry = self.links(source, context)?.module.clone();
        let identities = self.discovery(&entry, installed)?;
        let mut frontend = self.frontend.take().unwrap_or_default();

        let result = (|| {
            for identity in &identities {
                self.interrupted()?;

                let node = self
                    .graph
                    .node(identity)
                    .ok_or_else(|| invalid("discovered module is missing from the Rust graph"))?;

                let directory = node
                    .module
                    .source
                    .parent()
                    .ok_or_else(|| invalid("module source has no directory"))?;

                let settings = self.configuration(directory)?;

                register(&node, &settings, &mut frontend)?;
            }

            installed.extend(identities);

            Ok(entry)
        })();

        self.frontend = Some(frontend);

        result
    }

    fn preparation(
        &mut self,
        source: &Path,
        context: Option<&Identity>,
        options: &Options,
    ) -> io::Result<Vec<Link>> {
        let started = Instant::now();
        let entry = self.install(source, context, &mut BTreeSet::new())?;

        self.frontend
            .as_mut()
            .ok_or_else(|| invalid("native frontend is unavailable"))?
            .parse(&name(&entry.identity), &options.remaining(started))
    }
}

/// Encodes an exact Rust identity as an opaque native module name.
#[must_use]
pub fn name(identity: &Identity) -> String {
    format!("{identity:?}")
}

pub(crate) fn register(
    node: &Node,
    settings: &Settings,
    frontend: &mut Frontend,
) -> io::Result<()> {
    let sites = node
        .sites
        .iter()
        .map(|site| Site {
            call: site.call,
            argument: site.range,
            static_request: site.request.is_some(),
            target: site.target.as_ref().map(|target| name(&target.identity)),
        })
        .collect::<Vec<_>>();

    let name = name(&node.module.identity);

    frontend.insert(
        &name,
        Arc::clone(&node.document.text),
        node.document.revision,
        &settings.native,
        &sites,
    )?;

    frontend.flags(&name, &settings.flags)
}

pub(crate) fn definitions(
    environments: &BTreeMap<String, Rc<Environment>>,
    frontend: &mut Frontend,
    options: &Options,
    started: Instant,
) -> io::Result<Completion> {
    for (completed, (name, environment)) in environments.iter().enumerate() {
        options.report(
            "Registering declarations",
            completed,
            Some(environments.len()),
            Some(name),
        );

        if let Some(reason) = options.interrupted(started) {
            return Ok(Completion::Incomplete(reason));
        }

        frontend.definitions(name, &environment.definitions)?;
    }

    options.report(
        "Registering declarations",
        environments.len(),
        Some(environments.len()),
        None,
    );

    Ok(options
        .interrupted(started)
        .map_or(Completion::Complete, Completion::Incomplete))
}

pub(crate) fn progress(modules: &[Origin], options: &Options) -> Options {
    let mut limits = options.clone();

    if let Some(observer) = &options.progress {
        let observer = Arc::clone(observer);

        let sources = modules
            .iter()
            .filter_map(|origin| match origin {
                Origin::Module(module) => Some((
                    name(&module.identity),
                    module.source.to_string_lossy().into_owned(),
                )),

                Origin::Definition(_) => None,
            })
            .collect::<BTreeMap<_, _>>();

        limits.progress = Some(Arc::new(move |mut event| {
            if let Some(source) = event.source.as_ref().and_then(|source| sources.get(source)) {
                event.source = Some(source.clone());
            }

            observer(event);
        }));
    }

    limits
}
