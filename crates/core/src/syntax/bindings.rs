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
    pub(crate) is_assigned: bool,
    pub(crate) is_initialized: bool,
    pub(crate) is_mutated: bool,
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
    pub(crate) is_read: bool,
    pub(crate) is_read_before_write: bool,
    pub(crate) has_module_assignment: bool,
    pub(crate) definition: Option<NodeIndex>,
}

pub(crate) struct TypeBinding {
    pub(crate) name: String,
    pub(crate) declaration: NodeIndex,
    pub(crate) statement: NodeIndex,
    pub(crate) scope: usize,
    pub(crate) is_used: bool,
}

pub(crate) struct Bindings {
    pub(crate) locals: Vec<Local>,
    pub(crate) globals: BTreeMap<String, Global>,
    pub(crate) references: Vec<Option<usize>>,
    pub(crate) types: Vec<TypeBinding>,
    pub(crate) type_references: Vec<Option<usize>>,
    pub(crate) is_type_name: Vec<bool>,
    pub(crate) accesses: Vec<u8>,
    pub(crate) is_read: Vec<bool>,
    pub(crate) parents: Vec<Option<NodeIndex>>,
    pub(crate) loops: Vec<Option<NodeIndex>>,
    is_runtime: Vec<bool>,
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
                is_type_name: vec![false; count],
                accesses: vec![0; count],
                is_read: vec![false; count],
                parents: vec![None; count],
                loops: vec![None; count],
                is_runtime: vec![false; count],
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
            is_annotation: false,
            is_runtime: true,
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

    pub(crate) fn is_same(&self, tree: &Tree<'_>, left: NodeIndex, right: NodeIndex) -> bool {
        let left = unwrap(tree, left);
        let right = unwrap(tree, right);

        matches!(tree.node(left).kind, NodeKind::Name { .. })
            && matches!(tree.node(right).kind, NodeKind::Name { .. })
            && self.references[left.get()] == self.references[right.get()]
            && tree.text(left) == tree.text(right)
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

    pub(crate) fn is_global(&self, tree: &Tree<'_>, node: NodeIndex, expected: &str) -> bool {
        if !self.is_runtime[node.get()]
            || expected.is_empty()
            || self.writes.contains("_G")
            || self
                .writes
                .contains(expected.split('.').next().unwrap_or(expected))
        {
            return false;
        }

        let mut node = unwrap(tree, node);
        let mut expected = expected;

        while let NodeKind::Field { receiver, name, .. } = tree.node(node).kind {
            let Some((root, field)) = expected.rsplit_once('.') else {
                return false;
            };

            if tree.text(name) != field.as_bytes() {
                return false;
            }

            node = unwrap(tree, receiver);
            expected = root;
        }

        matches!(tree.node(node).kind, NodeKind::Name { .. })
            && self.references[node.get()].is_none()
            && tree.text(node) == expected.as_bytes()
    }

    pub(crate) fn value(&self, tree: &Tree<'_>, mut node: NodeIndex) -> NodeIndex {
        let mut visited = BTreeSet::new();

        loop {
            node = unwrap(tree, node);

            let Some(index) = self.references[node.get()] else {
                break;
            };

            let binding = &self.locals[index];

            if binding.is_assigned || !visited.insert(index) {
                break;
            }

            let Some(value) = binding.value else { break };

            node = value;
        }

        node
    }

    pub(crate) fn is_exported(&self, tree: &Tree<'_>, statement: NodeIndex) -> bool {
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
    is_annotation: bool,
    is_runtime: bool,
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
            .expect("Internal error: scope is missing.")
    }

    fn declare_type(&mut self, name: NodeIndex, statement: NodeIndex) {
        let name_text = text(self.tree, name);
        let index = self.bindings.types.len();
        self.bindings.type_references[name.get()] = Some(index);
        self.bindings.is_type_name[name.get()] = true;

        self.type_scopes
            .last_mut()
            .expect("Internal error: type scope is missing.")
            .insert(name_text.clone(), index);

        self.bindings.types.push(TypeBinding {
            name: name_text,
            declaration: name,
            statement,
            scope: self.scope(),
            is_used: false,
        });
    }

    fn reference_type(&mut self, name: NodeIndex) {
        self.bindings.is_type_name[name.get()] = true;
        let name_text = text(self.tree, name);

        self.bindings.type_references[name.get()] = self
            .type_scopes
            .iter()
            .rev()
            .find_map(|scope| scope.get(&name_text).copied());

        if let Some(index) = self.bindings.type_references[name.get()] {
            self.bindings.types[index].is_used = true;
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
            .expect("Internal error: lexical scope is missing.")
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
            is_assigned: false,
            is_initialized: value.is_some(),
            is_mutated: false,
        });
    }

