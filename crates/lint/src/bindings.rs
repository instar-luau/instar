use crate::{Options, Reason};

use std::{
    collections::{BTreeMap, BTreeSet},
    time::Instant,
};

use vermis::{
    token::{Keyword, TokenKind},
    tree::{NodeIndex, NodeKind, NodeList, Tree},
};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Category {
    Local,
    Constant,
    Parameter,
    Loop,
    Function,
}

pub(super) struct Binding {
    pub(super) name: NodeIndex,
    pub(super) category: Category,
    pub(super) value: Option<NodeIndex>,
    pub(super) read: bool,
    pub(super) assigned: bool,
    pub(super) mutated: bool,
}

#[derive(Default)]
pub(super) struct Bindings {
    pub(super) declarations: BTreeMap<usize, Binding>,
    pub(super) references: BTreeMap<usize, usize>,
    pub(super) globals: BTreeSet<usize>,
    pub(super) global_writes: BTreeSet<String>,
    pub(super) arities: BTreeMap<usize, (usize, bool)>,
    scopes: Vec<BTreeMap<String, usize>>,
    functions: BTreeMap<usize, (usize, bool)>,
    limits: Option<(Options, Instant)>,
    pub(super) interruption: Option<Reason>,
}

impl Bindings {
    pub(super) fn analyze(tree: &Tree<'_>, options: &Options, started: Instant) -> Self {
        let mut bindings = Self {
            scopes: vec![BTreeMap::new()],
            limits: Some((options.clone(), started)),
            ..Self::default()
        };

        bindings.visit(tree, tree.root);

        bindings
    }

    pub(super) fn global(&self, tree: &Tree<'_>, node: NodeIndex) -> bool {
        self.globals.contains(&node.get()) && !self.global_writes.contains(&text(tree, node))
    }

    pub(super) fn declaration(&self, node: NodeIndex) -> Option<&Binding> {
        self.references
            .get(&node.get())
            .and_then(|index| self.declarations.get(index))
    }

    fn resolve(&self, tree: &Tree<'_>, node: NodeIndex) -> Option<usize> {
        self.scopes
            .iter()
            .rev()
            .find_map(|scope| scope.get(&text(tree, node)).copied())
    }

    fn bind_name(
        &mut self,
        tree: &Tree<'_>,
        name: NodeIndex,
        category: Category,
        value: Option<NodeIndex>,
    ) {
        let index = name.get();

        self.scopes
            .last_mut()
            .expect("binding scope")
            .insert(text(tree, name), index);

        self.declarations.insert(
            index,
            Binding {
                name,
                category,
                value,
                read: false,
                assigned: false,
                mutated: false,
            },
        );

        if let Some(value) = value
            && let NodeKind::Function { parameters, .. } = &tree.node(value).kind
            && let NodeKind::Parameters { parameters, .. } = &tree.node(*parameters).kind
        {
            let parameters = tree.list(parameters);

            let variadic = parameters.iter().any(|parameter| {
                matches!(tree.node(parameter.node).kind, NodeKind::Variadic { .. })
            });

            self.functions
                .insert(index, (parameters.len() - usize::from(variadic), variadic));
        }
    }

    fn bind(
        &mut self,
        tree: &Tree<'_>,
        node: NodeIndex,
        category: Category,
        value: Option<NodeIndex>,
    ) {
        match &tree.node(node).kind {
            NodeKind::Binding {
                name, annotation, ..
            } => {
                if let Some(annotation) = annotation {
                    self.visit(tree, *annotation);
                }

                self.bind_name(tree, *name, category, value);
            }

            NodeKind::Variadic {
                annotation: Some(annotation),
                ..
            } => self.visit(tree, *annotation),

            _ => {}
        }
    }

    fn read(&mut self, tree: &Tree<'_>, node: NodeIndex) {
        if let Some(index) = self.resolve(tree, node) {
            self.references.insert(node.get(), index);

            self.declarations
                .get_mut(&index)
                .expect("resolved binding")
                .read = true;

            if let Some(arity) = self.functions.get(&index) {
                self.arities.insert(node.get(), *arity);
            }
        } else {
            self.globals.insert(node.get());
        }
    }

