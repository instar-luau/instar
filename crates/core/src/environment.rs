use std::{collections::BTreeMap, time::Instant};

use instar_analysis::{Completion, Diagnostic, Kind, Location, Options, Reason};
use instar_bridge::frontend::Definition;

use crate::{
    analysis::Origin,
    graph::Node,
    project::{Project, Settings},
    source::Document,
};

pub(crate) struct Environment {
    pub(crate) definitions: Vec<Definition>,
    pub(crate) diagnostics: Vec<Diagnostic<Origin>>,
    pub(crate) completion: Completion,
}

impl Environment {
    fn failure(&mut self, location: Location<Origin>, error: &std::io::Error) {
        self.diagnostics.push(Diagnostic {
            location,
            kind: Kind::Analysis { code: None },
            message: error.to_string(),
            related: Vec::new(),
        });

        self.completion = self
            .completion
            .combine(Completion::Incomplete(Reason::Environment));
    }
}

impl Project {
    pub(crate) fn environment(
        &mut self,
        node: &Node,
        settings: &Settings,
        sources: &mut BTreeMap<String, (Origin, Document)>,
        options: &Options,
        started: Instant,
    ) -> Environment {
        let mut environment = Environment {
            definitions: Vec::new(),
            diagnostics: Vec::new(),
            completion: Completion::Complete,
        };

        let configuration = &settings.configuration;

        let classes = if configuration.roblox.enabled == Some(true) {
            match self.roblox_assets(configuration.roblox.security, options, started) {
                Ok(assets) => {
                    for (path, document) in assets.definitions {
                        let name = format!("Definition({})", path.display());
                        sources.insert(name.clone(), (Origin::Definition(path), document.clone()));

                        environment.definitions.push(Definition {
                            name,
                            namespace: "@roblox".to_owned(),
                            revision: document.revision,
                            text: document.text.to_string(),
                        });
                    }

                    assets.classes
                }

                Err(error) => {
                    environment.failure(
                        Location {
                            module: Origin::Module(node.module.clone()),
                            revision: node.document.revision,
                            range: [0, 0],
                        },
                        &error,
                    );

                    if let Some(reason) = options.interrupted(started) {
                        environment.completion = Completion::Incomplete(reason);
                    }

                    Vec::new()
                }
            }
        } else {
            Vec::new()
        };

        if let Some(frontend) = &mut self.frontend
            && let Err(error) =
                frontend.classes(&crate::native::name(&node.module.identity), &classes)
        {
            environment.failure(
                Location {
                    module: Origin::Module(node.module.clone()),
                    revision: node.document.revision,
                    range: [0, 0],
                },
                &error,
            );
        }

        for (namespace, path) in configuration.environment.iter().flat_map(|environment| {
            environment
                .definitions
                .iter()
                .map(|path| (&environment.namespace, path))
        }) {
            if let Some(reason) = options.interrupted(started) {
                environment.completion = Completion::Incomplete(reason);
                break;
            }

            let document = match self.source(path) {
                Ok(document) => document,

                Err(error) => {
                    environment.failure(
                        Location {
                            module: Origin::Definition(path.clone()),
                            revision: 0,
                            range: [0, 0],
                        },
                        &error,
                    );

                    continue;
                }
            };

            let name = format!("Definition({})", path.display());

            sources.insert(
                name.clone(),
                (Origin::Definition(path.clone()), document.clone()),
            );

            if !environment
                .definitions
                .iter()
                .any(|definition| definition.name == name && &definition.namespace == namespace)
            {
                environment.definitions.push(Definition {
                    name,
                    namespace: namespace.clone(),
                    revision: document.revision,
                    text: document.text.to_string(),
                });
            }
        }

        environment
    }
}
