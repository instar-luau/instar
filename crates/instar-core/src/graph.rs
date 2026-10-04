//! Reachable require graphs with lexical extraction and retained failures.

use std::{
    collections::{BTreeSet, HashMap, HashSet},
    io,
    path::PathBuf,
    rc::Rc,
};

use petgraph::{algo::kosaraju_scc, graph::DiGraph};

use serde::Serialize;

use vermis::{
    token::{Keyword, Symbol, TokenKind},
    tree::{NodeIndex, NodeKind, NodeList, Tree},
};

use crate::{
    invalid,
    project::Project,
    resolve::{Failure, Module, Resolver},
    roblox::{Instance, Sourcemap},
    string_value,
};

/// Stable node index within a graph snapshot.
pub type ModuleId = petgraph::graph::NodeIndex<usize>;

/// A source or configuration problem encountered while discovering a module.
#[derive(Debug, Serialize)]
pub struct Diagnostic {
    /// Byte offset into the source, when applicable.
    pub offset: Option<usize>,

    /// Explanation of the problem.
    pub message: String,
}

/// A statically identified require argument.
#[derive(Debug, Serialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum Request {
    /// String navigation from the current module.
    String(String),

    /// An exact instance in a sourcemap.
    Instance(Instance),
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

    /// Statically known string or instance, if any.
    pub request: Option<Request>,

    /// Resolved node identity.
    pub target: Option<ModuleId>,

    /// Why this site could not be resolved.
    pub failure: Option<Failure>,

    /// Probed files and directories, including missing candidates.
    pub candidates: BTreeSet<PathBuf>,

    /// Configuration files consulted, including absent files.
    pub configurations: BTreeSet<PathBuf>,

    /// Sourcemaps consulted, including failed instance lookups.
    pub sourcemaps: BTreeSet<PathBuf>,
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
    identities: HashMap<Module, ModuleId>,

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
        let mut entries = Vec::new();

        for path in paths {
            entries.extend(
                self.resolver
                    .entries(project, path)
                    .map_err(|e| invalid(e.to_string()))?,
            );
        }

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
        if let Some(&id) = self.identities.get(&module) {
            return id;
        }

        let id = self.nodes.add_node(Node {
            module,
            requires: Vec::new(),
            diagnostics: Vec::new(),
            configuration: None,
            source: None,
        });

        self.identities.insert(self.nodes[id].module.clone(), id);

        id
    }

    fn discover(&mut self, project: &mut Project, id: ModuleId) {
        let module = self.nodes[id].module.clone();
        let mut configurations = Vec::new();

        match project.configuration(&module.source) {
            Ok(config) => {
                self.nodes[id].configuration = Some(config.json.clone());
                configurations.clone_from(&config.inputs);
            }

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

        let map = if let Some(instance) = &module.instance {
            Ok(Some(Rc::clone(&instance.map)))
        } else {
            project
                .sourcemap(&module.source)
                .map_err(|error| Failure::Configuration(error.to_string()))
        };

        let extracted = extract(&source, module.instance.clone(), map);
        let mut sites = extracted.sites;
        self.nodes[id].diagnostics.extend(extracted.diagnostics);

        for site in &mut sites {
            site.configurations.extend(configurations.iter().cloned());

            if let Some(request) = &site.request {
                let resolution = match request {
                    Request::String(request) => self.resolver.resolve(project, &module, request),
                    Request::Instance(instance) => Resolver::resolve_instance(instance),
                };

                site.candidates.extend(resolution.candidates);
                site.configurations.extend(resolution.configurations);
                site.sourcemaps.extend(resolution.sourcemaps);

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
    Boolean(bool),
    Instance(Result<Instance, Failure>),
    Other,
}

struct Extractor<'tree, 'source> {
    tree: &'tree Tree<'source>,
    source: &'source str,
    line_starts: Vec<usize>,
    scopes: Vec<HashMap<String, Binding>>,
    assigned: HashSet<usize>,
    global_assigned: HashSet<String>,
    sites: Vec<RequireSite>,
    script: Option<Instance>,
    map: Result<Option<Rc<Sourcemap>>, Failure>,
    values: HashMap<usize, Binding>,
}

pub(crate) struct Extraction {
    pub(crate) sites: Vec<RequireSite>,
    pub(crate) diagnostics: Vec<Diagnostic>,
    pub(crate) expressions: HashMap<[usize; 2], Request>,
    pub(crate) line_starts: Vec<usize>,
    pub(crate) reexport: Option<[usize; 2]>,
}

pub(crate) fn extract(
    source: &str,
    script: Option<Instance>,
    map: Result<Option<Rc<Sourcemap>>, Failure>,
) -> Extraction {
    let tree = vermis::parse(source.as_bytes());

    let diagnostics = tree
        .diagnostics
        .iter()
        .map(|error| Diagnostic {
            offset: Some(error.span.start),
            message: error.message.to_owned(),
        })
        .collect();

    let mut writes = Writes::new(None);
    writes.visit(&tree, tree.root);

    let mut line_starts = vec![0];

    line_starts.extend(
        source
            .bytes()
            .enumerate()
            .filter_map(|(index, byte)| (byte == b'\n').then_some(index + 1)),
    );

    let mut extractor = Extractor {
        tree: &tree,
        source,
        line_starts,
        scopes: vec![HashMap::new()],
        assigned: writes.assigned,
        global_assigned: writes.global_assigned,
        sites: Vec::new(),
        script,
        map,
        values: HashMap::new(),
    };

    extractor.visit(tree.root);

    extractor.sites.sort_by_key(|site| site.range);

    let expressions = extractor
        .values
        .into_iter()
        .filter_map(|(index, value)| {
            let request = match value {
                Binding::String(value) => Request::String(value),
                Binding::Instance(Ok(instance)) => Request::Instance(instance),
                _ => return None,
            };

            let span = tree.node(NodeIndex::new(index)).span;

            Some(([span.start, span.end], request))
        })
        .collect();

    Extraction {
        sites: extractor.sites,
        diagnostics,
        expressions,
        line_starts: extractor.line_starts,
        reexport: reexport(&tree),
    }
}

fn reexport(tree: &Tree<'_>) -> Option<[usize; 2]> {
    if !tree.diagnostics.is_empty() {
        return None;
    }

    let NodeKind::Root { block, .. } = &tree.node(tree.root).kind else {
        return None;
    };

    let NodeKind::Block { statements } = &tree.node(*block).kind else {
        return None;
    };

    let [statement] = tree.list(statements) else {
        return None;
    };

    let NodeKind::Return { values, .. } = &tree.node(statement.node).kind else {
        return None;
    };

    let [value] = tree.list(values) else {
        return None;
    };

    let NodeKind::Call { callee, arguments } = &tree.node(value.node).kind else {
        return None;
    };

    if tree.text(*callee) != b"require" {
        return None;
    }

    let NodeKind::Arguments { values, .. } = &tree.node(*arguments).kind else {
        return None;
    };

    let [argument] = tree.list(values) else {
        return None;
    };

    let span = tree.node(argument.node).span;

    Some([span.start, span.end])
}

impl Extractor<'_, '_> {
    fn binding(&self, name: &[u8]) -> Binding {
        let name = String::from_utf8_lossy(name);

        for scope in self.scopes.iter().rev() {
            if let Some(value) = scope.get(name.as_ref()) {
                return value.clone();
            }
        }

        if name != "require" && self.global_assigned.contains(name.as_ref()) {
            return Binding::Other;
        }

        match name.as_ref() {
            "require" => Binding::Require,

            "script" => Binding::Instance(self.script.clone().ok_or_else(|| {
                Failure::Roblox("source is not mapped to a script instance".into())
            })),

            "game" | "workspace" => {
                let game = self
                    .map
                    .as_ref()
                    .map_err(Clone::clone)
                    .and_then(|map| {
                        map.as_ref()
                            .ok_or_else(|| Failure::Roblox("no sourcemap configured".into()))
                    })
                    .and_then(Sourcemap::game);

                Binding::Instance(if name == "workspace" {
                    game.and_then(|game| game.service("Workspace"))
                } else {
                    game
                })
            }

            _ => Binding::Other,
        }
    }

    fn value(&mut self, node: NodeIndex) -> Binding {
        let key = node.get();

        if let Some(value) = self.values.get(&key) {
            return value.clone();
        }

        let value = self.value_inner(node);
        self.values.insert(key, value.clone());

        value
    }

    fn value_inner(&mut self, node: NodeIndex) -> Binding {
        let tree = self.tree;

        match &tree.node(node).kind {
            NodeKind::String { .. } => {
                string_value(tree.text(node)).map_or(Binding::Other, Binding::String)
            }

            NodeKind::Boolean { token } => {
                Binding::Boolean(tree.token(*token).kind == TokenKind::Keyword(Keyword::True))
            }

            NodeKind::Name { .. } => self.binding(tree.text(node)),

            NodeKind::Group { expression, .. } | NodeKind::Assertion { expression, .. } => {
                self.value(*expression)
            }

            NodeKind::Binary {
                left,
                operator,
                right,
            } if tree.token(*operator).kind == TokenKind::Symbol(Symbol::Concatenate) => {
                if let (Binding::String(left), Binding::String(right)) =
                    (self.value(*left), self.value(*right))
                {
                    Binding::String(left + &right)
                } else {
                    Binding::Other
                }
            }

            NodeKind::Field { receiver, name, .. } => self.field(*receiver, tree.text(*name)),

            NodeKind::Index { receiver, key, .. } => {
                if let Binding::String(name) = self.value(*key) {
                    self.field(*receiver, name.as_bytes())
                } else {
                    Binding::Other
                }
            }

            NodeKind::MethodCall {
                receiver,
                method,
                arguments,
                ..
            } => self.method(*receiver, tree.text(*method), *arguments),

            _ => Binding::Other,
        }
    }

    fn field(&mut self, receiver: NodeIndex, name: &[u8]) -> Binding {
        let Binding::Instance(instance) = self.value(receiver) else {
            return Binding::Other;
        };

        let instance = match instance {
            Ok(instance) => instance,
            Err(error) => return Binding::Instance(Err(error)),
        };

        match name {
            b"Parent" => Binding::Instance(instance.parent()),
            b"Name" => Binding::String(instance.name().to_owned()),
            b"ClassName" => Binding::String(instance.class_name().to_owned()),
            b"GetService" | b"WaitForChild" | b"FindFirstChild" => Binding::Other,

            _ => match std::str::from_utf8(name) {
                Ok(name) => Binding::Instance(instance.child(name)),
                Err(_) => Binding::Other,
            },
        }
    }

    fn method(&mut self, receiver: NodeIndex, method: &[u8], arguments: NodeIndex) -> Binding {
        if !matches!(method, b"GetService" | b"WaitForChild" | b"FindFirstChild") {
            return Binding::Other;
        }

        let Binding::Instance(instance) = self.value(receiver) else {
            return Binding::Other;
        };

        let instance = match instance {
            Ok(instance) => instance,
            Err(error) => return Binding::Instance(Err(error)),
        };

        let NodeKind::Arguments { values, .. } = &self.tree.node(arguments).kind else {
            return Binding::Other;
        };

        let mut arguments = self.tree.list(values).iter().map(|entry| entry.node);

        let Some(first) = arguments.next() else {
            return Binding::Other;
        };

        let Binding::String(name) = self.value(first) else {
            return Binding::Other;
        };

        let second = arguments.next();

        if arguments.next().is_some() {
            return Binding::Other;
        }

        let result = match method {
            b"GetService" if second.is_none() => instance.service(&name),
            b"WaitForChild" => instance.child(&name),

            b"FindFirstChild" => {
                let recursive = match second.map(|value| self.value(value)) {
                    None | Some(Binding::Boolean(false)) => false,
                    Some(Binding::Boolean(true)) => true,
                    _ => return Binding::Other,
                };

                instance.find_child(&name, recursive)
            }

            _ => return Binding::Other,
        };

        Binding::Instance(result)
    }

    fn bind(&mut self, node: NodeIndex, value: Binding) {
        let tree = self.tree;

        if let NodeKind::Binding {
            name, annotation, ..
        } = &tree.node(node).kind
        {
            if let Some(annotation) = annotation {
                self.visit(*annotation);
            }

            let assigned = self.assigned.contains(&tree.node(*name).span.start);
            let name = String::from_utf8_lossy(tree.text(*name)).into_owned();

            let value = if assigned { Binding::Other } else { value };

            if let Some(scope) = self.scopes.last_mut() {
                scope.insert(name, value);
            }
        } else if let NodeKind::Variadic {
            annotation: Some(annotation),
            ..
        } = &tree.node(node).kind
        {
            self.visit(*annotation);
        }
    }

    fn scoped(&mut self, node: NodeIndex) {
        self.scopes.push(HashMap::new());
        self.visit(node);
        self.scopes.pop();
    }

    fn function(&mut self, node: NodeIndex) {
        let tree = self.tree;

        let NodeKind::Function {
            prefix,
            name,
            parameters,
            returns,
            body,
            ..
        } = &tree.node(node).kind
        else {
            return;
        };

        if prefix.is_some_and(|prefix| {
            tree.token(prefix).kind == TokenKind::Keyword(Keyword::Local)
                || tree.token(prefix).bytes(tree.source) == b"const"
        }) && let Some(name) = name
        {
            let name = String::from_utf8_lossy(tree.text(*name)).into_owned();

            if let Some(scope) = self.scopes.last_mut() {
                scope.insert(name, Binding::Other);
            }
        }

        self.scopes.push(HashMap::new());

        if let NodeKind::Parameters { parameters, .. } = &tree.node(*parameters).kind {
            for parameter in tree.list(parameters) {
                self.bind(parameter.node, Binding::Other);
            }
        }

        if let Some(returns) = returns {
            self.visit(*returns);
        }

        if let Some(body) = body {
            self.visit(*body);
        }

        self.scopes.pop();
    }

    fn call(&mut self, callee: NodeIndex, arguments: NodeIndex) {
        let tree = self.tree;

        let NodeKind::Arguments { values, .. } = &tree.node(arguments).kind else {
            return;
        };

        if matches!(self.value(callee), Binding::Require) {
            let mut values = tree.list(values).iter().map(|entry| entry.node);
            let argument = values.next();
            let single = argument.is_some() && values.next().is_none();
            let span = tree.node(argument.unwrap_or(arguments)).span;

            let value = if single && !self.global_assigned.contains("require") {
                argument.map_or(Binding::Other, |arg| self.value(arg))
            } else {
                Binding::Other
            };

            let (request, failure) = if single {
                match value {
                    Binding::String(value) => (Some(Request::String(value)), None),
                    Binding::Instance(Ok(instance)) => (Some(Request::Instance(instance)), None),
                    Binding::Instance(Err(error)) => (None, Some(error)),

                    _ => (
                        None,
                        Some(Failure::Dynamic(
                            "require target is not a statically known string or instance".into(),
                        )),
                    ),
                }
            } else {
                (
                    None,
                    Some(Failure::Invalid("require expects one argument".into())),
                )
            };

            let sourcemaps = if let Ok(Some(map)) = &self.map {
                [map.path.clone()].into()
            } else {
                BTreeSet::new()
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
                sourcemaps,
            });
        }

        self.visit(callee);

        for argument in tree.list(values) {
            self.visit(argument.node);
        }
    }

    fn local(&mut self, bindings: &NodeList, values: &NodeList) {
        let tree = self.tree;
        let values = tree.list(values);

        let evaluated = values
            .iter()
            .map(|value| self.value(value.node))
            .collect::<Vec<_>>();

        for value in values {
            self.visit(value.node);
        }

        for (index, binding) in tree.list(bindings).iter().enumerate() {
            self.bind(
                binding.node,
                evaluated.get(index).cloned().unwrap_or(Binding::Other),
            );
        }
    }

    fn visit(&mut self, node: NodeIndex) {
        let tree = self.tree;

        if matches!(&tree.node(node).kind, NodeKind::Function { prefix: Some(prefix), .. }
            if tree.token(*prefix).bytes(tree.source) == b"type")
        {
            return;
        }

        self.value(node);

        match &tree.node(node).kind {
            NodeKind::Local {
                bindings, values, ..
            }
            | NodeKind::Constant {
                bindings, values, ..
            } => self.local(bindings, values),

            NodeKind::Function { .. } => self.function(node),

            NodeKind::If {
                branches,
                otherwise,
                ..
            } => {
                for branch in tree.list(branches) {
                    self.scoped(branch.node);
                }

                if let Some(otherwise) = otherwise {
                    self.scoped(*otherwise);
                }
            }

            NodeKind::Conditional {
                condition,
                truthy,
                falsy,
                ..
            } => {
                self.scopes.push(HashMap::new());
                self.visit(*condition);
                self.visit(*truthy);
                self.scopes.pop();
                self.visit(*falsy);
            }

            NodeKind::While {
                condition, body, ..
            } => {
                self.visit(*condition);
                self.scoped(*body);
            }

            NodeKind::Repeat {
                body, condition, ..
            } => {
                self.scopes.push(HashMap::new());
                self.visit(*body);
                self.visit(*condition);
                self.scopes.pop();
            }

            NodeKind::NumericFor {
                binding,
                start,
                end,
                step,
                body,
                ..
            } => {
                self.visit(*start);
                self.visit(*end);

                if let Some(step) = step {
                    self.visit(*step);
                }

                self.scopes.push(HashMap::new());
                self.bind(*binding, Binding::Other);
                self.visit(*body);
                self.scopes.pop();
            }

            NodeKind::GenericFor {
                bindings,
                values,
                body,
                ..
            } => {
                for value in tree.list(values) {
                    self.visit(value.node);
                }

                self.scopes.push(HashMap::new());

                for binding in tree.list(bindings) {
                    self.bind(binding.node, Binding::Other);
                }

                self.visit(*body);
                self.scopes.pop();
            }

            NodeKind::Do { body, .. } => self.scoped(*body),
            NodeKind::Call { callee, arguments } => self.call(*callee, *arguments),

            _ => {
                for child in tree.children(node) {
                    self.visit(child);
                }
            }
        }
    }
}

pub(crate) struct Writes {
    scopes: Vec<HashMap<String, usize>>,
    assigned: HashSet<usize>,
    global_assigned: HashSet<String>,
    details: Option<WriteDetails>,
}

#[derive(Default)]
struct WriteDetails {
    functions: HashMap<usize, (usize, bool)>,
    arities: HashMap<usize, (usize, bool)>,
    mutated: HashSet<usize>,
}

impl Writes {
    fn new(details: Option<WriteDetails>) -> Self {
        Self {
            scopes: vec![HashMap::new()],
            assigned: HashSet::new(),
            global_assigned: HashSet::new(),
            details,
        }
    }

    pub(crate) fn analyze(tree: &Tree<'_>) -> Self {
        let mut writes = Self::new(Some(WriteDetails::default()));
        writes.visit(tree, tree.root);

        writes
    }

    pub(crate) fn assigned(&self, start: usize) -> bool {
        self.assigned.contains(&start)
    }

    pub(crate) fn mutated(&self, start: usize) -> bool {
        self.details
            .as_ref()
            .is_some_and(|details| details.mutated.contains(&start))
    }

    pub(crate) fn arity(&self, start: usize) -> Option<(usize, bool)> {
        self.details.as_ref()?.arities.get(&start).copied()
    }

    fn resolve(&self, tree: &Tree<'_>, name: NodeIndex) -> Option<usize> {
        let name = String::from_utf8_lossy(tree.text(name));

        self.scopes
            .iter()
            .rev()
            .find_map(|scope| scope.get(name.as_ref()).copied())
    }

    fn function_arity(&mut self, tree: &Tree<'_>, name: NodeIndex, value: NodeIndex) {
        let Some(details) = &mut self.details else {
            return;
        };

        let NodeKind::Function { parameters, .. } = &tree.node(value).kind else {
            return;
        };

        let NodeKind::Parameters { parameters, .. } = &tree.node(*parameters).kind else {
            return;
        };

        let mut minimum = 0;
        let mut variadic = false;

        for parameter in tree.list(parameters) {
            if matches!(tree.node(parameter.node).kind, NodeKind::Variadic { .. }) {
                variadic = true;
            } else {
                minimum += 1;
            }
        }

        details
            .functions
            .insert(tree.node(name).span.start, (minimum, variadic));
    }

    fn read(&mut self, tree: &Tree<'_>, name: NodeIndex) {
        if self.details.is_none() {
            return;
        }

        let Some(id) = self.resolve(tree, name) else {
            return;
        };

        let details = self.details.as_mut().unwrap();

        if let Some(&arity) = details.functions.get(&id) {
            details.arities.insert(tree.node(name).span.start, arity);
        }
    }

    fn mutate(&mut self, tree: &Tree<'_>, target: NodeIndex) {
        if self.details.is_none() {
            return;
        }

        let (NodeKind::Field { receiver, .. } | NodeKind::Index { receiver, .. }) =
            &tree.node(target).kind
        else {
            return;
        };

        if matches!(tree.node(*receiver).kind, NodeKind::Name { .. })
            && let Some(id) = self.resolve(tree, *receiver)
        {
            self.details.as_mut().unwrap().mutated.insert(id);
        }
    }

    fn bind(&mut self, tree: &Tree<'_>, node: NodeIndex) {
        if let NodeKind::Binding { name, .. } = &tree.node(node).kind {
            self.scopes.last_mut().unwrap().insert(
                String::from_utf8_lossy(tree.text(*name)).into_owned(),
                tree.node(*name).span.start,
            );
        }
    }

    fn write(&mut self, tree: &Tree<'_>, name: NodeIndex) {
        if let Some(id) = self.resolve(tree, name) {
            self.assigned.insert(id);

            if let Some(details) = &mut self.details {
                details.functions.remove(&id);
            }
        } else {
            let name = String::from_utf8_lossy(tree.text(name));
            self.global_assigned.insert(name.into_owned());
        }
    }

    fn scoped(&mut self, tree: &Tree<'_>, node: NodeIndex) {
        self.scopes.push(HashMap::new());
        self.visit(tree, node);
        self.scopes.pop();
    }

    fn visit(&mut self, tree: &Tree<'_>, node: NodeIndex) {
        if matches!(&tree.node(node).kind, NodeKind::Function { prefix: Some(prefix), .. }
            if tree.token(*prefix).bytes(tree.source) == b"type")
        {
            return;
        }

        match &tree.node(node).kind {
            NodeKind::Local {
                bindings, values, ..
            }
            | NodeKind::Constant {
                bindings, values, ..
            } => {
                for value in tree.list(values) {
                    self.visit(tree, value.node);
                }

                let mut values = tree.list(values).iter();

                for binding in tree.list(bindings) {
                    self.bind(tree, binding.node);

                    if let Some(value) = values.next()
                        && let NodeKind::Binding { name, .. } = &tree.node(binding.node).kind
                    {
                        self.function_arity(tree, *name, value.node);
                    }
                }
            }

            NodeKind::Assignment { targets, .. } => {
                for target in tree.list(targets) {
                    if matches!(tree.node(target.node).kind, NodeKind::Name { .. }) {
                        self.write(tree, target.node);
                    } else {
                        self.mutate(tree, target.node);
                    }
                }

                for child in tree.children(node) {
                    self.visit(tree, child);
                }
            }

            NodeKind::CompoundAssignment { target, value, .. } => {
                if matches!(tree.node(*target).kind, NodeKind::Name { .. }) {
                    self.write(tree, *target);
                } else {
                    self.mutate(tree, *target);
                }

                self.visit(tree, *target);
                self.visit(tree, *value);
            }

            NodeKind::Function { .. } => self.visit_function(tree, node),

            NodeKind::If { .. }
            | NodeKind::Conditional { .. }
            | NodeKind::While { .. }
            | NodeKind::Repeat { .. }
            | NodeKind::NumericFor { .. }
            | NodeKind::GenericFor { .. }
            | NodeKind::Do { .. } => self.visit_scoped_control(tree, node),

            NodeKind::Name { .. } => self.read(tree, node),

            _ => {
                for child in tree.children(node) {
                    self.visit(tree, child);
                }
            }
        }
    }

    fn visit_function(&mut self, tree: &Tree<'_>, node: NodeIndex) {
        let NodeKind::Function {
            prefix,
            name,
            parameters,
            body,
            ..
        } = &tree.node(node).kind
        else {
            return;
        };

        if let Some(name) = name {
            if prefix.is_some_and(|prefix| {
                tree.token(prefix).kind == TokenKind::Keyword(Keyword::Local)
                    || tree.token(prefix).bytes(tree.source) == b"const"
            }) {
                self.scopes.last_mut().unwrap().insert(
                    String::from_utf8_lossy(tree.text(*name)).into_owned(),
                    tree.node(*name).span.start,
                );

                self.function_arity(tree, *name, node);
            } else if matches!(tree.node(*name).kind, NodeKind::Name { .. }) {
                self.write(tree, *name);
            } else if let NodeKind::FunctionName { path, method, .. } = &tree.node(*name).kind
                && let Some(receiver) = tree.list(path).first()
            {
                let path = tree.list(path);

                if path.len() == 1 && method.is_none() {
                    self.write(tree, receiver.node);
                } else if (path.len() == 1 && method.is_some()
                    || path.len() == 2 && method.is_none())
                    && let Some(id) = self.resolve(tree, receiver.node)
                    && self.details.is_some()
                {
                    self.details.as_mut().unwrap().mutated.insert(id);
                }
            }
        }

        self.scopes.push(HashMap::new());

        if self.details.is_some()
            && let Some(name) = name
            && let NodeKind::FunctionName {
                method: Some(method),
                ..
            } = &tree.node(*name).kind
        {
            self.scopes
                .last_mut()
                .unwrap()
                .insert("self".into(), tree.node(*method).span.start);
        }

        if let NodeKind::Parameters { parameters, .. } = &tree.node(*parameters).kind {
            for parameter in tree.list(parameters) {
                self.bind(tree, parameter.node);
            }
        }

        if let Some(body) = body {
            self.visit(tree, *body);
        }

        self.scopes.pop();
    }

    fn visit_scoped_control(&mut self, tree: &Tree<'_>, node: NodeIndex) {
        match &tree.node(node).kind {
            NodeKind::If {
                branches,
                otherwise,
                ..
            } => {
                for branch in tree.list(branches) {
                    self.scoped(tree, branch.node);
                }

                if let Some(otherwise) = otherwise {
                    self.scoped(tree, *otherwise);
                }
            }

            NodeKind::Conditional {
                condition,
                truthy,
                falsy,
                ..
            } => {
                self.scopes.push(HashMap::new());
                self.visit(tree, *condition);
                self.visit(tree, *truthy);
                self.scopes.pop();
                self.visit(tree, *falsy);
            }

            NodeKind::While {
                condition, body, ..
            } => {
                self.visit(tree, *condition);
                self.scoped(tree, *body);
            }

            NodeKind::Repeat {
                body, condition, ..
            } => {
                self.scopes.push(HashMap::new());
                self.visit(tree, *body);
                self.visit(tree, *condition);
                self.scopes.pop();
            }

            NodeKind::NumericFor {
                binding,
                start,
                end,
                step,
                body,
                ..
            } => {
                self.visit(tree, *start);
                self.visit(tree, *end);

                if let Some(step) = step {
                    self.visit(tree, *step);
                }

                self.scopes.push(HashMap::new());
                self.bind(tree, *binding);
                self.visit(tree, *body);
                self.scopes.pop();
            }

            NodeKind::GenericFor {
                bindings,
                values,
                body,
                ..
            } => {
                for value in tree.list(values) {
                    self.visit(tree, value.node);
                }

                self.scopes.push(HashMap::new());

                for binding in tree.list(bindings) {
                    self.bind(tree, binding.node);
                }

                self.visit(tree, *body);
                self.scopes.pop();
            }

            NodeKind::Do { body, .. } => self.scoped(tree, *body),

            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conditional_bindings_keep_require_scope() {
        for keyword in ["local", "const"] {
            for suffix in ["", "\nlocal ="] {
                let source = format!(
                    "local path = './outer'\n\
                     local result = if {keyword} path = path .. '/inner' then require(path) else require(path)\n\
                     require(path){suffix}"
                );

                let extraction = extract(&source, None, Ok(None));

                assert_eq!(
                    extraction.diagnostics.is_empty(),
                    suffix.is_empty(),
                    "{source}"
                );

                let requests: Vec<_> = extraction
                    .sites
                    .iter()
                    .map(|site| match &site.request {
                        Some(Request::String(path)) => Some(path.as_str()),
                        _ => None,
                    })
                    .collect();

                assert_eq!(
                    requests,
                    [Some("./outer/inner"), Some("./outer"), Some("./outer")],
                    "{source}"
                );
            }
        }
    }

    #[test]
    fn conditional_bindings_keep_write_scope() {
        for keyword in ["local", "const"] {
            let source = format!(
                "local target = false\n\
                 local result = if {keyword} target = false then target else (function() target = true end)()\n\
                 target = true"
            );

            let tree = vermis::parse(source.as_bytes());
            assert!(tree.diagnostics.is_empty(), "{:?}", tree.diagnostics);

            let writes = Writes::analyze(&tree);
            let outer = source.find("target").unwrap();
            let inner = source.match_indices("target = false").nth(1).unwrap().0;

            assert!(writes.assigned(outer), "{source}");
            assert!(!writes.assigned(inner), "{source}");
        }
    }

    #[test]
    fn require_values_keep_lexical_write_ownership() {
        let cases = [
            (
                "local path,other='./a',1\npath,other='./b',2\nrequire(path)",
                None,
            ),
            (
                "local path='./a'\ndo local path='./b' path='./c' end\nrequire(path)",
                Some("./a"),
            ),
            (
                "local path='./a'\ndo local path=path require(path) end",
                Some("./a"),
            ),
            (
                "local path='./a'\nlocal function change(path) path='./b' end\nrequire(path)",
                Some("./a"),
            ),
            (
                "local path='./a'\nlocal function change() path..='b' end\nrequire(path)",
                None,
            ),
        ];

        for (source, expected) in cases {
            let extraction = extract(source, None, Ok(None));
            assert!(extraction.diagnostics.is_empty(), "{source}");

            let requests: Vec<_> = extraction
                .sites
                .iter()
                .map(|site| match &site.request {
                    Some(Request::String(path)) => Some(path.as_str()),
                    _ => None,
                })
                .collect();

            assert_eq!(requests, [expected], "{source}");
        }
    }

    #[test]
    fn function_declarations_invalidate_require_values() {
        let cases: &[(&str, &[Option<&str>])] = &[
            (
                "local path='./a'\nfunction path() end\nrequire(path)",
                &[None],
            ),
            (
                "function require(value) return value end\nrequire('./a')",
                &[None],
            ),
            (
                "local loader=require\nfunction loader() end\nloader('./a')",
                &[],
            ),
        ];

        for &(source, expected) in cases {
            let extraction = extract(source, None, Ok(None));
            assert!(extraction.diagnostics.is_empty(), "{source}");

            let requests: Vec<_> = extraction
                .sites
                .iter()
                .map(|site| match &site.request {
                    Some(Request::String(path)) => Some(path.as_str()),
                    _ => None,
                })
                .collect();

            assert_eq!(requests, expected, "{source}");
        }
    }

    #[test]
    fn function_name_writes_keep_lexical_and_member_ownership() {
        let cases = [
            (
                "local target=function() end\nfunction target() end",
                true,
                false,
            ),
            ("local target={}\nfunction target.member() end", false, true),
            ("local target={}\nfunction target:method() end", false, true),
            (
                "local target={member={}}\nfunction target.member.nested() end",
                false,
                false,
            ),
            (
                "local target=false\ndo local target=false\nfunction target() end\nend",
                false,
                false,
            ),
            (
                "local target=false\nlocal function target() end",
                false,
                false,
            ),
        ];

        for (source, assigned, mutated) in cases {
            let tree = vermis::parse(source.as_bytes());
            assert!(tree.diagnostics.is_empty(), "{:?}", tree.diagnostics);
            let target = source.find("target").unwrap();

            let mut writes = Writes::new(None);
            writes.visit(&tree, tree.root);
            assert_eq!(writes.assigned(target), assigned, "{source}");
            assert!(!writes.mutated(target), "{source}");

            let writes = Writes::analyze(&tree);
            assert_eq!(writes.assigned(target), assigned, "{source}");
            assert_eq!(writes.mutated(target), mutated, "{source}");
        }
    }

    #[test]
    fn variadic_annotations_extract_requires_in_parameter_scope() {
        let cases: &[(&str, &[&str])] = &[
            (
                "local function f(...: typeof(require('./a'))) end",
                &["./a"],
            ),
            (
                "local loader=require\nlocal function f(...: typeof(loader('./a'))) end",
                &["./a"],
            ),
            (
                "local function f(require, ...: typeof(require('./a'))) end",
                &[],
            ),
            (
                "local function f(value: typeof(require('./a'))) end",
                &["./a"],
            ),
        ];

        for &(source, expected) in cases {
            let extraction = extract(source, None, Ok(None));
            assert!(extraction.diagnostics.is_empty(), "{source}");

            let requests: Vec<_> = extraction
                .sites
                .iter()
                .map(|site| match &site.request {
                    Some(Request::String(path)) => path.as_str(),
                    _ => panic!("expected a static string require: {source}"),
                })
                .collect();

            assert_eq!(requests, expected, "{source}");
        }
    }
}
