use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    mem,
    path::{Path, PathBuf},
    sync::Arc,
};

use instar_core::workspace::{GeneratorReporter, Generators};
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use tower_lsp_server::{
    gen_lsp_types::Uri,
    jsonrpc::{Error, Result},
};

use super::{
    configuration::Configuration,
    document::{self, Encoding},
    error::failure,
    project::Project,
};

pub(super) type Documents = BTreeMap<PathBuf, BTreeMap<String, Stamp>>;

pub(super) struct State {
    pub(super) projects: BTreeMap<PathBuf, Project>,
    pub(super) configuration: Configuration,
    pub(super) configuration_revision: u64,
    pub(super) capabilities: Arc<Value>,
    pub(super) encoding: Encoding,
    pub(super) events: Vec<PathBuf>,
    pub(super) history: History,
    pub(super) report: Option<GeneratorReporter>,
    documents: Arc<Documents>,
    topology: BTreeMap<PathBuf, bool>,
}

impl State {
    pub(super) fn new() -> Self {
        Self {
            projects: BTreeMap::new(),
            configuration: Configuration::default(),
            configuration_revision: 0,
            capabilities: Arc::new(Value::Null),
            encoding: Encoding::default(),
            events: Vec::new(),
            history: History::default(),
            report: None,
            documents: Arc::default(),
            topology: BTreeMap::new(),
        }
    }

    pub(super) fn observe(&mut self, paths: &[PathBuf]) -> Result<()> {
        self.history.record(paths)?;
        self.events.extend_from_slice(paths);

        Ok(())
    }

    pub(super) fn documents(&self) -> Arc<Documents> {
        Arc::clone(&self.documents)
    }

    fn refresh(&mut self) {
        let count = self
            .projects
            .values()
            .flat_map(|project| project.documents.values())
            .filter(|document| document.version.is_some())
            .count();

        let previous: usize = self.documents.values().map(BTreeMap::len).sum();

        let is_unchanged = count == previous
            && self.topology.len() == self.projects.len()
            && self.projects.iter().all(|(root, project)| {
                self.topology.get(root) == Some(&project.should_index)
                    && project.documents.iter().all(|(module, document)| {
                        document.version.is_none()
                            || self
                                .documents
                                .get(root)
                                .and_then(|documents| documents.get(module))
                                .is_some_and(|stamp| {
                                    stamp.uri == document.uri
                                        && Some(stamp.version) == document.version
                                        && stamp.revision == document.revision
                                        && stamp.text == document.text
                                })
                    })
            });

        if is_unchanged {
            return;
        }

        self.documents = Arc::new(
            self.projects
                .iter()
                .filter_map(|(root, project)| {
                    let documents: BTreeMap<_, _> = project
                        .documents
                        .iter()
                        .filter_map(|(module, document)| {
                            Some((
                                module.clone(),
                                Stamp {
                                    uri: document.uri.clone(),
                                    version: document.version?,
                                    revision: document.revision,
                                    text: Arc::clone(&document.text),
                                },
                            ))
                        })
                        .collect();

                    (!documents.is_empty()).then(|| (root.clone(), documents))
                })
                .collect(),
        );

        self.topology = self
            .projects
            .iter()
            .map(|(root, project)| (root.clone(), project.should_index))
            .collect();
    }

    pub(super) fn propagate(&mut self, previous: &Arc<Documents>) -> Result<()> {
        self.refresh();

        if Arc::ptr_eq(previous, &self.documents) {
            return Ok(());
        }

        let current = self.documents();

        for (root, project) in &mut self.projects {
            if previous
                .iter()
                .filter(|(owner, _)| *owner != root)
                .ne(current.iter().filter(|(owner, _)| *owner != root))
            {
                project.changed()?;
            }
        }

        Ok(())
    }

    pub(super) fn roots(&self) -> Result<Vec<Uri>> {
        self.projects
            .keys()
            .map(|root| document::uri(root))
            .collect()
    }

