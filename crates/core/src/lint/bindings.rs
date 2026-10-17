use std::{
    collections::{BTreeMap, BTreeSet},
    mem,
};

use vermis::{
    token::{Keyword, TokenKind},
    tree::{NodeIndex, NodeKind, Tree},
};

use crate::lint::syntax::unwrap;

pub(super) struct Local {
    pub(super) name: String,
    pub(super) declaration: NodeIndex,
    pub(super) statement: NodeIndex,
    pub(super) value: Option<NodeIndex>,
    pub(super) category: &'static str,
    pub(super) function: NodeIndex,
    pub(super) shadow: Option<usize>,
    pub(super) reads: Vec<NodeIndex>,
    pub(super) type_reads: Vec<NodeIndex>,
    pub(super) assigned: bool,
    pub(super) initialized: bool,
    pub(super) mutated: bool,
}

#[derive(Default)]
pub(super) struct Global {
    pub(super) references: Vec<NodeIndex>,
    pub(super) writes: Vec<NodeIndex>,
    pub(super) functions: Vec<NodeIndex>,
    pub(super) read: bool,
    pub(super) read_before_write: bool,
    pub(super) module_assignment: bool,
    pub(super) definition: Option<NodeIndex>,
}

pub(super) struct Bindings {
    pub(super) locals: Vec<Local>,
    pub(super) globals: BTreeMap<String, Global>,
    pub(super) references: Vec<Option<usize>>,
    pub(super) reads: Vec<bool>,
    pub(super) parents: Vec<Option<NodeIndex>>,
    pub(super) loops: Vec<Option<NodeIndex>>,
    pub(super) writes: BTreeSet<String>,
    pub(super) declared: BTreeSet<String>,
}

impl Bindings {
    pub(super) fn new(tree: &Tree<'_>) -> Self {
        let count = tree.nodes.len();

        let mut collector = Collector {
            tree,
            bindings: Self {
                locals: Vec::new(),
                globals: BTreeMap::new(),
                references: vec![None; count],
                reads: vec![false; count],
                parents: vec![None; count],
                loops: vec![None; count],
                writes: BTreeSet::new(),
                declared: BTreeSet::new(),
            },
            scopes: vec![BTreeMap::new()],
            functions: Vec::new(),
            dominated: BTreeSet::new(),
            loop_body: None,
            annotation: false,
        };

        for index in 0..count {
            let parent = NodeIndex::new(index);

            for child in tree.children(parent) {
                collector.bindings.parents[child.get()] = Some(parent);
            }
        }

        collector.visit(tree.root);

        collector.bindings
    }

    pub(super) fn path(&self, tree: &Tree<'_>, node: NodeIndex) -> String {
        let node = unwrap(tree, node);

        match tree.node(node).kind {
            NodeKind::Name { .. } if self.references[node.get()].is_none() => text(tree, node),

            NodeKind::Field { receiver, name, .. } => {
                let root = self.path(tree, receiver);

                if root.is_empty() {
                    root
                } else {
                    format!("{root}.{}", text(tree, name))
                }
            }

            _ => String::new(),
        }
    }

    pub(super) fn global(&self, tree: &Tree<'_>, node: NodeIndex, expected: &str) -> bool {
        self.path(tree, node) == expected
            && !self.writes.contains("_G")
            && !self.writes.contains(expected)
            && !self
                .writes
                .contains(expected.split('.').next().unwrap_or(expected))
    }

    pub(super) fn value(&self, tree: &Tree<'_>, mut node: NodeIndex) -> NodeIndex {
        let mut visited = BTreeSet::new();

        loop {
            node = unwrap(tree, node);

            let Some(index) = self.references[node.get()] else {
                break;
            };

            let binding = &self.locals[index];

            if binding.assigned || !visited.insert(index) {
                break;
            }

            let Some(value) = binding.value else { break };

            node = value;
        }

        node
    }

    pub(super) fn same(&self, tree: &Tree<'_>, left: NodeIndex, right: NodeIndex) -> bool {
        let left = unwrap(tree, left);
        let right = unwrap(tree, right);

        matches!(tree.node(left).kind, NodeKind::Name { .. })
            && matches!(tree.node(right).kind, NodeKind::Name { .. })
            && self.references[left.get()] == self.references[right.get()]
            && tree.text(left) == tree.text(right)
    }

