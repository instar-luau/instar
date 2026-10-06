//! Native analysis preparation from the Rust-owned graph.

use crate::{configuration::invalid, project::Project, resolve::Identity};
use instar_bridge::frontend::{Frontend, Link, Site};
use std::{io, path::Path};

/// Encodes an exact Rust identity as an opaque native module name.
#[must_use]
pub fn name(identity: &Identity) -> String {
    format!("{identity:?}")
}

impl Project {
    /// Prepares reachable native ASTs using Rust sources, settings and resolved sites.
    ///
    /// No checker runs. Native analysis receives exact contextual identities and
    /// validates every static Rust request against its parsed argument and call ranges.
    ///
    /// # Errors
    /// Returns graph discovery, source/configuration or native agreement failures.
    pub fn prepare(&mut self, source: &Path, context: Option<&Identity>) -> io::Result<Vec<Link>> {
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
            }

            frontend.parse(&name(&entry.identity))
        })();

        self.frontend = Some(frontend);

        result
    }
}