    fn write(&mut self, tree: &Tree<'_>, node: NodeIndex, reading: bool) {
        if matches!(tree.node(node).kind, NodeKind::Name { .. }) {
            if let Some(index) = self.resolve(tree, node) {
                self.references.insert(node.get(), index);
                let binding = self.declarations.get_mut(&index).expect("resolved write");
                binding.assigned = true;
                binding.read |= reading;
                self.functions.remove(&index);
            } else {
                self.globals.insert(node.get());
                self.global_writes.insert(text(tree, node));
            }
        } else {
            self.mutate(tree, node);
            self.visit(tree, node);
        }
    }

    fn mutate(&mut self, tree: &Tree<'_>, node: NodeIndex) {
        let (NodeKind::Field { receiver, .. }
        | NodeKind::Index { receiver, .. }
        | NodeKind::Group {
            expression: receiver,
            ..
        }) = &tree.node(node).kind
        else {
            return;
        };

        if matches!(tree.node(*receiver).kind, NodeKind::Name { .. }) {
            if let Some(index) = self.resolve(tree, *receiver) {
                self.declarations
                    .get_mut(&index)
                    .expect("resolved mutation")
                    .mutated = true;
            } else {
                self.global_writes.insert(text(tree, *receiver));
            }
        } else {
            self.mutate(tree, *receiver);
        }
    }

    fn scoped(&mut self, tree: &Tree<'_>, node: NodeIndex) {
        self.scopes.push(BTreeMap::new());
        self.visit(tree, node);
        self.scopes.pop();
    }

    fn local(
        &mut self,
        tree: &Tree<'_>,
        bindings: &NodeList,
        values: &NodeList,
        category: Category,
    ) {
        for value in tree.list(values) {
            self.visit(tree, value.node);
        }

        for (index, binding) in tree.list(bindings).iter().enumerate() {
            self.bind(
                tree,
                binding.node,
                category,
                tree.list(values).get(index).map(|value| value.node),
            );
        }
    }

    fn function(&mut self, tree: &Tree<'_>, node: NodeIndex) {
        let NodeKind::Function {
            prefix,
            name,
            parameters,
            body,
            returns,
            ..
        } = &tree.node(node).kind
        else {
            return;
        };

        if prefix.is_some_and(|prefix| tree.token(prefix).bytes(tree.source) == b"type") {
            return;
        }

        let local = prefix.is_some_and(|prefix| {
            tree.token(prefix).kind == TokenKind::Keyword(Keyword::Local)
                || tree.token(prefix).bytes(tree.source) == b"const"
        });

        if let Some(name) = name {
            if local {
                self.bind_name(tree, *name, Category::Function, Some(node));
            } else if let NodeKind::FunctionName { path, method, .. } = &tree.node(*name).kind {
                if let Some(first) = tree.list(path).first() {
                    if tree.list(path).len() == 1 && method.is_none() {
                        self.write(tree, first.node, false);
                    } else {
                        self.read(tree, first.node);

                        if let Some(index) = self.resolve(tree, first.node) {
                            self.declarations
                                .get_mut(&index)
                                .expect("function receiver")
                                .mutated = true;
                        } else {
                            self.global_writes.insert(text(tree, first.node));
                        }
                    }
                }
            } else {
                self.write(tree, *name, false);
            }
        }

        self.scopes.push(BTreeMap::new());

        if let Some(name) = name
            && let NodeKind::FunctionName {
                method: Some(method),
                ..
            } = &tree.node(*name).kind
        {
            self.scopes
                .last_mut()
                .expect("method scope")
                .insert("self".to_owned(), method.get());

            self.declarations.insert(
                method.get(),
                Binding {
                    name: *method,
                    category: Category::Parameter,
                    value: None,
                    read: true,
                    assigned: false,
                    mutated: false,
                },
            );
        }

        if let NodeKind::Parameters { parameters, .. } = &tree.node(*parameters).kind {
            for parameter in tree.list(parameters) {
                self.bind(tree, parameter.node, Category::Parameter, None);
            }
        }

        if let Some(returns) = returns {
            self.visit(tree, *returns);
        }

        if let Some(body) = body {
            self.visit(tree, *body);
        }

        self.scopes.pop();
    }

