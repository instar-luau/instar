use std::{
    collections::{BTreeMap, BTreeSet},
    mem,
};

use vermis::{
    token::{Keyword, TokenKind},
    tree::{NodeIndex, NodeKind, Tree},
};

use crate::syntax::expression::unwrap;

pub(crate) struct Local {
    pub(crate) name: String,
    pub(crate) declaration: NodeIndex,
    pub(crate) statement: NodeIndex,
    pub(crate) scope: usize,
    pub(crate) value: Option<NodeIndex>,
    pub(crate) category: Category,
    pub(crate) function: NodeIndex,
    pub(crate) shadow: Option<usize>,
    pub(crate) reads: Vec<NodeIndex>,
    pub(crate) type_reads: Vec<NodeIndex>,
    pub(crate) assigned: bool,
    pub(crate) initialized: bool,
    pub(crate) mutated: bool,
}

#[derive(Copy, Clone, Eq, PartialEq)]
pub(crate) enum Category {
    Local,
    Parameter,
    Loop,
}

#[derive(Default)]
pub(crate) struct Global {
    pub(crate) references: Vec<NodeIndex>,
    pub(crate) writes: Vec<NodeIndex>,
    pub(crate) functions: Vec<NodeIndex>,
    pub(crate) read: bool,
    pub(crate) read_before_write: bool,
    pub(crate) module_assignment: bool,
    pub(crate) definition: Option<NodeIndex>,
}

pub(crate) struct TypeBinding {
    pub(crate) name: String,
    pub(crate) declaration: NodeIndex,
    pub(crate) statement: NodeIndex,
    pub(crate) scope: usize,
    pub(crate) used: bool,
}

pub(crate) struct Bindings {
    pub(crate) locals: Vec<Local>,
    pub(crate) globals: BTreeMap<String, Global>,
    pub(crate) references: Vec<Option<usize>>,
    pub(crate) types: Vec<TypeBinding>,
    pub(crate) type_references: Vec<Option<usize>>,
    pub(crate) type_names: Vec<bool>,
    pub(crate) accesses: Vec<u8>,
    pub(crate) reads: Vec<bool>,
    pub(crate) parents: Vec<Option<NodeIndex>>,
    pub(crate) loops: Vec<Option<NodeIndex>>,
    runtime: Vec<bool>,
    writes: BTreeSet<String>,
    pub(crate) declared: BTreeSet<String>,
}

