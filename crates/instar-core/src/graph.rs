//! Reachable require graphs with lexical extraction and retained failures.

use std::{
    collections::{BTreeSet, HashMap, HashSet},
    io,
    path::PathBuf,
    rc::Rc,
};

use petgraph::{
    algo::kosaraju_scc,
    graph::{DiGraph, NodeIndex},
};

use serde::Serialize;
use vermis::{Kind, Parts, View};

use crate::{
    invalid,
    project::Project,
    resolve::{Failure, Module, Resolver},
    string_value,
};

/// Stable node index within a graph snapshot.
pub type ModuleId = NodeIndex<usize>;

/// A source or configuration problem encountered while discovering a module.
#[derive(Debug, Serialize)]
pub struct Diagnostic {
    /// Byte offset into the source, when applicable.
    pub offset: Option<usize>,

    /// Explanation of the problem.
    pub message: String,
}

/// A require call, including unresolved and dynamic expressions.
#[derive(Debug, Serialize)]
pub struct RequireSite {
    /// Argument byte range, with an exclusive end.
    pub range: [usize; 2],

    /// One-based source line and byte column.
    pub location: [usize; 2],

    /// Original argument expression.
    pub expression: String,

    /// Statically known string, if any.
    pub request: Option<String>,

    /// Resolved node identity.
    pub target: Option<ModuleId>,

    /// Why this site could not be resolved.
    pub failure: Option<Failure>,

    /// Probed files and directories, including missing candidates.
    pub candidates: BTreeSet<PathBuf>,

    /// Configuration files consulted, including absent files.
    pub configurations: BTreeSet<PathBuf>,
}

/// One source module, parsed at most once in this graph snapshot.
#[derive(Serialize)]
pub struct Node {
    /// Navigation and filesystem identities.
    pub module: Module,

    /// Require sites in source order.
    pub requires: Vec<RequireSite>,

    /// Parse, source-loading, and configuration errors.
    pub diagnostics: Vec<Diagnostic>,

    /// Effective native configuration JSON, if configuration loaded successfully.
    pub configuration: Option<String>,

    /// Source snapshot used for all byte ranges in this node.
    #[serde(skip)]
    pub source: Option<Rc<str>>,
}

/// A dependency graph shared by any number of entry files.
#[derive(Default, Serialize)]
pub struct Graph {
    /// Explicit entry nodes, without duplicates.
    pub entries: BTreeSet<ModuleId>,

    /// Module storage and directed dependency edges, including incoming adjacency.
    pub nodes: DiGraph<Node, (), usize>,

    #[serde(skip)]
    identities: HashMap<PathBuf, ModuleId>,

    #[serde(skip)]
    resolver: Resolver,

    #[serde(skip)]
    processed: usize,
}

impl Graph {
    /// Creates an empty dependency graph.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds entry files and discovers their reachable dependencies without reprocessing known nodes.
    ///
    /// # Errors
    /// Returns an error if an explicitly supplied entry is missing or ambiguous.
    /// Dependency failures are retained on their require sites instead.
    pub fn add_entries(&mut self, project: &mut Project, paths: &[PathBuf]) -> io::Result<()> {
        let mut entries = paths
            .iter()
            .map(|path| self.resolver.entry(path))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| invalid(e.to_string()))?;

        entries.sort_by(|a, b| a.path.cmp(&b.path));

        for module in entries {
            let id = self.intern(module);
            self.entries.insert(id);
        }

        while self.processed < self.nodes.node_count() {
            let id = ModuleId::new(self.processed);
            self.processed += 1;
            self.discover(project, id);
        }