    fn visit(&mut self, tree: &Tree<'_>, node: NodeIndex) {
        if self.interruption.is_some() {
            return;
        }

        if let Some((options, started)) = &self.limits {
            self.interruption = if options.cancellation.requested() {
                Some(Reason::Cancelled)
            } else if started.elapsed() >= options.timeout {
                Some(Reason::Timeout)
            } else {
                None
            };

            if self.interruption.is_some() {
                return;
            }
        }

        match &tree.node(node).kind {
            NodeKind::Name { .. } => self.read(tree, node),

            NodeKind::Local {
                bindings, values, ..
            } => self.local(tree, bindings, values, Category::Local),

            NodeKind::Constant {
                bindings, values, ..
            } => self.local(tree, bindings, values, Category::Constant),

            NodeKind::Assignment {
                targets, values, ..
            } => {
                for value in tree.list(values) {
                    self.visit(tree, value.node);
                }

                for target in tree.list(targets) {
                    self.write(tree, target.node, false);
                }
            }

            NodeKind::CompoundAssignment { target, value, .. } => {
                self.visit(tree, *value);
                self.write(tree, *target, true);
            }

            NodeKind::Function { .. } => self.function(tree, node),
            NodeKind::If { .. } | NodeKind::Conditional { .. } => self.branch(tree, node),

            NodeKind::While { .. }
            | NodeKind::Repeat { .. }
            | NodeKind::NumericFor { .. }
            | NodeKind::GenericFor { .. } => self.looping(tree, node),

            NodeKind::Do { body, .. } => self.scoped(tree, *body),

            NodeKind::Field { receiver, .. } | NodeKind::MethodCall { receiver, .. } => {
                self.visit(tree, *receiver);

                if let NodeKind::MethodCall { arguments, .. } = &tree.node(node).kind {
                    self.visit(tree, *arguments);
                }
            }

            NodeKind::TableField {
                key,
                opening,
                value,
                ..
            } => {
                if opening.is_some()
                    && let Some(key) = key
                {
                    self.visit(tree, *key);
                }

                self.visit(tree, *value);
            }

            NodeKind::TypeName {
                namespace,
                arguments,
                ..
            } => {
                if let Some(namespace) = namespace {
                    self.read(tree, *namespace);
                }

                if let Some(arguments) = arguments {
                    self.visit(tree, *arguments);
                }
            }

            NodeKind::TypeAlias { annotation, .. } => self.visit(tree, *annotation),

            NodeKind::TypeParameter { annotation, .. } | NodeKind::TypeField { annotation, .. } => {
                self.visit(tree, *annotation);
            }

            NodeKind::Generic { default, .. } => {
                if let Some(default) = default {
                    self.visit(tree, *default);
                }
            }

            NodeKind::FunctionName { .. }
            | NodeKind::Attributes { .. }
            | NodeKind::Class { .. } => {}

            _ => {
                for child in tree.children(node) {
                    self.visit(tree, child);
                }
            }
        }
    }

    fn branch(&mut self, tree: &Tree<'_>, node: NodeIndex) {
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
                self.scopes.push(BTreeMap::new());
                self.visit(tree, *condition);
                self.visit(tree, *truthy);
                self.scopes.pop();
                self.visit(tree, *falsy);
            }

            _ => {}
        }
    }

    fn looping(&mut self, tree: &Tree<'_>, node: NodeIndex) {
        match &tree.node(node).kind {
            NodeKind::While {
                condition, body, ..
            } => {
                self.scopes.push(BTreeMap::new());
                self.visit(tree, *condition);
                self.visit(tree, *body);
                self.scopes.pop();
            }

            NodeKind::Repeat {
                body, condition, ..
            } => {
                self.scopes.push(BTreeMap::new());
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

                self.scopes.push(BTreeMap::new());
                self.bind(tree, *binding, Category::Loop, None);
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

                self.scopes.push(BTreeMap::new());

                for binding in tree.list(bindings) {
                    self.bind(tree, binding.node, Category::Loop, None);
                }

                self.visit(tree, *body);
                self.scopes.pop();
            }

            _ => {}
        }
    }
}

fn text(tree: &Tree<'_>, node: NodeIndex) -> String {
    String::from_utf8_lossy(tree.text(node)).into_owned()
}
