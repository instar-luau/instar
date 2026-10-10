use std::{
    collections::{BTreeMap, BTreeSet},
    io, mem,
    rc::Rc,
};

use instar_analysis::{Diagnostic, Kind, Location, Related};

use super::{Origin, locate};

use crate::{
    environment::Environment,
    graph::{Graph, Node},
    native,
    source::Document,
};

struct Resolution {
    location: Location<Origin>,
    call: [usize; 2],
    runtime: bool,
}

#[derive(Default)]
pub(crate) struct Report {
    pub(crate) lint: bool,
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

                let runtime = failure.kind == io::ErrorKind::Unsupported && site.request.is_none();

                self.resolutions.push(Resolution {
                    location: location.clone(),
                    call: site.call,
                    runtime,
                });

                if runtime && self.lint {
                    continue;
                }

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
        {
            if resolution.runtime && self.lint {
                return Ok(());
            }

            if let Some(existing) = self.diagnostics.iter_mut().find(|existing| {
                matches!(existing.kind, Kind::Resolution { .. })
                    && existing.location == resolution.location
            }) {
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

    pub(crate) fn target(origin: &Origin, targets: &BTreeSet<String>) -> bool {
        matches!(origin, Origin::Module(module) if targets.contains(&native::name(&module.identity)))
    }

    pub(crate) fn select(
        &mut self,
        targets: &BTreeSet<String>,
        graph: &Graph,
        environments: &BTreeMap<String, Rc<Environment>>,
    ) -> Vec<Diagnostic<Origin>> {
        for diagnostic in mem::take(&mut self.diagnostics) {
            if Self::target(&diagnostic.location.module, targets) {
                self.record(diagnostic.clone());
            }

            if matches!(diagnostic.kind, Kind::Type { .. })
                && matches!(diagnostic.location.module, Origin::Module(_))
            {
                continue;
            }

            let mut affected = match &diagnostic.location.module {
                Origin::Module(module) => BTreeSet::from([module.identity.clone()]),

                Origin::Definition(path) => {
                    let name = format!("Definition({})", path.display());

                    graph
                        .nodes
                        .keys()
                        .filter(|identity| {
                            environments
                                .get(&native::name(identity))
                                .is_some_and(|environment| {
                                    environment
                                        .definitions
                                        .iter()
                                        .any(|definition| definition.name == name)
                                        || environment.diagnostics.iter().any(|failure| {
                                            failure.location.module == diagnostic.location.module
                                        })
                                })
                        })
                        .cloned()
                        .collect()
                }
            };

            let mut pending = affected.iter().cloned().collect::<Vec<_>>();

            while let Some(identity) = pending.pop() {
                for dependent in graph.dependents(&identity) {
                    if affected.insert(dependent.clone()) {
                        pending.push(dependent);
                    }
                }
            }

            for identity in &affected {
                let name = native::name(identity);

                if !targets.contains(&name) {
                    continue;
                }

                let Some((origin, document)) = self.sources.get(&name) else {
                    continue;
                };

                if *origin == diagnostic.location.module {
                    continue;
                }

                let range = graph
                    .node(identity)
                    .and_then(|node| {
                        node.sites
                            .iter()
                            .find(|site| {
                                site.target
                                    .as_ref()
                                    .is_some_and(|target| affected.contains(&target.identity))
                            })
                            .map(|site| site.range)
                    })
                    .unwrap_or([0, 0]);

                let mut related = vec![Related {
                    location: diagnostic.location.clone(),
                    message: diagnostic.message.clone(),
                }];

                related.extend(diagnostic.related.iter().cloned());

                self.record(Diagnostic {
                    location: Location {
                        module: origin.clone(),
                        revision: document.revision,
                        range,
                    },
                    kind: Kind::Analysis {
                        code: diagnostic.kind.native_code(),
                    },
                    message: format!(
                        "Dependency {}: {}",
                        diagnostic.location.module.source().display(),
                        diagnostic.message
                    ),
                    related,
                });
            }
        }

        mem::take(&mut self.diagnostics)
    }
}