        Ok(())
    }

    /// Returns strongly connected components containing a dependency cycle.
    #[must_use]
    pub fn cycles(&self) -> Vec<Vec<ModuleId>> {
        let mut cycles = kosaraju_scc(&self.nodes);

        cycles.retain(|component| {
            component.len() > 1 || self.nodes.contains_edge(component[0], component[0])
        });

        for component in &mut cycles {
            component.sort_unstable();
        }

        cycles.sort();

        cycles
    }

    fn intern(&mut self, module: Module) -> ModuleId {
        if let Some(&id) = self.identities.get(&module.path) {
            return id;
        }

        let id = self.nodes.add_node(Node {
            module,
            requires: Vec::new(),
            diagnostics: Vec::new(),
            configuration: None,
            source: None,
        });

        self.identities
            .insert(self.nodes[id].module.path.clone(), id);

        id
    }

    fn discover(&mut self, project: &mut Project, id: ModuleId) {
        let module = self.nodes[id].module.clone();

        match project.configuration(&module.source) {
            Ok(config) => self.nodes[id].configuration = Some(config.json.clone()),

            Err(error) => self.nodes[id].diagnostics.push(Diagnostic {
                offset: None,
                message: error.to_string(),
            }),
        }

        let source = match project.source(&module.source) {
            Ok(source) => source,

            Err(error) => {
                self.nodes[id].diagnostics.push(Diagnostic {
                    offset: None,
                    message: error.to_string(),
                });

                return;
            }
        };

        let (mut sites, diagnostics) = extract(&source);
        self.nodes[id].diagnostics.extend(diagnostics);

        for site in &mut sites {
            if let Some(request) = &site.request {
                let resolution = self.resolver.resolve(project, &module, request);
                site.candidates = resolution.candidates;
                site.configurations = resolution.configurations;

                match resolution.result {
                    Ok(module) => {
                        let target = self.intern(module);
                        site.target = Some(target);
                        self.nodes.update_edge(id, target, ());
                    }

                    Err(error) => site.failure = Some(error),
                }
            }
        }

        self.nodes[id].requires = sites;
        self.nodes[id].source = Some(source);
    }
}

#[derive(Clone)]
enum Binding {
    Require,
    String(String),
    Other,
}

struct Extractor<'source> {
    source: &'source str,
    line_starts: Vec<usize>,
    scopes: Vec<HashMap<String, Binding>>,
    assigned: HashSet<String>,
    sites: Vec<RequireSite>,
}

fn extract(source: &str) -> (Vec<RequireSite>, Vec<Diagnostic>) {
    let tree = vermis::parse(source.as_bytes());

    let diagnostics = tree
        .diagnostics
        .iter()
        .map(|error| Diagnostic {
            offset: Some(error.span.start),
            message: error.message.to_owned(),
        })
        .collect();

    // ponytail: writes invalidate same-named constants across scopes; binding IDs can refine this later.
    let mut assigned = HashSet::new();

    for index in 0..tree.nodes.len() {
        if let Some(Parts::Assignment { targets, .. }) = tree.view(index).and_then(View::parts) {
            for target in targets {
                if target.kind() == Kind::Name {
                    assigned.insert(String::from_utf8_lossy(target.text()).into_owned());
                }
            }
        }

        if let Some(Parts::Function {
            name: Some(name), ..
        }) = tree.view(index).and_then(View::parts)
            && name.text() == b"require"
            && tree.nodes[index].kind != Kind::LocalFunction
        {
            assigned.insert("require".to_owned());
        }
    }

    let mut line_starts = vec![0];

    line_starts.extend(
        source
            .bytes()
            .enumerate()
            .filter_map(|(index, byte)| (byte == b'\n').then_some(index + 1)),
    );

    let mut extractor = Extractor {
        source,
        line_starts,
        scopes: vec![HashMap::new()],
        assigned,
        sites: Vec::new(),
    };

    if let Some(root) = tree.view(tree.root) {
        extractor.visit(root);
    }

    extractor.sites.sort_by_key(|site| site.range);

    (extractor.sites, diagnostics)
}

