//! Native analysis preparation from the Rust-owned graph.

use std::{io, path::Path};

use instar_analysis::error::invalid;
use instar_bridge::frontend::{Frontend, Link, Site};

use crate::{
    project::Project,
    resolve::{Identity, Module},
};

/// Encodes an exact Rust identity as an opaque native module name.
#[must_use]
pub fn name(identity: &Identity) -> String {
    format!("{identity:?}")
}

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
        options: &instar_analysis::Options,
    ) -> io::Result<Vec<Link>> {
        let started = std::time::Instant::now();
        let entry = self.install(source, context)?;

        self.frontend
            .as_mut()
            .ok_or_else(|| invalid("native frontend is unavailable"))?
            .parse(&name(&entry.identity), &options.remaining(started))
    }

    pub(crate) fn install(
        &mut self,
        source: &Path,
        context: Option<&Identity>,
    ) -> io::Result<Module> {
        let entry = self.links(source, context)?.module.clone();
        let identities = self.discover(source, context)?;

        let mut frontend = match self.frontend.take() {
            Some(frontend) => frontend,
            None => Frontend::new()?,
        };

        let result = (|| {
            for identity in identities {
                let node = self
                    .graph
                    .node(&identity)
                    .ok_or_else(|| invalid("discovered module is missing from the Rust graph"))?;

                let directory = node
                    .module
                    .source
                    .parent()
                    .ok_or_else(|| invalid("module source has no directory"))?;

                let settings = self.configuration(directory)?;

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

                frontend.insert(
                    &name(&identity),
                    &node.document.text,
                    node.document.revision,
                    &settings.native,
                    &sites,
                )?;

                frontend.flags(&name(&identity), &settings.configuration.luau.native()?)?;
            }

            Ok(entry)
        })();

        self.frontend = Some(frontend);

        result
    }
}
