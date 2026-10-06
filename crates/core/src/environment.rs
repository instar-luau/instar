use std::{collections::BTreeMap, time::Instant};

use instar_analysis::{Completion, Diagnostic, Kind, Location, Options, Reason};
use instar_bridge::frontend::Definition;

use crate::{
    analysis::Origin,
    configuration::Security,
    graph::Node,
    project::{Project, Settings},
    source::Document,
};

pub(crate) struct Environment {
    pub(crate) definitions: Vec<Definition>,
    pub(crate) diagnostics: Vec<Diagnostic<Origin>>,
    pub(crate) completion: Completion,
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

        if configuration.roblox.enabled == Some(true)
            && (configuration.environment.definitions.is_empty()
                || !matches!(configuration.roblox.security, Security::None))
        {
            environment.diagnostics.push(Diagnostic {
                location: Location {
                    module: Origin::Module(node.module.clone()),
                    revision: node.document.revision,
                    range: [0, 0],
                },
                kind: Kind::Unsupported,
                message: "Native Roblox analysis requires explicit API declarations and does not support API security filtering".to_owned(),
                related: Vec::new(),
            });

            environment.completion = Completion::Incomplete(Reason::Unsupported);
        }

        for path in &configuration.environment.definitions {
            if let Some(reason) = options.interrupted(started) {
                environment.completion = Completion::Incomplete(reason);
                break;
            }

            let document = match self.source(path) {
                Ok(document) => document,

                Err(error) => {
                    environment.diagnostics.push(Diagnostic {
                        location: Location {
                            module: Origin::Definition(path.clone()),
                            revision: 0,
                            range: [0, 0],
                        },
                        kind: Kind::Analysis { code: None },
                        message: error.to_string(),
                        related: Vec::new(),
                    });

                    environment.completion = environment
                        .completion
                        .combine(Completion::Incomplete(Reason::Environment));

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
                .any(|definition| definition.name == name)
            {
                environment.definitions.push(Definition {
                    name,
                    revision: document.revision,
                    text: document.text.to_string(),
                });
            }
        }

        environment
    }
}