    pub(super) fn add(&mut self, uri: &Uri) -> Result<()> {
        let root = document::path(uri)?;

        let project = Project::new(
            &root,
            self.configuration.clone(),
            Arc::clone(&self.capabilities),
            self.encoding,
            Generators::new(
                &CancellationToken::new(),
                Arc::clone(
                    self.report
                        .as_ref()
                        .expect("Internal error: generator reporter is missing."),
                ),
            ),
        )?;

        let root = project.workspace.directory().to_owned();

        if self
            .projects
            .get(&root)
            .is_some_and(|project| project.should_index)
        {
            return Ok(());
        }

        let previous = self.projects.get(&root).map(|project| project.should_index);

        self.projects
            .entry(root.clone())
            .or_insert(project)
            .should_index = true;

        if let Err(error) = self.reconcile() {
            if let Some(should_index) = previous {
                self.projects
                    .get_mut(&root)
                    .expect("Internal error: added project is missing.")
                    .should_index = should_index;
            } else {
                self.projects.remove(&root);
            }

            return Err(error);
        }

        Ok(())
    }

    pub(super) fn remove(&mut self, uri: &Uri) -> Result<Vec<Uri>> {
        let root = self.path(uri)?;

        let Some(project) = self.projects.get_mut(&root) else {
            return Ok(Vec::new());
        };

        if !project.should_index {
            return Ok(Vec::new());
        }

        let removed = project
            .published
            .iter()
            .map(|module| document::uri(Path::new(module)))
            .collect::<Result<_>>()?;

        let should_index = project.should_index;
        let published = mem::take(&mut project.published);
        project.should_index = false;

        if let Err(error) = self.reconcile() {
            let project = self
                .projects
                .get_mut(&root)
                .expect("Internal error: removed project is missing.");

            project.should_index = should_index;
            project.published = published;

            return Err(error);
        }

        Ok(removed)
    }

    fn owner(&self, path: &Path) -> Option<&PathBuf> {
        self.projects
            .iter()
            .filter(|(root, project)| project.should_index && path.starts_with(root))
            .max_by_key(|(root, _)| root.components().count())
            .map(|(root, _)| root)
            .or_else(|| {
                self.projects
                    .iter()
                    .find(|(_, project)| {
                        project.documents.iter().any(|(module, document)| {
                            document.version.is_some() && Path::new(module) == path
                        })
                    })
                    .map(|(root, _)| root)
            })
            .or_else(|| {
                self.projects
                    .iter()
                    .filter(|(root, project)| !project.should_index && path.starts_with(root))
                    .max_by_key(|(root, _)| root.components().count())
                    .map(|(root, _)| root)
            })
    }