impl Bindings {
    pub(crate) fn new(tree: &Tree<'_>) -> Self {
        let count = tree.nodes.len();

        let mut collector = Collector {
            tree,
            bindings: Self {
                locals: Vec::new(),
                globals: BTreeMap::new(),
                references: vec![None; count],
                types: Vec::new(),
                type_references: vec![None; count],
                type_names: vec![false; count],
                accesses: vec![0; count],
                reads: vec![false; count],
                parents: vec![None; count],
                loops: vec![None; count],
                runtime: vec![false; count],
                writes: BTreeSet::new(),
                declared: BTreeSet::new(),
            },
            scopes: vec![BTreeMap::new()],
            type_scopes: vec![BTreeMap::new()],
            scope_identifiers: vec![0],
            next_scope: 1,
            functions: Vec::new(),
            dominated: BTreeSet::new(),
            loop_body: None,
            annotation: false,
            runtime: true,
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

    pub(crate) fn path(&self, tree: &Tree<'_>, node: NodeIndex) -> String {
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

    pub(crate) fn global(&self, tree: &Tree<'_>, node: NodeIndex, expected: &str) -> bool {
        self.runtime[node.get()]
            && !expected.is_empty()
            && self.path(tree, node) == expected
            && !self.writes.contains("_G")
            && !self
                .writes
                .contains(expected.split('.').next().unwrap_or(expected))
    }

    pub(crate) fn value(&self, tree: &Tree<'_>, mut node: NodeIndex) -> NodeIndex {
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

    pub(crate) fn exported(&self, tree: &Tree<'_>, statement: NodeIndex) -> bool {
        self.parents[statement.get()]
            .is_some_and(|parent| matches!(tree.node(parent).kind, NodeKind::Export { .. }))
    }
}

struct Collector<'tree, 'source> {
    tree: &'tree Tree<'source>,
    bindings: Bindings,
    scopes: Vec<BTreeMap<String, usize>>,
    type_scopes: Vec<BTreeMap<String, usize>>,
    scope_identifiers: Vec<usize>,
    next_scope: usize,
    functions: Vec<NodeIndex>,
    dominated: BTreeSet<String>,
    loop_body: Option<NodeIndex>,
    annotation: bool,
    runtime: bool,
}

impl Collector<'_, '_> {
    fn enter(&mut self) {
        self.scopes.push(BTreeMap::new());
        self.type_scopes.push(BTreeMap::new());
        self.scope_identifiers.push(self.next_scope);
        self.next_scope += 1;
    }

    fn leave(&mut self) {
        self.scopes.pop();
        self.type_scopes.pop();
        self.scope_identifiers.pop();
    }

    fn scope(&self) -> usize {
        *self
            .scope_identifiers
            .last()
            .expect("internal error: missing scope")
    }

    fn declare_type(&mut self, name: NodeIndex, statement: NodeIndex) {
        let name_text = text(self.tree, name);
        let index = self.bindings.types.len();
        self.bindings.type_references[name.get()] = Some(index);
        self.bindings.type_names[name.get()] = true;

        self.type_scopes
            .last_mut()
            .expect("internal error: missing type scope")
            .insert(name_text.clone(), index);

        self.bindings.types.push(TypeBinding {
            name: name_text,
            declaration: name,
            statement,
            scope: self.scope(),
            used: false,
        });
    }

    fn type_reference(&mut self, name: NodeIndex) {
        self.bindings.type_names[name.get()] = true;
        let name_text = text(self.tree, name);

        self.bindings.type_references[name.get()] = self
            .type_scopes
            .iter()
            .rev()
            .find_map(|scope| scope.get(&name_text).copied());

        if let Some(index) = self.bindings.type_references[name.get()] {
            self.bindings.types[index].used = true;
        }
    }

    fn generics(&mut self, generics: Option<NodeIndex>) {
        let tree = self.tree;

        let Some(generics) = generics else { return };

        let NodeKind::Generics { parameters, .. } = &tree.node(generics).kind else {
            return;
        };

        for parameter in tree.list(parameters) {
            if let NodeKind::Generic { name, default, .. } = tree.node(parameter.node).kind {
                if let Some(default) = default {
                    self.annotation(default);
                }

                self.declare_type(name, parameter.node);
            }
        }
    }

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
        category: Category,
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
        self.bindings.accesses[name_node.get()] |= 2;

        self.scopes
            .last_mut()
            .expect("internal error: missing lexical scope")
            .insert(name.clone(), index);

        self.bindings.locals.push(Local {
            name,
            declaration: name_node,
            statement,
            scope: self.scope(),
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
        self.bindings.accesses[node.get()] |= u8::from(read) | (u8::from(write) << 1);

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
        } else if read && self.functions.iter().any(|function| {
            matches!(self.tree.node(*function).kind, NodeKind::Function { prefix: Some(prefix), .. }
                    if self.tree.token(prefix).bytes(self.tree.source) == b"type")
        })
            && self
                .type_scopes
                .iter()
                .rev()
                .find_map(|scope| scope.get(&name).copied())
                .is_some_and(|index| {
                    matches!(
                        self.tree.node(self.bindings.types[index].statement).kind,
                        NodeKind::Function { .. }
                    )
                })
        {
            self.type_reference(node);
        } else if self.annotation {
            self.bindings.globals.entry(name).or_default().read |= read;
        } else {
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

    fn write(&mut self, mut node: NodeIndex) {
        if !self.runtime || self.annotation {
            return;
        }

        loop {
            node = unwrap(self.tree, node);

            match self.tree.node(node).kind {
                NodeKind::Field { receiver, .. } | NodeKind::Index { receiver, .. } => {
                    node = receiver;
                }

                NodeKind::Name { .. } => {
                    let name = text(self.tree, node);

                    if self.lookup(&name).is_none() {
                        self.bindings.writes.insert(name);
                    }

                    break;
                }

                _ => break,
            }
        }
    }

    fn target(&mut self, node: NodeIndex, read: bool) {
        self.write(node);

        match self.tree.node(node).kind {
            NodeKind::Name { .. } => self.reference(node, true, read),

            NodeKind::Field { receiver, .. } | NodeKind::Index { receiver, .. } => {
                self.visit(node);
                self.mutate(receiver);
            }

            _ => self.visit(node),
        }
    }

    fn block(&mut self, node: NodeIndex) {
        let tree = self.tree;

        if let NodeKind::Block { statements } = &tree.node(node).kind {
            for entry in tree.list(statements) {
                let declaration = match tree.node(entry.node).kind {
                    NodeKind::Export { declaration, .. } => declaration,
                    _ => entry.node,
                };

                let name = match tree.node(declaration).kind {
                    NodeKind::TypeAlias { name, .. } => Some(name),

                    NodeKind::Function {
                        prefix: Some(prefix),
                        name,
                        ..
                    } if tree.token(prefix).bytes(tree.source) == b"type" => name,

                    _ => None,
                };

                if let Some(name) = name {
                    self.declare_type(name, declaration);
                }
            }

            for entry in tree.list(statements) {
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
        self.bindings.runtime[node.get()] = self.runtime && !self.annotation;

        match &tree.node(node).kind {
            NodeKind::Name { .. } => self.reference(node, false, true),

            NodeKind::Block { .. } => {
                self.enter();
                self.block(node);
                self.leave();
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
                        Category::Local,
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
                generics,
                parameters,
                returns,
                body: Some(body),
                ..
            } => {
                let previous_runtime = self.runtime;

                self.runtime &=
                    !prefix.is_some_and(|prefix| tree.token(prefix).bytes(tree.source) == b"type");

                let local = self.bindings.exported(tree, node)
                    || prefix.is_some_and(|prefix| {
                        tree.token(prefix).kind == TokenKind::Keyword(Keyword::Local)
                            || tree.token(prefix).bytes(tree.source) == b"const"
                    });

                let mut method = None;

                if let Some(name) = name
                    && !prefix
                        .is_some_and(|prefix| tree.token(prefix).bytes(tree.source) == b"type")
                    && !self.bindings.parents[node.get()].is_some_and(|parent| {
                        matches!(tree.node(parent).kind, NodeKind::Class { .. })
                    })
                {
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
                            self.declare(first, node, Some(node), Category::Local);
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
                            self.write(first);
                        }
                    }
                }

                self.enter();
                self.generics(*generics);
                self.functions.push(node);
                let previous_loop = self.loop_body.take();
                let previous_dominated = mem::take(&mut self.dominated);

                if let Some(method) = method {
                    let index = self.bindings.locals.len();
                    let shadow = self.lookup("self");
                    self.declare(method, node, None, Category::Parameter);

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
                            self.declare(parameter.node, node, None, Category::Parameter);
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
                self.leave();
                self.runtime = previous_runtime;
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

                self.enter();
                self.declare(*binding, node, None, Category::Loop);
                self.loop_block(*body);
                self.leave();
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

                self.enter();

                for binding in tree.list(bindings) {
                    self.declare(binding.node, node, None, Category::Loop);
                }

                self.loop_block(*body);
                self.leave();
            }

            NodeKind::While {
                condition, body, ..
            } => {
                self.visit(*condition);
                self.enter();
                self.loop_block(*body);
                self.leave();
            }

            NodeKind::Repeat {
                body, condition, ..
            } => {
                self.enter();
                self.loop_block(*body);
                self.visit(*condition);
                self.leave();
            }

            NodeKind::Branch {
                condition, body, ..
            } => {
                let dominated = self.dominated.clone();
                self.enter();
                self.visit(*condition);
                self.block(*body);
                self.leave();
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
                name,
                arguments,
                ..
            } => {
                if let Some(namespace) = namespace {
                    self.annotation(*namespace);
                } else {
                    self.type_reference(*name);
                }

                if let Some(arguments) = arguments {
                    self.annotation(*arguments);
                }
            }

            NodeKind::TypeAlias {
                name,
                generics,
                annotation,
                ..
            } => {
                if self.bindings.type_references[name.get()].is_none() {
                    self.declare_type(*name, node);
                }

                self.enter();
                self.generics(*generics);
                self.annotation(*annotation);
                self.leave();
            }

            NodeKind::TypeFunction {
                generics,
                parameters,
                returns,
                ..
            } => {
                self.enter();
                self.generics(*generics);
                self.annotation(*parameters);
                self.annotation(*returns);
                self.leave();
            }

            NodeKind::GenericPack { name, .. } => self.type_reference(*name),

            NodeKind::Binding { annotation, .. } => {
                if let Some(annotation) = annotation {
                    self.annotation(*annotation);
                }
            }

            NodeKind::TypeField { annotation, .. } | NodeKind::TypeParameter { annotation, .. } => {
                self.annotation(*annotation);
            }

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

            NodeKind::Class {
                name,
                extends,
                with,
                members,
                ..
            } => {
                let external = with.is_some()
                    || self.bindings.parents[node.get()].is_some_and(|parent| {
                        matches!(tree.node(parent).kind, NodeKind::Declaration { .. })
                    });

                if external {
                    self.bindings.declared.insert(text(tree, *name));
                } else {
                    self.declare(*name, node, Some(node), Category::Local);
                    self.declare_type(*name, node);
                }

                self.enter();

                if let Some(extends) = extends {
                    if external {
                        if let NodeKind::Extends { superclass, .. } = tree.node(*extends).kind {
                            self.type_reference(superclass);
                        }
                    } else {
                        self.visit(*extends);
                    }
                }

                for member in tree.list(members) {
                    if external {
                        self.annotation(member.node);
                    } else {
                        self.visit(member.node);
                    }
                }

                self.leave();
            }

            NodeKind::Function {
                name,
                generics,
                parameters,
                returns,
                body: None,
                ..
            } => {
                if !self.annotation
                    && let Some(name) = name
                {
                    self.bindings.declared.insert(text(tree, *name));
                }

                self.enter();
                self.generics(*generics);
                self.annotation(*parameters);

                if let Some(returns) = returns {
                    self.annotation(*returns);
                }

                self.leave();
            }

            NodeKind::Generics { .. } | NodeKind::Generic { .. } | NodeKind::Attribute { .. } => {}

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

pub(crate) fn text(tree: &Tree<'_>, node: NodeIndex) -> String {
    String::from_utf8_lossy(tree.text(node)).into_owned()
}

pub(crate) fn expands(tree: &Tree<'_>, node: NodeIndex) -> bool {
    matches!(
        tree.node(node).kind,
        NodeKind::Call { .. } | NodeKind::MethodCall { .. } | NodeKind::Variadic { .. }
    )
}
