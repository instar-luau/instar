use std::collections::{BTreeMap, BTreeSet};

use vermis::{
    token::{Keyword, Symbol, TokenKind},
    tree::{NodeIndex, NodeKind, NodeList, Tree},
};

use crate::{
    graph::{Problem, Site},
    resolve::{Request, Root, Step},
    source::Failure,
};

#[derive(Clone)]
enum Value {
    Other,
    Loader(bool),
    String(String),
    Boolean(bool),
    Instance(Root, Vec<Step>),
}

#[derive(Clone)]
struct Binding {
    declaration: usize,
    value: Value,
}

struct Extractor<'tree, 'source> {
    tree: &'tree Tree<'source>,
    scopes: Vec<BTreeMap<String, Binding>>,
    writes: BTreeSet<usize>,
    globals: BTreeSet<String>,
    writing: bool,
    revision: u64,
    sites: Vec<Site>,
}

pub(crate) fn extract(source: &str, revision: u64) -> (Vec<Site>, Vec<Problem>) {
    let tree = vermis::parse(source.as_bytes());

    let mut extractor = Extractor {
        tree: &tree,
        scopes: vec![BTreeMap::new()],
        writes: BTreeSet::new(),
        globals: BTreeSet::new(),
        writing: true,
        revision,
        sites: Vec::new(),
    };

    extractor.visit(tree.root);
    extractor.scopes = vec![BTreeMap::new()];
    extractor.writing = false;
    extractor.visit(tree.root);
    extractor.sites.sort_by_key(|site| site.range);

    let problems = tree
        .diagnostics
        .iter()
        .map(|diagnostic| Problem {
            range: [diagnostic.span.start, diagnostic.span.end],
            message: diagnostic.message.to_owned(),
        })
        .collect();

    (extractor.sites, problems)
}

impl Extractor<'_, '_> {
    fn name(&self, node: NodeIndex) -> String {
        String::from_utf8_lossy(self.tree.text(node)).into_owned()
    }

    fn lookup(&self, node: NodeIndex) -> Value {
        let name = self.name(node);

        for scope in self.scopes.iter().rev() {
            if let Some(binding) = scope.get(&name) {
                return if self.writes.contains(&binding.declaration) {
                    Value::Other
                } else {
                    binding.value.clone()
                };
            }
        }

        if name == "require" {
            return Value::Loader(!self.globals.contains(&name));
        }

        if self.globals.contains(&name) {
            return Value::Other;
        }

        match name.as_str() {
            "script" => Value::Instance(Root::Script, Vec::new()),
            "game" => Value::Instance(Root::Game, Vec::new()),
            "workspace" => Value::Instance(Root::Workspace, Vec::new()),
            _ => Value::Other,
        }
    }

    fn value(&self, node: NodeIndex) -> Value {
        match &self.tree.node(node).kind {
            NodeKind::Name { .. } => self.lookup(node),

            NodeKind::String { .. } => {
                instar_syntax::literal::string(self.tree, node).map_or(Value::Other, Value::String)
            }

            NodeKind::Boolean { token } => {
                Value::Boolean(self.tree.token(*token).kind == TokenKind::Keyword(Keyword::True))
            }

            NodeKind::Group { expression, .. }
            | NodeKind::Assertion { expression, .. }
            | NodeKind::Instantiate { expression, .. } => self.value(*expression),

            NodeKind::Binary {
                left,
                operator,
                right,
            } if self.tree.token(*operator).kind == TokenKind::Symbol(Symbol::Concatenate) => {
                if let (Value::String(left), Value::String(right)) =
                    (self.value(*left), self.value(*right))
                {
                    Value::String(left + &right)
                } else {
                    Value::Other
                }
            }

            NodeKind::Field { receiver, name, .. } => self.field(*receiver, &self.name(*name)),

            NodeKind::Index { receiver, key, .. } => {
                if let Value::String(name) = self.value(*key) {
                    self.field(*receiver, &name)
                } else {
                    Value::Other
                }
            }

            NodeKind::MethodCall {
                receiver,
                method,
                arguments,
                ..
            } => {
                let Value::Instance(root, mut steps) = self.value(*receiver) else {
                    return Value::Other;
                };

                let NodeKind::Arguments { values, .. } = &self.tree.node(*arguments).kind else {
                    return Value::Other;
                };

                let arguments = self.tree.list(values);

                let Some(first) = arguments.first() else {
                    return Value::Other;
                };

                let Value::String(name) = self.value(first.node) else {
                    return Value::Other;
                };

                let step = match (self.name(*method).as_str(), arguments.len()) {
                    ("GetService", 1) => Step::Service(name),

                    ("WaitForChild", 1 | 2) | ("FindFirstChild", 1) => Step::Child {
                        name,
                        recursive: false,
                    },

                    ("FindFirstChild", 2) => {
                        let Value::Boolean(recursive) = self.value(arguments[1].node) else {
                            return Value::Other;
                        };

                        Step::Child { name, recursive }
                    }

                    _ => return Value::Other,
                };

                steps.push(step);

                Value::Instance(root, steps)
            }

            _ => Value::Other,
        }
    }