    pub(super) fn exported(&self, tree: &Tree<'_>, statement: NodeIndex) -> bool {
        self.parents[statement.get()]
            .is_some_and(|parent| matches!(tree.node(parent).kind, NodeKind::Export { .. }))
    }
}

struct Collector<'tree, 'source> {
    tree: &'tree Tree<'source>,
    bindings: Bindings,
    scopes: Vec<BTreeMap<String, usize>>,
    functions: Vec<NodeIndex>,
    dominated: BTreeSet<String>,
    loop_body: Option<NodeIndex>,
    annotation: bool,
}

impl Collector<'_, '_> {
    fn lookup(&self, name: &str) -> Option<usize> {
        self.scopes
            .iter()
            .rev()
            .find_map(|scope| scope.get(name).copied())
    }

    fn declare(
        &mut self,
        node: NodeIndex,
        statement: NodeIndex,
        value: Option<NodeIndex>,
        category: &'static str,
    ) {
        let name_node = if let NodeKind::Binding {
            name, annotation, ..
        } = self.tree.node(node).kind
        {
            if let Some(annotation) = annotation {
                self.annotation(annotation);
            }

            name
        } else {
            node
        };

        let name = text(self.tree, name_node);
        let shadow = self.lookup(&name);
        let index = self.bindings.locals.len();
        self.bindings.references[name_node.get()] = Some(index);
        self.bindings.references[node.get()] = Some(index);

        self.scopes
            .last_mut()
            .expect("internal error: missing lexical scope")
            .insert(name.clone(), index);

        self.bindings.locals.push(Local {
            name,
            declaration: name_node,
            statement,
            value,
            category,
            function: self.functions.last().copied().unwrap_or(self.tree.root),
            shadow,
            reads: Vec::new(),
            type_reads: Vec::new(),
            assigned: false,
            initialized: value.is_some(),
            mutated: false,
        });
    }

    fn reference(&mut self, node: NodeIndex, write: bool, read: bool) {
        let name = text(self.tree, node);

        if let Some(index) = self.lookup(&name) {
            self.bindings.references[node.get()] = Some(index);
            let binding = &mut self.bindings.locals[index];
            binding.assigned |= write;

            if read {
                if self.annotation {
                    binding.type_reads.push(node);
                } else {
                    binding.reads.push(node);
                }
            }
        } else if !self.annotation {
            let global = self.bindings.globals.entry(name.clone()).or_default();

            if global.references.is_empty() {
                global.functions.clone_from(&self.functions);
            } else {
                let common = global
                    .functions
                    .iter()
                    .zip(&self.functions)
                    .take_while(|(left, right)| left == right)
                    .count();

                global.functions.truncate(common);
            }

            global.references.push(node);
            global.read |= read;

            global.read_before_write |=
                read && !self.functions.is_empty() && !self.dominated.contains(&name);

            if write {
                global.writes.push(node);
                global.module_assignment |= self.functions.is_empty();
                self.bindings.writes.insert(name.clone());
                self.dominated.insert(name);
            }
        }

        self.bindings.reads[node.get()] |= read && !self.annotation;
    }

    fn mutate(&mut self, mut node: NodeIndex) {
        let mut visited = BTreeSet::new();

        loop {
            node = unwrap(self.tree, node);

            let Some(index) = self.bindings.references[node.get()] else {
                break;
            };

            if !visited.insert(index) {
                break;
            }

            self.bindings.locals[index].mutated = true;

            let Some(value) = self.bindings.locals[index].value else {
                break;
            };

            node = value;
        }
    }

    fn target(&mut self, node: NodeIndex, read: bool) {
        match self.tree.node(node).kind {
            NodeKind::Name { .. } => self.reference(node, true, read),

            NodeKind::Field { receiver, .. } | NodeKind::Index { receiver, .. } => {
                self.visit(node);
                self.mutate(receiver);
                let path = self.bindings.path(self.tree, receiver);

                if !path.is_empty() {
                    self.bindings.writes.insert(path);
                }
            }

            _ => self.visit(node),
        }
    }

