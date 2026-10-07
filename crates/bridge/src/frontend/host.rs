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
