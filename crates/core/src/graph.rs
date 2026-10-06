//! Rust-owned lazy module graph and source-revision require sites.

use crate::{
    extract::extract,
    project::Project,
    resolve::{Identity, Module, Request},
    source::{Document, Failure, related},
};

use std::{
    collections::{BTreeMap, BTreeSet},
    io,
    path::{Path, PathBuf},
    rc::Rc,
};

/// A syntax parsing problem, independent of checker diagnostics.
#[derive(Clone, Debug)]
pub struct Problem {
    /// Source byte range at the document revision.
    pub range: [usize; 2],

    /// Parser explanation.
    pub message: String,
}

/// One lexical require call, including dynamic and failed requests.
#[derive(Clone, Debug)]
pub struct Site {
    /// Source revision anchoring both byte ranges.
    pub revision: u64,

    /// Argument byte range, with exclusive end.
    pub range: [usize; 2],

    /// Entire call byte range, with exclusive end.
    pub call: [usize; 2],

    /// Statically known argument, when available.
    pub request: Option<Request>,

    /// Resolved target, without forcing its dependencies to be expanded.
    pub target: Option<Module>,

    /// Retained extraction or resolution failure.
    pub failure: Option<Failure>,

    /// Present and absent consulted files, settings and maps.
    pub inputs: BTreeSet<PathBuf>,
}

/// One immutable parsed module snapshot.
#[derive(Clone, Debug)]
pub struct Node {
    /// Exact module and source identity.
    pub module: Module,

    /// Source snapshot anchoring require sites and syntax problems.
    pub document: Document,

    /// Require calls in source order.
    pub sites: Vec<Site>,

    /// Syntax parsing problems, without typechecking.
    pub problems: Vec<Problem>,

    /// All source, navigation, configuration and map inputs.
    pub inputs: BTreeSet<PathBuf>,
}

/// Module storage and incoming dependency adjacency.
#[derive(Default)]
pub struct Graph {
    pub(crate) nodes: BTreeMap<Identity, Rc<Node>>,
    reverse: BTreeMap<Identity, BTreeSet<Identity>>,
}

impl Graph {
    /// Returns a cached immutable module snapshot if it has been expanded.
    #[must_use]
    pub fn node(&self, identity: &Identity) -> Option<Rc<Node>> {
        self.nodes.get(identity).cloned()
    }

    /// Returns direct incoming dependencies, including targets not yet expanded.
    #[must_use]
    pub fn dependents(&self, identity: &Identity) -> BTreeSet<Identity> {
        self.reverse.get(identity).cloned().unwrap_or_default()
    }

    /// Returns the number of expanded module contexts, not backing files.
    #[must_use]
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    /// Whether no module context has been expanded.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    fn insert(&mut self, node: Rc<Node>) {
        for site in &node.sites {
            if let Some(target) = &site.target {
                self.reverse
                    .entry(target.identity.clone())
                    .or_default()
                    .insert(node.module.identity.clone());
            }
        }

        self.nodes.insert(node.module.identity.clone(), node);
    }

    pub(crate) fn invalidate(&mut self, path: &Path) -> BTreeSet<Identity> {
        let mut affected = self
            .nodes
            .iter()
            .filter(|(_, node)| node.inputs.iter().any(|input| related(input, path)))
            .map(|(identity, _)| identity.clone())
            .collect::<BTreeSet<_>>();

        let mut pending = affected.iter().cloned().collect::<Vec<_>>();

        while let Some(identity) = pending.pop() {
            for dependent in self.reverse.get(&identity).into_iter().flatten() {
                if affected.insert(dependent.clone()) {
                    pending.push(dependent.clone());
                }
            }
        }

        self.nodes
            .retain(|identity, _| !affected.contains(identity));

        self.reverse.clear();

        for node in self.nodes.values() {
            for site in &node.sites {
                if let Some(target) = &site.target {
                    self.reverse
                        .entry(target.identity.clone())
                        .or_default()
                        .insert(node.module.identity.clone());
                }
            }
        }

        affected
    }
}

impl Project {
    /// Returns the Rust-owned module graph.
    #[must_use]
    pub fn graph(&self) -> &Graph {
        &self.graph
    }

    /// Extracts and resolves document links without expanding target modules.
    ///
    /// # Errors
    /// Returns entry source, identity, configuration or map errors. Site failures are retained.
    pub fn links(&mut self, source: &Path, context: Option<&Identity>) -> io::Result<Rc<Node>> {
        let outer = std::mem::take(&mut self.view.consulted);

        let result = (|| {
            let module = self.module(source, context)?;

            if let Some(node) = self.graph.node(&module.identity) {
                self.view.consulted.extend(node.inputs.iter().cloned());

                return Ok(node);
            }

            self.analyze(module)
        })();

        let inputs = std::mem::take(&mut self.view.consulted);
        self.view.consulted = outer;
        self.view.consulted.extend(inputs);

        result
    }

    fn analyze(&mut self, module: Module) -> io::Result<Rc<Node>> {
        let document = self.source(&module.source)?;
        let (mut sites, problems) = extract(&document.text, document.revision);
        let shared = self.view.consulted.clone();

        for site in &mut sites {
            site.inputs.extend(shared.iter().cloned());

            if let Some(request) = &site.request {
                let resolution = self.resolve(&module, request);
                site.inputs.extend(resolution.inputs);

                match resolution.result {
                    Ok(target) => {
                        self.view.consulted.insert(target.source.clone());
                        site.inputs.insert(target.source.clone());
                        site.target = Some(target);
                    }

                    Err(failure) => site.failure = Some(failure),
                }
            }
        }

        let node = Rc::new(Node {
            module,
            document,
            sites,
            problems,
            inputs: self.view.consulted.clone(),
        });

        self.graph.insert(Rc::clone(&node));

        Ok(node)
    }

    /// Expands reachable modules on demand; failures and cycles remain graph data.
    ///
    /// # Errors
    /// Returns errors identifying or loading the explicit entry or a resolved target.
    pub fn discover(
        &mut self,
        source: &Path,
        context: Option<&Identity>,
    ) -> io::Result<BTreeSet<Identity>> {
        let entry = self.links(source, context)?;
        let mut seen = BTreeSet::new();
        let mut pending = vec![entry.module.clone()];

        while let Some(module) = pending.pop() {
            if !seen.insert(module.identity.clone()) {
                continue;
            }

            let node = self.links(&module.source, Some(&module.identity))?;
            pending.extend(node.sites.iter().filter_map(|site| site.target.clone()));
        }

        Ok(seen)
    }
}