    fn block(&mut self, node: NodeIndex) {
        if let NodeKind::Block { statements } = &self.tree.node(node).kind {
            for entry in self.tree.list(statements) {
                self.visit(entry.node);
            }
        }
    }

    fn annotation(&mut self, node: NodeIndex) {
        let previous = self.annotation;
        self.annotation = true;
        self.visit(node);
        self.annotation = previous;
    }

    fn visit(&mut self, node: NodeIndex) {
        let tree = self.tree;
        self.bindings.loops[node.get()] = self.loop_body;

        match &tree.node(node).kind {
            NodeKind::Name { .. } => self.reference(node, false, true),

            NodeKind::Block { .. } => {
                self.scopes.push(BTreeMap::new());
                self.block(node);
                self.scopes.pop();
            }

            NodeKind::Local {
                bindings, values, ..
            }
            | NodeKind::Constant {
                bindings, values, ..
            } => {
                for value in tree.list(values) {
                    self.visit(value.node);
                }

                let expanded = tree
                    .list(values)
                    .last()
                    .is_some_and(|value| expands(tree, value.node));

                for (index, binding) in tree.list(bindings).iter().enumerate() {
                    self.declare(
                        binding.node,
                        node,
                        tree.list(values).get(index).map(|entry| entry.node),
                        "local",
                    );

                    self.bindings
                        .locals
                        .last_mut()
                        .expect("internal error: missing declared local")
                        .initialized |= expanded;
                }
            }

            NodeKind::Assignment {
                targets, values, ..
            } => {
                for value in tree.list(values) {
                    self.visit(value.node);
                }

                for target in tree.list(targets) {
                    self.target(target.node, false);
                }
            }

            NodeKind::CompoundAssignment { target, value, .. } => {
                self.target(*target, true);
                self.visit(*value);
            }

            NodeKind::Function {
                prefix,
                name,
                parameters,
                returns,
                body: Some(body),
                ..
            } => {
                let local = self.bindings.exported(tree, node)
                    || prefix.is_some_and(|prefix| {
                        tree.token(prefix).kind == TokenKind::Keyword(Keyword::Local)
                            || tree.token(prefix).bytes(tree.source) == b"const"
                    });

                let mut method = None;

                if let Some(name) = name {
                    let first = if let NodeKind::FunctionName {
                        path,
                        method: member,
                        ..
                    } = &tree.node(*name).kind
                    {
                        method = *member;

                        tree.list(path).first().map(|entry| {
                            (entry.node, tree.list(path).len() == 1 && member.is_none())
                        })
                    } else {
                        Some((*name, true))
                    };

                    if let Some((first, simple)) = first {
                        if local {
                            self.declare(first, node, Some(node), "local");
                        } else if simple {
                            self.target(first, false);

                            if self.bindings.references[first.get()].is_none() {
                                self.bindings
                                    .globals
                                    .entry(text(tree, first))
                                    .or_default()
                                    .definition = Some(first);
                            }
                        } else {
                            self.visit(first);
                            self.mutate(first);
                            let root = self.bindings.path(tree, first);

                            if !root.is_empty() {
                                self.bindings.writes.insert(root);
                            }
                        }
                    }
                }

                self.scopes.push(BTreeMap::new());
                self.functions.push(node);
                let previous_loop = self.loop_body.take();
                let previous_dominated = mem::take(&mut self.dominated);

                if let Some(method) = method {
                    let index = self.bindings.locals.len();
                    let shadow = self.lookup("self");
                    self.declare(method, node, None, "parameter");

                    let scope = self
                        .scopes
                        .last_mut()
                        .expect("internal error: missing function scope");

                    scope.remove(&text(tree, method));
                    scope.insert("self".to_owned(), index);
                    "self".clone_into(&mut self.bindings.locals[index].name);
                    self.bindings.locals[index].shadow = shadow;
                }

                if let NodeKind::Parameters { parameters, .. } = &tree.node(*parameters).kind {
                    for parameter in tree.list(parameters) {
                        if matches!(
                            tree.node(parameter.node).kind,
                            NodeKind::Binding { .. } | NodeKind::Name { .. }
                        ) {
                            self.declare(parameter.node, node, None, "parameter");
                        } else {
                            self.annotation(parameter.node);
                        }
                    }
                }

                if let Some(returns) = returns {
                    self.annotation(*returns);
                }

                self.block(*body);
                self.dominated = previous_dominated;
                self.loop_body = previous_loop;
                self.functions.pop();
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
                self.declare(*binding, node, None, "loop");
                self.loop_block(*body);
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
                    self.declare(binding.node, node, None, "loop");
                }

                self.loop_block(*body);
                self.scopes.pop();
            }

            NodeKind::While {
                condition, body, ..
            } => {
                self.visit(*condition);
                self.scopes.push(BTreeMap::new());
                self.loop_block(*body);
                self.scopes.pop();
            }

            NodeKind::Repeat {
                body, condition, ..
            } => {
                self.scopes.push(BTreeMap::new());
                self.loop_block(*body);
                self.visit(*condition);
                self.scopes.pop();
            }

            NodeKind::Branch {
                condition, body, ..
            } => {
                let dominated = self.dominated.clone();
                self.scopes.push(BTreeMap::new());
                self.visit(*condition);
                self.block(*body);
                self.scopes.pop();
                self.dominated = dominated;
            }

            NodeKind::Else { body, .. } => {
                let dominated = self.dominated.clone();
                self.visit(*body);
                self.dominated = dominated;
            }

            NodeKind::Field { receiver, .. } => self.visit(*receiver),

            NodeKind::MethodCall {
                receiver,
                arguments,
                ..
            } => {
                self.visit(*receiver);
                self.mutate(*receiver);
                self.visit(*arguments);
            }

            NodeKind::Arguments { values, .. } => {
                for value in tree.list(values) {
                    self.visit(value.node);
                    self.mutate(value.node);
                }
            }

            NodeKind::TableField {
                opening,
                key,
                value,
                ..
            } => {
                if opening.is_some()
                    && let Some(key) = key
                {
                    self.visit(*key);
                }

                self.visit(*value);
            }

            NodeKind::TypeName {
                namespace,
                arguments,
                ..
            } => {
                if let Some(namespace) = namespace {
                    self.annotation(*namespace);
                }

                if let Some(arguments) = arguments {
                    self.annotation(*arguments);
                }
            }

            NodeKind::TypeAlias { annotation, .. }
            | NodeKind::Binding {
                annotation: Some(annotation),
                ..
            }
            | NodeKind::TypeField { annotation, .. }
            | NodeKind::TypeParameter { annotation, .. } => self.annotation(*annotation),

            NodeKind::Assertion {
                expression,
                annotation,
                ..
            } => {
                self.visit(*expression);
                self.annotation(*annotation);
            }

            NodeKind::Export { declaration, .. } => self.visit(*declaration),

            NodeKind::Declaration { declaration, .. } => {
                if let NodeKind::Binding { name, .. } = tree.node(*declaration).kind {
                    self.bindings.declared.insert(text(tree, name));
                }

                self.visit(*declaration);
            }

            NodeKind::Function {
                name: Some(name),
                body: None,
                ..
            } => {
                self.bindings.declared.insert(text(tree, *name));
            }

            NodeKind::Class { .. }
            | NodeKind::Function { body: None, .. }
            | NodeKind::Generics { .. }
            | NodeKind::Generic { .. }
            | NodeKind::Attribute { .. } => {}

            _ => {
                for child in tree.children(node) {
                    self.visit(child);
                }
            }
        }
    }

    fn loop_block(&mut self, body: NodeIndex) {
        let previous = self.loop_body.replace(body);
        let dominated = self.dominated.clone();
        self.block(body);
        self.dominated = dominated;
        self.loop_body = previous;
    }
}

pub(super) fn text(tree: &Tree<'_>, node: NodeIndex) -> String {
    String::from_utf8_lossy(tree.text(node)).into_owned()
}

pub(super) fn expands(tree: &Tree<'_>, node: NodeIndex) -> bool {
    matches!(
        tree.node(node).kind,
        NodeKind::Call { .. } | NodeKind::MethodCall { .. } | NodeKind::Variadic { .. }
    )
}
