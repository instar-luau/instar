use std::{collections::BTreeSet, ops::Range};

use vermis::tree::{NodeIndex, NodeKind, NodeList, Tree};

use crate::format::{directives::Directives, literal};

#[derive(Clone)]
pub(crate) struct Require {
    pub(crate) path: String,
    pub(crate) binding: NodeIndex,
}

pub(crate) struct Imports {
    globals: BTreeSet<usize>,
    scopes: Vec<bool>,
    reassigned: bool,
}

impl Imports {
    pub(crate) fn new(tree: &Tree<'_>) -> Self {
        let mut imports = Self {
            globals: BTreeSet::new(),
            scopes: vec![false],
            reassigned: false,
        };

        imports.visit(tree, tree.root);

        imports
    }

    pub(crate) fn require(
        &self,
        tree: &Tree<'_>,
        index: NodeIndex,
        directives: &Directives,
    ) -> Option<Require> {
        if self.reassigned || directives.blocks(tree, index) {
            return None;
        }

        let (NodeKind::Local {
            bindings, values, ..
        }
        | NodeKind::Constant {
            bindings, values, ..
        }) = &tree.node(index).kind
        else {
            return None;
        };

        let ([binding], [value]) = (tree.list(bindings), tree.list(values)) else {
            return None;
        };

        let NodeKind::Binding {
            name,
            annotation: None,
            ..
        } = &tree.node(binding.node).kind
        else {
            return None;
        };

        if tree.text(*name) == b"require" {
            return None;
        }

        let NodeKind::Call { callee, arguments } = &tree.node(value.node).kind else {
            return None;
        };

        if !self.globals.contains(&callee.get()) {
            return None;
        }

        let NodeKind::Arguments { values, .. } = &tree.node(*arguments).kind else {
            return None;
        };

        let [value] = tree.list(values) else {
            return None;
        };

        Some(Require {
            path: literal::string(tree, value.node)?,
            binding: *name,
        })
    }

    fn global(&self, tree: &Tree<'_>, node: NodeIndex) -> bool {
        tree.text(node) == b"require" && !self.scopes.iter().any(|shadowed| *shadowed)
    }

    fn bind(&mut self, tree: &Tree<'_>, node: NodeIndex) {
        if let NodeKind::Binding {
            name, annotation, ..
        } = &tree.node(node).kind
        {
            if let Some(annotation) = annotation {
                self.visit(tree, *annotation);
            }

            self.bind_name(tree, *name);
        }
    }

    fn bind_name(&mut self, tree: &Tree<'_>, node: NodeIndex) {
        if tree.text(node) == b"require" {
            *self
                .scopes
                .last_mut()
                .expect("internal error: missing lexical scope") = true;
        }
    }

    fn local(&mut self, tree: &Tree<'_>, bindings: &NodeList, values: &NodeList) {
        for value in tree.list(values) {
            self.visit(tree, value.node);
        }

        for binding in tree.list(bindings) {
            self.bind(tree, binding.node);
        }
    }

    fn write(&mut self, tree: &Tree<'_>, node: NodeIndex) {
        match &tree.node(node).kind {
            NodeKind::Name { .. } => self.reassigned |= self.global(tree, node),

            NodeKind::Field { receiver, .. }
            | NodeKind::Index { receiver, .. }
            | NodeKind::Group {
                expression: receiver,
                ..
            } => self.write(tree, *receiver),

            _ => {}
        }

        self.visit(tree, node);
    }

    fn scoped(&mut self, tree: &Tree<'_>, node: NodeIndex) {
        self.scopes.push(false);
        self.visit(tree, node);
        self.scopes.pop();
    }

    fn function(&mut self, tree: &Tree<'_>, node: NodeIndex, exported: bool) {
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

        if prefix.is_some_and(|prefix| tree.token(prefix).bytes(tree.source) == b"type") {
            return;
        }

        if let Some(name) = name {
            if exported
                || prefix.is_some_and(|prefix| {
                    matches!(tree.token(prefix).bytes(tree.source), b"local" | b"const")
                })
            {
                self.bind_name(tree, *name);
            } else if let NodeKind::FunctionName { path, .. } = &tree.node(*name).kind {
                if let Some(first) = tree.list(path).first() {
                    self.write(tree, first.node);
                }
            } else {
                self.write(tree, *name);
            }
        }

        if let Some(returns) = returns {
            self.visit(tree, *returns);
        }

        self.scopes.push(false);

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

    fn visit(&mut self, tree: &Tree<'_>, node: NodeIndex) {
        match &tree.node(node).kind {
            NodeKind::Name { .. } => {
                if self.global(tree, node) {
                    self.globals.insert(node.get());
                }
            }

            NodeKind::Local {
                bindings, values, ..
            }
            | NodeKind::Constant {
                bindings, values, ..
            } => self.local(tree, bindings, values),

            NodeKind::Assignment {
                targets, values, ..
            } => {
                for value in tree.list(values) {
                    self.visit(tree, value.node);
                }

                for target in tree.list(targets) {
                    self.write(tree, target.node);
                }
            }

            NodeKind::CompoundAssignment { target, value, .. } => {
                self.visit(tree, *value);
                self.write(tree, *target);
            }

            NodeKind::Function { .. } => self.function(tree, node, false),

            NodeKind::Export { declaration, .. }
                if matches!(tree.node(*declaration).kind, NodeKind::Function { .. }) =>
            {
                self.function(tree, *declaration, true);
            }

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
                self.scopes.push(false);
                self.visit(tree, *condition);
                self.visit(tree, *truthy);
                self.scopes.pop();
                self.visit(tree, *falsy);
            }

            NodeKind::While {
                condition, body, ..
            } => {
                self.scopes.push(false);
                self.visit(tree, *condition);
                self.visit(tree, *body);
                self.scopes.pop();
            }

            NodeKind::Repeat {
                body, condition, ..
            } => {
                self.scopes.push(false);
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

                self.scopes.push(false);
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

                self.scopes.push(false);

                for binding in tree.list(bindings) {
                    self.bind(tree, binding.node);
                }

                self.visit(tree, *body);
                self.scopes.pop();
            }

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
                    self.visit(tree, *namespace);
                }

                if let Some(arguments) = arguments {
                    self.visit(tree, *arguments);
                }
            }

            NodeKind::TypeAlias { annotation, .. }
            | NodeKind::TypeParameter { annotation, .. }
            | NodeKind::TypeField { annotation, .. } => self.visit(tree, *annotation),

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
}

pub(crate) fn runs<'require>(
    tree: &Tree<'_>,
    requires: impl IntoIterator<Item = Option<&'require Require>>,
) -> Vec<Range<usize>> {
    let requires: Vec<_> = requires.into_iter().collect();
    let mut result = Vec::new();
    let mut start = 0;

    while start < requires.len() {
        if requires[start].is_none() {
            start += 1;
            continue;
        }

        let mut end = start + 1;

        while end < requires.len() && requires[end].is_some() {
            end += 1;
        }

        let mut names = BTreeSet::new();

        if requires[start..end]
            .iter()
            .all(|require| require.is_some_and(|require| names.insert(tree.text(require.binding))))
        {
            result.push(start..end);
        }

        start = end;
    }

    result
}