    fn reference(&mut self, node: NodeIndex, is_write: bool, is_read: bool) {
        let name = text(self.tree, node);
        self.bindings.accesses[node.get()] |= u8::from(is_read) | (u8::from(is_write) << 1);

        if let Some(index) = self.lookup(&name) {
            self.bindings.references[node.get()] = Some(index);
            let binding = &mut self.bindings.locals[index];
            binding.is_assigned |= is_write;

            if is_read {
                if self.is_annotation {
                    binding.type_reads.push(node);
                } else {
                    binding.reads.push(node);
                }
            }
        } else if is_read && self.functions.iter().any(|function| {
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
            self.reference_type(node);
        } else if self.is_annotation {
            self.bindings.globals.entry(name).or_default().is_read |= is_read;
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
            global.is_read |= is_read;

            global.is_read_before_write |=
                is_read && !self.functions.is_empty() && !self.dominated.contains(&name);

            if is_write {
                global.writes.push(node);
                global.has_module_assignment |= self.functions.is_empty();
                self.dominated.insert(name);
            }
        }

        self.bindings.is_read[node.get()] |= is_read && !self.is_annotation;
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

            self.bindings.locals[index].is_mutated = true;

            let Some(value) = self.bindings.locals[index].value else {
                break;
            };

            node = value;
        }
    }

    fn write(&mut self, mut node: NodeIndex) {
        if !self.is_runtime || self.is_annotation {
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

    fn target(&mut self, node: NodeIndex, is_read: bool) {
        self.write(node);

        match self.tree.node(node).kind {
            NodeKind::Name { .. } => self.reference(node, true, is_read),

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
        let was_annotation = self.is_annotation;
        self.is_annotation = true;
        self.visit(node);
        self.is_annotation = was_annotation;
    }

    fn visit(&mut self, node: NodeIndex) {
        let tree = self.tree;
        self.bindings.loops[node.get()] = self.loop_body;
        self.bindings.is_runtime[node.get()] = self.is_runtime && !self.is_annotation;

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

                let is_expanded = tree
                    .list(values)
                    .last()
                    .is_some_and(|value| can_expand(tree, value.node));

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
                        .expect("Internal error: declared local binding is missing.")
                        .is_initialized |= is_expanded;
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
                let was_runtime = self.is_runtime;

                self.is_runtime &=
                    !prefix.is_some_and(|prefix| tree.token(prefix).bytes(tree.source) == b"type");

                let is_local = self.bindings.is_exported(tree, node)
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

                    if let Some((first, is_simple)) = first {
                        if is_local {
                            self.declare(first, node, Some(node), Category::Local);
                        } else if is_simple {
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
                        .expect("Internal error: function scope is missing.");

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
                self.is_runtime = was_runtime;
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
                    self.reference_type(*name);
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

            NodeKind::GenericPack { name, .. } => self.reference_type(*name),

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
                let is_external = with.is_some()
                    || self.bindings.parents[node.get()].is_some_and(|parent| {
                        matches!(tree.node(parent).kind, NodeKind::Declaration { .. })
                    });

                if is_external {
                    self.bindings.declared.insert(text(tree, *name));
                } else {
                    self.declare(*name, node, Some(node), Category::Local);
                    self.declare_type(*name, node);
                }

                self.enter();

                if let Some(extends) = extends {
                    if is_external {
                        if let NodeKind::Extends { superclass, .. } = tree.node(*extends).kind {
                            self.reference_type(superclass);
                        }
                    } else {
                        self.visit(*extends);
                    }
                }

                for member in tree.list(members) {
                    if is_external {
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
                if !self.is_annotation
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

pub(crate) fn can_expand(tree: &Tree<'_>, node: NodeIndex) -> bool {
    matches!(
        tree.node(node).kind,
        NodeKind::Call { .. } | NodeKind::MethodCall { .. } | NodeKind::Variadic { .. }
    )
}
