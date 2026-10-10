use std::{collections::BTreeMap, io};

use instar_luau::{
    ImportIdentity, ImportRequest, ImportSite, InstancePath, InstanceRoot, InstanceStep,
};

use instar_shared::{Completion, Location, Options, SourceSnapshot, Span};
use instar_syntax::{BindingResult, bindings, literal};

use instar_syntax::{
    token::{Keyword, Symbol, TokenKind},
    tree::{NodeIndex, NodeKind, NodeList, Tree},
};

#[derive(Clone)]
enum Value {
    Other,
    Loader(bool),
    String(String),
    Boolean(bool),
    Instance(InstanceRoot, Vec<InstanceStep>),
}

struct Extractor<'tree, 'source> {
    tree: &'tree Tree<'source>,
    operation: &'tree Options,
    snapshot: &'tree SourceSnapshot,
    bindings: BindingResult,
    values: BTreeMap<usize, Value>,
    sites: Vec<ImportSite>,
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
            "script" => Value::Instance(InstanceRoot::Script, Vec::new()),
            "game" => Value::Instance(InstanceRoot::Game, Vec::new()),
            "workspace" => Value::Instance(InstanceRoot::Workspace, Vec::new()),
            _ => Value::Other,
        }
    }

    fn value(&self, node: NodeIndex) -> Value {
        match &self.tree.node(node).kind {
            NodeKind::Name { .. } => self.lookup(node),

            NodeKind::String { .. } => {
                literal::string(self.tree, node).map_or(Value::Other, Value::String)
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
                    ("GetService", 1) => InstanceStep::Service(name),

                    ("WaitForChild", 1 | 2) | ("FindFirstChild", 1) => InstanceStep::Child {
                        name,
                        recursive: false,
                    },

                    ("FindFirstChild", 2) => {
                        let Value::Boolean(recursive) = self.value(arguments[1].node) else {
                            return Value::Other;
                        };

                        InstanceStep::Child { name, recursive }
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

        steps.push(if name == "Parent" {
            InstanceStep::Parent
        } else {
            InstanceStep::Field(name.to_owned())
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
                    Value::String(value) => ImportRequest::Static(value),

                    Value::Instance(root, steps) => {
                        ImportRequest::Instance(InstancePath { root, steps })
                    }

                    _ => ImportRequest::Nonconstant,
                }
            } else {
                ImportRequest::Nonconstant
            };

            let call = self.tree.node(node).span;

            self.sites.push(ImportSite {
                identity: ImportIdentity(node.get() as u64),
                call: Location {
                    source: self.snapshot.version,
                    span: Span {
                        start: call.start,
                        end: call.end,
                    },
                },
                argument: Location {
                    source: self.snapshot.version,
                    span: Span {
                        start: range.start,
                        end: range.end,
                    },
                },
                request,
            });
        }

        for child in self.tree.children(node) {
            self.visit(child);
        }
    }

    fn visit(&mut self, node: NodeIndex) {
        if self.operation.interrupted().is_some() {
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
    tree: &instar_syntax::Tree<'_>,
    options: &Options,
) -> io::Result<Vec<ImportSite>> {
    options.check()?;
    let bindings = bindings(tree, options);

    if bindings.completion != Completion::Complete {
        return Err(bindings.completion.error());
    }

    let mut extractor = Extractor {
        tree: tree.syntax(),
        operation: options,
        snapshot: tree.snapshot(),
        bindings,
        values: BTreeMap::new(),
        sites: Vec::new(),
    };

    extractor.visit(tree.syntax().root);
    options.check()?;
    extractor.sites.sort_by_key(|site| site.argument.span.start);

    Ok(extractor.sites)
}