    fn field(&self, receiver: NodeIndex, name: &str) -> Value {
        let Value::Instance(root, mut steps) = self.value(receiver) else {
            return Value::Other;
        };

        if matches!(
            name,
            "Name" | "ClassName" | "GetService" | "WaitForChild" | "FindFirstChild"
        ) {
            return Value::Other;
        }

        steps.push(if name == "Parent" {
            Step::Parent
        } else {
            Step::Child {
                name: name.to_owned(),
                recursive: false,
            }
        });

        Value::Instance(root, steps)
    }

    fn bind_name(&mut self, name: NodeIndex, value: Value) {
        let declaration = self.tree.node(name).span.start;
        let name = self.name(name);

        if let Some(scope) = self.scopes.last_mut() {
            scope.insert(name, Binding { declaration, value });
        }
    }

    fn bind(&mut self, node: NodeIndex, value: Value) {
        match &self.tree.node(node).kind {
            NodeKind::Binding {
                name, annotation, ..
            } => {
                if let Some(annotation) = annotation {
                    self.visit(*annotation);
                }

                self.bind_name(*name, value);
            }

            NodeKind::Variadic {
                annotation: Some(annotation),
                ..
            } => self.visit(*annotation),

            _ => {}
        }
    }

    fn assign(&mut self, node: NodeIndex) {
        if !self.writing || !matches!(self.tree.node(node).kind, NodeKind::Name { .. }) {
            return;
        }

        let name = self.name(node);

        if let Some(binding) = self.scopes.iter().rev().find_map(|scope| scope.get(&name)) {
            self.writes.insert(binding.declaration);
        } else {
            self.globals.insert(name);
        }
    }

    fn scoped(&mut self, node: NodeIndex) {
        self.scopes.push(BTreeMap::new());
        self.visit(node);
        self.scopes.pop();
    }

    fn local(&mut self, bindings: &NodeList, values: &NodeList) {
        let values = self.tree.list(values);

        let evaluated = values
            .iter()
            .map(|entry| self.value(entry.node))
            .collect::<Vec<_>>();

        for entry in values {
            self.visit(entry.node);
        }

        for (index, entry) in self.tree.list(bindings).iter().enumerate() {
            self.bind(
                entry.node,
                evaluated.get(index).cloned().unwrap_or(Value::Other),
            );
        }
    }