impl Extractor<'_> {
    fn binding(&self, name: &[u8]) -> Binding {
        let name = String::from_utf8_lossy(name);

        for scope in self.scopes.iter().rev() {
            if let Some(value) = scope.get(name.as_ref()) {
                return value.clone();
            }
        }

        if name == "require" {
            Binding::Require
        } else {
            Binding::Other
        }
    }

    fn value(&self, node: View<'_, '_>) -> Binding {
        match node.kind() {
            Kind::String => {
                return string_value(node.text()).map_or(Binding::Other, Binding::String);
            }

            Kind::Name => return self.binding(node.text()),
            _ => {}
        }

        match node.parts() {
            Some(Parts::Group { expression } | Parts::Assertion { expression, .. }) => {
                self.value(expression)
            }

            Some(Parts::Binary {
                left,
                operator,
                right,
            }) if operator.text() == b".." => {
                if let (Binding::String(left), Binding::String(right)) =
                    (self.value(left), self.value(right))
                {
                    Binding::String(left + &right)
                } else {
                    Binding::Other
                }
            }

            _ => Binding::Other,
        }
    }

    fn bind(&mut self, node: View<'_, '_>, value: Binding) {
        if let Some(Parts::Binding { name, annotation }) = node.parts() {
            if let Some(annotation) = annotation {
                self.visit(annotation);
            }

            let name = String::from_utf8_lossy(name.text()).into_owned();

            let value = if self.assigned.contains(&name) {
                Binding::Other
            } else {
                value
            };

            if let Some(scope) = self.scopes.last_mut() {
                scope.insert(name, value);
            }
        }
    }

    fn scoped(&mut self, node: View<'_, '_>) {
        self.scopes.push(HashMap::new());
        self.visit(node);
        self.scopes.pop();
    }

    fn function(&mut self, node: View<'_, '_>) {
        let Some(Parts::Function {
            name,
            parameters,
            returns,
            body,
            ..
        }) = node.parts()
        else {
            return;
        };

        if node.kind() == Kind::LocalFunction
            && let Some(name) = name
        {
            let name = String::from_utf8_lossy(name.text()).into_owned();

            if let Some(scope) = self.scopes.last_mut() {
                scope.insert(name, Binding::Other);
            }
        }

        self.scopes.push(HashMap::new());

        for parameter in parameters.children() {
            self.bind(parameter, Binding::Other);
        }

        if let Some(returns) = returns {
            self.visit(returns);
        }

        if let Some(body) = body {
            self.visit(body);
        }

        self.scopes.pop();
    }

    fn call(&mut self, callee: View<'_, '_>, arguments: View<'_, '_>) {
        if matches!(self.value(callee), Binding::Require) {
            let mut values = arguments.children();
            let argument = values.next();
            let single = argument.is_some() && values.next().is_none();
            let span = argument.map_or(arguments.span(), View::span);

            let request = if single && !self.assigned.contains("require") {
                argument.and_then(|arg| match self.value(arg) {
                    Binding::String(value) => Some(value),
                    _ => None,
                })
            } else {
                None
            };

            let failure = if !single {
                Some(Failure::Invalid("require expects one argument".into()))
            } else if request.is_none() {
                Some(Failure::Dynamic(
                    "require target is not a statically known string".into(),
                ))
            } else {
                None
            };

            let line = self
                .line_starts
                .partition_point(|&start| start <= span.start);

            self.sites.push(RequireSite {
                range: [span.start, span.end],
                location: [line, span.start - self.line_starts[line - 1] + 1],
                expression: self.source[span.start..span.end].to_owned(),
                request,
                target: None,
                failure,
                candidates: BTreeSet::new(),
                configurations: BTreeSet::new(),
            });
        }

        self.visit(callee);

        for argument in arguments.children() {
            self.visit(argument);
        }
    }

    fn visit(&mut self, node: View<'_, '_>) {
        if node.kind() == Kind::TypeFunction {
            return;
        }

        match node.parts() {
            Some(Parts::Local { bindings, values }) => {
                let values = values.collect::<Vec<_>>();

                let evaluated = values
                    .iter()
                    .map(|&value| self.value(value))
                    .collect::<Vec<_>>();

                for value in values {
                    self.visit(value);
                }

                for (index, binding) in bindings.enumerate() {
                    self.bind(
                        binding,
                        evaluated.get(index).cloned().unwrap_or(Binding::Other),
                    );
                }
            }

            Some(Parts::Function { .. }) => self.function(node),

            Some(Parts::If {
                branches,
                otherwise,
            }) => {
                for branch in branches {
                    self.scoped(branch);
                }

                if let Some(otherwise) = otherwise {
                    self.scoped(otherwise);
                }
            }

            Some(Parts::While { condition, body }) => {
                self.visit(condition);
                self.scoped(body);
            }

            Some(Parts::Repeat { body, condition }) => {
                self.scopes.push(HashMap::new());
                self.visit(body);
                self.visit(condition);
                self.scopes.pop();
            }

            Some(Parts::NumericFor {
                binding,
                start,
                end,
                step,
                body,
            }) => {
                self.visit(start);
                self.visit(end);

                if let Some(step) = step {
                    self.visit(step);
                }

                self.scopes.push(HashMap::new());
                self.bind(binding, Binding::Other);
                self.visit(body);
                self.scopes.pop();
            }

            Some(Parts::GenericFor {
                bindings,
                values,
                body,
            }) => {
                for value in values {
                    self.visit(value);
                }

                self.scopes.push(HashMap::new());

                for binding in bindings {
                    self.bind(binding, Binding::Other);
                }

                self.visit(body);
                self.scopes.pop();
            }

            Some(Parts::Body { body }) if node.kind() == Kind::Do => self.scoped(body),
            Some(Parts::Call { callee, arguments }) => self.call(callee, arguments),

            _ => {
                for child in node.children() {
                    self.visit(child);
                }
            }
        }
    }
}