    pub(super) fn reconcile(&mut self) -> Result<()> {
        let mut transfers = Vec::new();
        let mut destinations = BTreeSet::new();
        let mut publications = Vec::new();

        for (root, project) in &self.projects {
            for (module, document) in &project.documents {
                if document.version.is_none() {
                    continue;
                }

                let destination = self.owner(Path::new(module)).unwrap_or(root);

                if destination == root {
                    continue;
                }

                if !destinations.insert((destination.clone(), module.clone()))
                    || self.projects[destination]
                        .documents
                        .get(module)
                        .is_some_and(|document| document.version.is_some())
                {
                    return Err(Error::invalid_params("Open document owners collide."));
                }

                transfers.push((
                    root.clone(),
                    destination.clone(),
                    module.clone(),
                    document.clone(),
                ));
            }

            for module in &project.published {
                if let Some(destination) = self.owner(Path::new(module))
                    && destination != root
                {
                    publications.push((root.clone(), destination.clone(), module.clone()));
                }
            }
        }

        let mut overlays = Vec::new();

        for (_, destination, module, document) in &transfers {
            let result = self.projects[destination]
                .workspace
                .set_source(Path::new(module), Arc::clone(&document.text))
                .map_err(failure)
                .and_then(|identity| {
                    let is_matching = identity == *module;

                    let previous = self.projects[destination]
                        .documents
                        .get(&identity)
                        .or_else(|| self.projects[destination].borrowed.get(&identity))
                        .map(|document| Arc::clone(&document.text));

                    overlays.push((destination.clone(), identity, previous));

                    if is_matching {
                        Ok(())
                    } else {
                        Err(Error::content_modified())
                    }
                });

            if let Err(error) = result {
                self.restore(overlays)?;

                return Err(error);
            }
        }

        for (root, destination, module, document) in transfers {
            let project = self
                .projects
                .get_mut(&root)
                .expect("Internal error: document owner is missing.");

            project.workspace.remove_source(&module);
            project.documents.remove(&module);

            self.projects
                .get_mut(&destination)
                .expect("Internal error: document destination is missing.")
                .adopt(module, document);
        }

        for (root, destination, module) in publications {
            self.projects
                .get_mut(&root)
                .expect("Internal error: diagnostic owner is missing.")
                .published
                .remove(&module);

            self.projects
                .get_mut(&destination)
                .expect("Internal error: diagnostic destination is missing.")
                .published
                .insert(module);
        }

        let roots: BTreeSet<_> = self
            .projects
            .iter()
            .filter(|(_, project)| project.should_index)
            .map(|(root, _)| root.clone())
            .collect();

        for (root, project) in &mut self.projects {
            project.nested = roots
                .iter()
                .filter(|nested| *nested != root && nested.starts_with(root.as_path()))
                .cloned()
                .collect();

            project.tokens.clear();
            project.changed()?;
            project.invalidation = project.generation;
        }

        self.projects.retain(|_, project| {
            project.should_index
                || !project.published.is_empty()
                || project
                    .documents
                    .values()
                    .any(|document| document.version.is_some())
        });

        Ok(())
    }

    pub(super) fn synchronize(&mut self, root: &Path) -> Result<()> {
        self.projects
            .get_mut(root)
            .expect("Internal error: synchronized project is missing.")
            .advance_generators()?;

        let mut borrowed = BTreeMap::new();

        for (owner, project) in &self.projects {
            if owner == root {
                continue;
            }

            for (module, document) in &project.documents {
                if document.version.is_some()
                    && borrowed.insert(module.clone(), document.clone()).is_some()
                {
                    return Err(Error::invalid_params("Open document owners collide."));
                }
            }
        }

        let project = &self.projects[root];

        if borrowed.keys().any(|module| {
            project
                .documents
                .get(module)
                .is_some_and(|document| document.version.is_some())
        }) {
            return Err(Error::invalid_params("Open document owners collide."));
        }

        let has_changed = project.borrowed.len() != borrowed.len()
            || borrowed.iter().any(|(module, document)| {
                project.borrowed.get(module).is_none_or(|previous| {
                    previous.uri != document.uri
                        || previous.version != document.version
                        || previous.revision != document.revision
                        || previous.text != document.text
                })
            });

        if !has_changed {
            return Ok(());
        }

        let mut overlays = Vec::new();
        let project = &self.projects[root];

        for (module, document) in &borrowed {
            if project
                .borrowed
                .get(module)
                .is_some_and(|previous| previous.text == document.text)
            {
                continue;
            }

            let result = project
                .workspace
                .set_source(Path::new(module), Arc::clone(&document.text))
                .map_err(failure)
                .and_then(|identity| {
                    let previous = project
                        .documents
                        .get(&identity)
                        .or_else(|| project.borrowed.get(&identity))
                        .map(|document| Arc::clone(&document.text));

                    let is_matching = identity == *module;
                    overlays.push((root.to_owned(), identity, previous));

                    if is_matching {
                        Ok(())
                    } else {
                        Err(Error::content_modified())
                    }
                });

            if let Err(error) = result {
                self.restore(overlays)?;

                return Err(error);
            }
        }

        let project = self
            .projects
            .get_mut(root)
            .expect("Internal error: synchronized project is missing.");

        for module in project.borrowed.keys() {
            if !borrowed.contains_key(module)
                && project
                    .documents
                    .get(module)
                    .is_none_or(|document| document.version.is_none())
            {
                project.workspace.remove_source(module);
                project.documents.remove(module);
            }
        }

        for module in borrowed.keys() {
            project.documents.remove(module);
        }

        project.borrowed = borrowed;
        project.tokens.clear();

        Ok(())
    }

