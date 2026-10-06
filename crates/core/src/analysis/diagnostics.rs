use std::{collections::BTreeMap, io};

use instar_analysis::{Diagnostic, Kind, Location, Related};

use super::{Origin, locate};
use crate::{graph::Node, native, source::Document};

struct Resolution {
    location: Location<Origin>,
    call: [usize; 2],
}

#[derive(Default)]
pub(crate) struct Report {
    pub(crate) sources: BTreeMap<String, (Origin, Document)>,
    pub(crate) diagnostics: Vec<Diagnostic<Origin>>,
    resolutions: Vec<Resolution>,
}

impl Report {
    pub(crate) fn collect(&mut self, node: &Node) -> Origin {
        let origin = Origin::Module(node.module.clone());

        self.sources.insert(
            native::name(&node.module.identity),
            (origin.clone(), node.document.clone()),
        );

        for site in &node.sites {
            if let Some(failure) = &site.failure {
                let location = Location {
                    module: origin.clone(),
                    revision: node.document.revision,
                    range: site.range,
                };

                self.resolutions.push(Resolution {
                    location: location.clone(),
                    call: site.call,
                });

                self.record(Diagnostic {
                    location,
                    kind: Kind::Resolution { code: None },
                    message: failure.message.clone(),
                    related: Vec::new(),
                });
            }
        }

        for problem in &node.problems {
            self.record(Diagnostic {
                location: Location {
                    module: origin.clone(),
                    revision: node.document.revision,
                    range: problem.range,
                },
                kind: Kind::Syntax { code: None },
                message: problem.message.clone(),
                related: Vec::new(),
            });
        }

        origin
    }

    pub(crate) fn record(&mut self, diagnostic: Diagnostic<Origin>) {
        if !self.diagnostics.contains(&diagnostic) {
            self.diagnostics.push(diagnostic);
        }
    }

    pub(crate) fn native(&mut self, diagnostic: Diagnostic<String>) -> io::Result<()> {
        let location = locate(&diagnostic.location, &self.sources)?;

        let related = diagnostic
            .related
            .into_iter()
            .map(|related| {
                Ok(Related {
                    location: locate(&related.location, &self.sources)?,
                    message: related.message,
                })
            })
            .collect::<io::Result<Vec<_>>>()?;

        if let Kind::Resolution { code } = diagnostic.kind
            && let Some(resolution) = self.resolutions.iter().find(|resolution| {
                let overlaps = if location.range[0] == location.range[1] {
                    (resolution.call[0]..resolution.call[1]).contains(&location.range[0])
                } else {
                    location.range[0] < resolution.call[1] && resolution.call[0] < location.range[1]
                };

                resolution.location.module == location.module
                    && resolution.location.revision == location.revision
                    && overlaps
            })
            && let Some(existing) = self.diagnostics.iter_mut().find(|existing| {
                matches!(existing.kind, Kind::Resolution { .. })
                    && existing.location == resolution.location
            })
        {
            existing.kind = Kind::Resolution {
                code: code.or(existing.kind.native_code()),
            };

            for related in related {
                if !existing.related.contains(&related) {
                    existing.related.push(related);
                }
            }

            return Ok(());
        }

        if matches!(diagnostic.kind, Kind::Syntax { code: Some(_) }) {
            self.diagnostics.retain(|existing| {
                !matches!(existing.kind, Kind::Syntax { code: None })
                    || existing.location.module != location.module
                    || existing.location.revision != location.revision
            });
        }

        self.record(Diagnostic {
            location,
            kind: diagnostic.kind,
            message: diagnostic.message,
            related,
        });

        Ok(())
    }
}
