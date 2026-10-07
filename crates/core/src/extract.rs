use std::{
    collections::{BTreeMap, BTreeSet},
    io,
    time::Instant,
};

use instar_analysis::Options;
use instar_syntax::bindings::Bindings;

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

struct Extractor<'tree, 'source> {
    tree: &'tree Tree<'source>,
    operation: Option<&'tree (Options, Instant)>,
    bindings: Bindings,
    values: BTreeMap<usize, Value>,
    revision: u64,
    sites: Vec<Site>,
}

impl Extractor<'_, '_> {
    fn name(&self, node: NodeIndex) -> String {
        String::from_utf8_lossy(self.tree.text(node)).into_owned()
    }

    fn lookup(&self, node: NodeIndex) -> Value {
        let name = self.name(node);

        if let Some(binding) = self.bindings.declaration(node) {
            return if binding.assigned {
                Value::Other
            } else {
                self.values
                    .get(&binding.name.get())
                    .cloned()
                    .unwrap_or(Value::Other)
            };
        }

        if !self.bindings.globals.contains(&node.get()) {
            return Value::Other;
        }

        if name == "require" {
            return Value::Loader(!self.bindings.global_assignments.contains(&name));
        }

        if self.bindings.global_assignments.contains(&name) {
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

    fn bind(&mut self, node: NodeIndex, value: Value) {
        match &self.tree.node(node).kind {
            NodeKind::Binding {
                name, annotation, ..
            } => {
                if let Some(annotation) = annotation {
                    self.visit(*annotation);
                }

                self.values.insert(name.get(), value);
            }

            NodeKind::Variadic {
                annotation: Some(annotation),
                ..
            } => self.visit(*annotation),

            _ => {}
        }
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

    fn call(&mut self, node: NodeIndex, callee: NodeIndex, arguments: NodeIndex) {
        if let Value::Loader(stable) = self.value(callee)
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

    fn visit(&mut self, node: NodeIndex) {
        if self
            .operation
            .is_some_and(|(options, started)| options.interrupted(*started).is_some())
        {
            return;
        }

        let tree = self.tree;

        match &tree.node(node).kind {
            NodeKind::Local {
                bindings, values, ..
            }
            | NodeKind::Constant {
                bindings, values, ..
            } => self.local(bindings, values),

            NodeKind::Function {
                prefix: Some(prefix),
                ..
            } if tree.token(*prefix).bytes(tree.source) == b"type" => {}

            NodeKind::Call { callee, arguments } => self.call(node, *callee, *arguments),

            _ => {
                for child in tree.children(node) {
                    self.visit(child);
                }
            }
        }
    }
}

pub(crate) fn extract(
    source: &str,
    revision: u64,
    operation: Option<&(Options, Instant)>,
) -> io::Result<(Vec<Site>, Vec<Problem>)> {
    if let Some((options, started)) = operation {
        options.check(*started)?;
    }

    let tree = vermis::parse(source.as_bytes());

    let bindings = Bindings::analyze(
        &tree,
        operation.map(|(options, started)| (options, *started)),
    );

    if let Some(reason) = bindings.interruption {
        return Err(reason.error());
    }

    let mut extractor = Extractor {
        tree: &tree,
        operation,
        bindings,
        values: BTreeMap::new(),
        revision,
        sites: Vec::new(),
    };

    extractor.visit(tree.root);

    if let Some((options, started)) = operation {
        options.check(*started)?;
    }

    extractor.sites.sort_by_key(|site| site.range);

    let problems = tree
        .diagnostics
        .iter()
        .map(|diagnostic| Problem {
            range: [diagnostic.span.start, diagnostic.span.end],
            message: diagnostic.message.to_owned(),
        })
        .collect();

    Ok((extractor.sites, problems))
}