    pub(super) fn select(&mut self, uri: &Uri) -> Result<PathBuf> {
        let path = self.path(uri)?;

        if let Some(root) = self.owner(&path) {
            return Ok(root.clone());
        }

        let root = if path.is_dir() {
            path
        } else {
            path.parent()
                .ok_or_else(|| Error::invalid_params("Document has no parent directory."))?
                .to_owned()
        };

        let mut project = Project::new(
            &root,
            self.configuration.clone(),
            Arc::clone(&self.capabilities),
            self.encoding,
            Generators::new(
                &CancellationToken::new(),
                Arc::clone(
                    self.report
                        .as_ref()
                        .expect("Internal error: generator reporter is missing."),
                ),
            ),
        )?;

        project.should_index = false;
        let root = project.workspace.directory().to_owned();
        self.projects.entry(root.clone()).or_insert(project);

        Ok(root)
    }

    pub(super) fn restore(&self, overlays: Vec<(PathBuf, String, Option<Arc<str>>)>) -> Result<()> {
        let mut restore_error = None;

        for (root, module, previous) in overlays.into_iter().rev() {
            let workspace = &self.projects[&root].workspace;

            let result = if let Some(previous) = previous {
                workspace
                    .set_source(Path::new(&module), previous)
                    .map(|_| ())
                    .map_err(failure)
            } else {
                workspace.remove_source(&module);

                Ok(())
            };

            if let Err(error) = result
                && restore_error.is_none()
            {
                restore_error = Some(error);
            }
        }

        restore_error.map_or(Ok(()), Err)
    }

    fn path(&self, uri: &Uri) -> Result<PathBuf> {
        let path = document::path(uri)?;

        let path = if path.try_exists().map_err(failure)? {
            path.canonicalize().map_err(failure)?
        } else {
            path
        };

        if let Some(project) = self.projects.values().next() {
            project
                .workspace
                .normalize(&path)
                .map(PathBuf::from)
                .map_err(failure)
        } else {
            Ok(path)
        }
    }
}

#[derive(Eq, PartialEq)]
pub(super) struct Stamp {
    uri: Uri,
    version: i32,
    revision: u64,
    text: Arc<str>,
}

#[derive(Clone, Default)]
pub(super) struct History {
    pub(super) sequence: u64,
    entries: VecDeque<Change>,
}

impl History {
    fn record(&mut self, paths: &[PathBuf]) -> Result<()> {
        if paths.is_empty() {
            return Ok(());
        }

        self.sequence = self
            .sequence
            .checked_add(1)
            .ok_or_else(Error::internal_error)?;

        self.entries.push_back(Change {
            sequence: self.sequence,
            paths: Arc::from(paths),
        });

        if self.entries.len() > 256 {
            self.entries.pop_front();
        }

        Ok(())
    }

    pub(super) fn since(&self, sequence: u64) -> Option<impl Iterator<Item = &Change>> {
        if sequence > self.sequence
            || self
                .entries
                .front()
                .is_some_and(|change| sequence < change.sequence - 1)
        {
            return None;
        }

        Some(
            self.entries
                .iter()
                .filter(move |change| change.sequence > sequence),
        )
    }
}

#[derive(Clone)]
pub(super) struct Change {
    pub(super) paths: Arc<[PathBuf]>,
    sequence: u64,
}
