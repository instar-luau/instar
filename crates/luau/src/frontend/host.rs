use std::{
    collections::{BTreeMap, BTreeSet},
    io::{self, Write},
    sync::Arc,
};

use instar_analysis::{Options, Progress, process::Message};

use super::source::{Class, Declaration, Environment, Source};
use crate::{boundary, protocol::Response};

pub(crate) struct Cancellation(
    pub(crate) instar_analysis::Cancellation,
    pub(crate) Option<Options>,
);

impl Cancellation {
    pub(crate) fn requested(&self) -> bool {
        self.0.requested()
    }

    pub(crate) fn started(&self) {
        if let Some(options) = &self.1 {
            options.report("configuration", 0, None, None);
        }
    }
}

#[derive(Default)]
pub(crate) struct Host {
    pub(crate) sources: BTreeMap<String, Source>,
    pub(crate) declarations: BTreeMap<String, Declaration>,
    pub(crate) environments: BTreeMap<u64, Environment>,
    classes: BTreeMap<Arc<[Class]>, u64>,
    revision: u64,
    pub(crate) progress: bool,
}

impl Host {
    pub(crate) fn detach(&mut self, name: &str) {
        if let Some(source) = self.sources.get_mut(name) {
            for binding in source.definitions.drain(..) {
                let declaration = self
                    .declarations
                    .get_mut(&binding.name)
                    .expect("registered declaration");

                declaration.consumers.remove(name);

                if declaration.consumers.is_empty() {
                    self.declarations.remove(&binding.name);
                }
            }
        }
    }

    pub(crate) fn environment(&mut self, name: &str, classes: &[Class]) -> io::Result<()> {
        let previous = self
            .sources
            .get(name)
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "module is unavailable"))?
            .environment;

        let identity = if classes.is_empty() {
            None
        } else if let Some(identity) = self.classes.get(classes) {
            Some(*identity)
        } else {
            self.revision = self
                .revision
                .checked_add(1)
                .ok_or_else(|| io::Error::other("environment revision exhausted"))?;

            let classes: Arc<[Class]> = Arc::from(classes);
            self.classes.insert(Arc::clone(&classes), self.revision);

            self.environments.insert(
                self.revision,
                Environment {
                    classes,
                    consumers: BTreeSet::new(),
                },
            );

            Some(self.revision)
        };

        if identity == previous {
            return Ok(());
        }

        self.release(name);

        if let Some(identity) = identity {
            self.environments
                .get_mut(&identity)
                .expect("registered environment")
                .consumers
                .insert(name.to_owned());
        }

        self.sources
            .get_mut(name)
            .expect("available module")
            .environment = identity;

        Ok(())
    }

    pub(crate) fn remove(&mut self, name: &str) {
        self.detach(name);
        self.release(name);
        self.sources.remove(name);
    }

    pub(crate) fn progress(
        &self,
        phase: &str,
        source: &str,
        completed: usize,
        total: usize,
    ) -> io::Result<()> {
        if completed > total || (!source.is_empty() && !self.sources.contains_key(source)) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid native progress event",
            ));
        }

        if !self.progress {
            return Ok(());
        }

        let event = Message::<Response>::Progress(Progress {
            phase: phase.to_owned(),
            completed,
            total: Some(total),
            source: (!source.is_empty()).then(|| source.to_owned()),
        });

        let mut output = io::stdout().lock();
        serde_json::to_writer(&mut output, &event)?;
        output.write_all(b"\n")?;

        output.flush()
    }

    pub(crate) fn read_document(&self, name: &str) -> boundary::NativeDocument {
        let document = self
            .sources
            .get(name)
            .map(|source| (&source.text, source.revision))
            .or_else(|| {
                self.declarations
                    .get(name)
                    .map(|declaration| (&declaration.text, declaration.revision))
            });

        boundary::NativeDocument {
            found: document.is_some(),
            text: document.map_or_else(String::new, |(text, _)| text.to_string()),
            revision: document.map_or(0, |(_, revision)| revision),
        }
    }

    pub(crate) fn revision(&self, name: &str) -> io::Result<u64> {
        self.sources
            .get(name)
            .map(|source| source.revision)
            .or_else(|| {
                self.declarations
                    .get(name)
                    .map(|declaration| declaration.revision)
            })
            .ok_or_else(|| {
                io::Error::other(format!("native diagnostic source is unavailable: {name}"))
            })
    }

    pub(crate) fn offset(&self, name: &str, line: usize, column: usize) -> io::Result<usize> {
        let (text, lines) = self
            .sources
            .get(name)
            .map(|source| (&source.text, &source.lines))
            .or_else(|| {
                self.declarations
                    .get(name)
                    .map(|source| (&source.text, &source.lines))
            })
            .ok_or_else(|| io::Error::other("native source is unavailable"))?;

        let start = *lines
            .get(line)
            .ok_or_else(|| io::Error::other("native source position exceeds host revision"))?;

        start
            .checked_add(column)
            .filter(|&offset| offset <= text.len())
            .ok_or_else(|| io::Error::other("native source column exceeds host revision"))
    }

    pub(crate) fn require(&self, name: &str, start: usize, end: usize) -> bool {
        self.sources
            .get(name)
            .is_some_and(|source| source.sites.iter().any(|site| site.call == [start, end]))
    }

    pub(crate) fn read_module(&self, name: &str) -> boundary::NativeModule {
        let Some(source) = self.sources.get(name) else {
            let declaration = self.declarations.get(name);

            return boundary::NativeModule {
                found: declaration.is_some(),
                revision: declaration.map_or(0, |declaration| declaration.revision),
                sites: Vec::new(),
            };
        };

        boundary::NativeModule {
            found: true,
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

    pub(crate) fn read_source(&self, name: &str) -> boundary::NativeSource {
        let Some(source) = self.sources.get(name) else {
            return boundary::NativeSource {
                definitions: Vec::new(),
                classes: Vec::new(),
            };
        };

        boundary::NativeSource {
            classes: source
                .environment
                .into_iter()
                .flat_map(|identity| self.environments[&identity].classes.iter())
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
                .map(|binding| {
                    let declaration = &self.declarations[&binding.name];

                    boundary::NativeDefinition {
                        name: binding.name.clone(),
                        namespace: binding.namespace.clone(),
                        revision: declaration.revision,
                    }
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

    fn release(&mut self, name: &str) {
        if let Some(identity) = self
            .sources
            .get_mut(name)
            .and_then(|source| source.environment.take())
        {
            let environment = self
                .environments
                .get_mut(&identity)
                .expect("registered environment");

            environment.consumers.remove(name);

            if environment.consumers.is_empty() {
                self.classes.remove(environment.classes.as_ref());
                self.environments.remove(&identity);
            }
        }
    }
}