    fn function(&mut self, node: NodeIndex) {
        let NodeKind::Function {
            prefix,
            name,
            parameters,
            returns,
            body,
            ..
        } = &self.tree.node(node).kind
        else {
            return;
        };

        let local = prefix.is_some_and(|prefix| {
            self.tree.token(prefix).kind == TokenKind::Keyword(Keyword::Local)
                || self.tree.token(prefix).bytes(self.tree.source) == b"const"
        });

        let mut method = false;

        if let Some(name) = name {
            if local {
                self.bind_name(*name, Value::Other);
            } else if let NodeKind::FunctionName {
                path,
                method: member,
                ..
            } = &self.tree.node(*name).kind
            {
                method = member.is_some();
                let path = self.tree.list(path);

                if path.len() == 1 && !method {
                    self.assign(path[0].node);
                }
            } else {
                self.assign(*name);
            }
        }

        self.scopes.push(BTreeMap::new());

        if method && let Some(scope) = self.scopes.last_mut() {
            scope.insert(
                "self".to_owned(),
                Binding {
                    declaration: self.tree.node(node).span.start,
                    value: Value::Other,
                },
            );
        }

        if let NodeKind::Parameters { parameters, .. } = &self.tree.node(*parameters).kind {
            for parameter in self.tree.list(parameters) {
                self.bind(parameter.node, Value::Other);
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

    fn call(&mut self, node: NodeIndex, callee: NodeIndex, arguments: NodeIndex) {
        if !self.writing
            && let Value::Loader(stable) = self.value(callee)
            && let NodeKind::Arguments { values, .. } = &self.tree.node(arguments).kind
        {
            let values = self.tree.list(values);
            let argument = values.first().map_or(arguments, |entry| entry.node);
            let range = self.tree.node(argument).span;

            let request = if stable && values.len() == 1 {
                match self.value(argument) {
                    Value::String(value) => Some(Request::String(value)),
                    Value::Instance(root, steps) => Some(Request::Instance { root, steps }),
                    _ => None,
                }
            } else {
                None
            };

            let failure = request.is_none().then(|| Failure {
                kind: std::io::ErrorKind::InvalidData,
                message: if values.len() == 1 {
                    "require target is not a statically known string or instance"
                } else {
                    "require expects one argument"
                }
                .to_owned(),
            });

            let call = self.tree.node(node).span;

            self.sites.push(Site {
                revision: self.revision,
                range: [range.start, range.end],
                call: [call.start, call.end],
                request,
                target: None,
                failure,
                inputs: BTreeSet::new(),
            });
        }

        for child in self.tree.children(node) {
            self.visit(child);
        }
    }

    fn control(&mut self, node: NodeIndex) {
        let tree = self.tree;

        match &tree.node(node).kind {
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
                self.scopes.push(BTreeMap::new());
                self.visit(*condition);
                self.visit(*truthy);
                self.scopes.pop();
                self.visit(*falsy);
            }

            NodeKind::While {
                condition, body, ..
            } => {
                self.scopes.push(BTreeMap::new());
                self.visit(*condition);
                self.visit(*body);
                self.scopes.pop();
            }

            NodeKind::Repeat {
                body, condition, ..
            } => {
                self.scopes.push(BTreeMap::new());
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

                self.scopes.push(BTreeMap::new());
                self.bind(*binding, Value::Other);
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

                self.scopes.push(BTreeMap::new());

                for binding in tree.list(bindings) {
                    self.bind(binding.node, Value::Other);
                }

                self.visit(*body);
                self.scopes.pop();
            }

            _ => {}
        }
    }

    fn visit(&mut self, node: NodeIndex) {
        let tree = self.tree;

        match &tree.node(node).kind {
            NodeKind::Local {
                bindings, values, ..
            }
            | NodeKind::Constant {
                bindings, values, ..
            } => self.local(bindings, values),

            NodeKind::Assignment { targets, .. } => {
                for target in tree.list(targets) {
                    self.assign(target.node);
                }

                for child in tree.children(node) {
                    self.visit(child);
                }
            }

            NodeKind::CompoundAssignment { target, .. } => {
                self.assign(*target);

                for child in tree.children(node) {
                    self.visit(child);
                }
            }

            NodeKind::Function {
                prefix: Some(prefix),
                ..
            } if tree.token(*prefix).bytes(tree.source) == b"type" => {}

            NodeKind::Function { .. } => self.function(node),

            NodeKind::If { .. }
            | NodeKind::Conditional { .. }
            | NodeKind::While { .. }
            | NodeKind::Repeat { .. }
            | NodeKind::NumericFor { .. }
            | NodeKind::GenericFor { .. } => self.control(node),

            NodeKind::Do { body, .. } => self.scoped(*body),
            NodeKind::Call { callee, arguments } => self.call(node, *callee, *arguments),

            _ => {
                for child in tree.children(node) {
                    self.visit(child);
                }
            }
        }
    }
}
