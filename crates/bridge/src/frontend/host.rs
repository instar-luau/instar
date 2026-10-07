use std::collections::BTreeMap;

use super::source::Source;
use crate::boundary;
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
    pub(crate) fn read_document(&self, name: &str) -> boundary::NativeDocument {
        let document = self
            .sources
            .get(name)
            .map(|source| (&source.text, source.revision))
            .or_else(|| {
                self.sources
                    .values()
                    .flat_map(|source| &source.definitions)
                    .find(|definition| definition.name == name)
                    .map(|definition| (&definition.text, definition.revision))
            });

        boundary::NativeDocument {
            found: document.is_some(),
            text: document.map_or_else(String::new, |(text, _)| text.clone()),
            revision: document.map_or(0, |(_, revision)| revision),
        }
    }

    pub(crate) fn require(&self, name: &str, start: usize, end: usize) -> bool {
        self.sources
            .get(name)
            .is_some_and(|source| source.sites.iter().any(|site| site.call == [start, end]))
    }

    pub(crate) fn read_source(&self, name: &str) -> boundary::NativeSource {
        let Some(source) = self.sources.get(name) else {
            let document = self.read_document(name);

            return boundary::NativeSource {
                found: document.found,
                text: document.text,
                revision: document.revision,
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
