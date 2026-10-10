//! Lexical bindings and global identity over typed Luau syntax.

use std::collections::{BTreeMap, BTreeSet};

use instar_shared::{Availability, Completion, Options, Reason, SourceVersion, Span};

use vermis::{
    token::{Keyword, TokenKind},
    tree::{NodeIndex, NodeKind, NodeList, Tree},
};

use crate::Tree as SnapshotTree;

/// Lexical declaration category.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Category {
    /// Local variable.
    Local,

    /// Constant binding.
    Constant,

    /// Function parameter.
    Parameter,

    /// Implicit receiver of a colon-method declaration.
    Receiver,

    /// Loop binding.
    Loop,

    /// Local function.
    Function,
}

/// Execution realm supplying lexical occurrences and aggregate binding effects.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExecutionScope {
    /// Ordinary module execution, including its runtime closures.
    Runtime,

    /// An independent type-function virtual machine and its closures.
    TypeFunction(NodeIndex),
}

/// One lexical declaration and its observed uses.
pub struct Binding {
    /// Declaration name node.
    pub name: NodeIndex,

    /// Declaration category.
    pub category: Category,

    /// Initializer expression when available.
    pub value: Option<NodeIndex>,

    /// Whether the binding is read in its execution realm.
    pub read: bool,

    /// Whether the binding is reassigned in its execution realm.
    pub assigned: bool,

    /// Whether the referenced value is mutated in its execution realm.
    pub mutated: bool,
}

/// Lexical declarations and references from one immutable syntax tree.
pub struct BindingResult {
    /// Exact snapshot revision supplying these lexical identities.
    pub source: SourceVersion,

    /// Declarations indexed by syntax node identity.
    pub declarations: BTreeMap<usize, Binding>,

    /// References mapped to their declaration identities.
    pub references: BTreeMap<usize, usize>,

    /// Execution realm for traversed nodes, declarations and name occurrences.
    pub execution: BTreeMap<usize, ExecutionScope>,

    /// Name occurrences read by expressions or compound assignments.
    pub reads: BTreeSet<usize>,

    /// Assignment target occurrences.
    pub writes: BTreeSet<usize>,

    /// Root receiver occurrences whose members are mutated.
    pub mutations: BTreeSet<usize>,

    /// Declarations mapped to the prior lexical declarations they shadow.
    pub shadows: BTreeMap<usize, usize>,

    /// Unbound name references.
    pub globals: BTreeSet<usize>,

    /// Runtime global names reassigned in this source.
    pub global_assignments: BTreeSet<String>,

    /// Runtime global values whose members are mutated in this source.
    pub global_mutations: BTreeSet<String>,

    /// Known function argument counts and variadic status at references.
    pub arities: BTreeMap<usize, (usize, bool)>,

    scopes: Vec<BTreeMap<String, usize>>,
    functions: BTreeMap<usize, (usize, bool)>,
    options: Options,
    execution_scope: ExecutionScope,

    /// Malformed spans where lexical facts are unavailable.
    pub recovery: Vec<Span>,

    /// Lexical coverage in the presence of recovered or interrupted syntax.
    pub availability: Availability,

    /// Whether lexical traversal completed.
    pub completion: Completion,
}

impl BindingResult {
    fn collect(snapshot: &SnapshotTree<'_>, options: &Options) -> Self {
        let tree = snapshot.syntax();

        let mut bindings = Self {
            source: snapshot.snapshot().version,
            declarations: BTreeMap::new(),
            references: BTreeMap::new(),
            execution: BTreeMap::new(),
            reads: BTreeSet::new(),
            writes: BTreeSet::new(),
            mutations: BTreeSet::new(),
            shadows: BTreeMap::new(),
            globals: BTreeSet::new(),
            global_assignments: BTreeSet::new(),
            global_mutations: BTreeSet::new(),
            arities: BTreeMap::new(),
            scopes: vec![BTreeMap::new()],
            functions: BTreeMap::new(),
            options: options.clone(),
            execution_scope: ExecutionScope::Runtime,
            recovery: Vec::new(),
            availability: Availability::Available,
            completion: Completion::Complete,
        };

        bindings.visit(tree, tree.root);

        bindings
    }

    /// Whether a reference names an unchanged runtime global.
    #[must_use]
    pub fn global(&self, tree: &Tree<'_>, node: NodeIndex) -> bool {
        self.runtime(node)
            && self.globals.contains(&node.get())
            && !self.global_assignments.contains(&text(tree, node))
            && !self.global_mutations.contains(&text(tree, node))
    }

    /// Whether a traversed node belongs to ordinary runtime execution.
    #[must_use]
    pub fn runtime(&self, node: NodeIndex) -> bool {
        self.execution.get(&node.get()) == Some(&ExecutionScope::Runtime)
    }

    /// Returns the declaration referenced by a syntax node.
    #[must_use]
    pub fn declaration(&self, node: NodeIndex) -> Option<&Binding> {
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
        self.execution.insert(index, self.execution_scope);

        if let Some(shadowed) = self.resolve(tree, name) {
            self.shadows.insert(index, shadowed);
        }

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
        self.reads.insert(node.get());
        self.execution.insert(node.get(), self.execution_scope);

        if let Some(index) = self.resolve(tree, node) {
            self.references.insert(node.get(), index);

            if self.execution.get(&index) == Some(&self.execution_scope) {
                self.declarations
                    .get_mut(&index)
                    .expect("resolved binding")
                    .read = true;
            }

            if let Some(arity) = self.functions.get(&index) {
                self.arities.insert(node.get(), *arity);
            }
        } else {
            self.globals.insert(node.get());
        }
    }

    fn write(&mut self, tree: &Tree<'_>, node: NodeIndex, reading: bool) {
        self.writes.insert(node.get());
        self.execution.insert(node.get(), self.execution_scope);

        if reading {
            self.reads.insert(node.get());
        }

        if matches!(tree.node(node).kind, NodeKind::Name { .. }) {
            if let Some(index) = self.resolve(tree, node) {
                self.references.insert(node.get(), index);

                if self.execution.get(&index) == Some(&self.execution_scope) {
                    let binding = self.declarations.get_mut(&index).expect("resolved write");
                    binding.assigned = true;
                    binding.read |= reading;
                    self.functions.remove(&index);
                }
            } else {
                self.globals.insert(node.get());

                if self.execution_scope == ExecutionScope::Runtime {
                    self.global_assignments.insert(text(tree, node));
                }
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
            self.mutations.insert(receiver.get());
            self.execution.insert(receiver.get(), self.execution_scope);

            if let Some(index) = self.resolve(tree, *receiver) {
                if self.execution.get(&index) == Some(&self.execution_scope) {
                    self.declarations
                        .get_mut(&index)
                        .expect("resolved mutation")
                        .mutated = true;
                }
            } else if self.execution_scope == ExecutionScope::Runtime {
                self.global_mutations.insert(text(tree, *receiver));
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

        let type_function =
            prefix.is_some_and(|prefix| tree.token(prefix).bytes(tree.source) == b"type");

        let execution_scope = self.execution_scope;

        if type_function {
            self.execution_scope = ExecutionScope::TypeFunction(node);
            self.execution.insert(node.get(), self.execution_scope);
        }

        if let Some(name) = name {
            self.execution.insert(name.get(), self.execution_scope);
        }

        if let NodeKind::Parameters { parameters, .. } = &tree.node(*parameters).kind {
            for parameter in tree.list(parameters) {
                if let NodeKind::Binding {
                    annotation: Some(annotation),
                    ..
                }
                | NodeKind::Variadic {
                    annotation: Some(annotation),
                    ..
                } = &tree.node(parameter.node).kind
                {
                    self.visit(tree, *annotation);
                }
            }
        }

        if let Some(returns) = returns {
            self.visit(tree, *returns);
        }

        let local = prefix.is_some_and(|prefix| {
            tree.token(prefix).kind == TokenKind::Keyword(Keyword::Local)
                || tree.token(prefix).bytes(tree.source) == b"const"
        });

        if !type_function && let Some(name) = name {
            if local {
                self.bind_name(tree, *name, Category::Function, Some(node));
            } else if let NodeKind::FunctionName { path, method, .. } = &tree.node(*name).kind {
                if let Some(first) = tree.list(path).first() {
                    if tree.list(path).len() == 1 && method.is_none() {
                        self.write(tree, first.node, false);
                    } else {
                        self.read(tree, first.node);
                        self.mutations.insert(first.node.get());

                        if let Some(index) = self.resolve(tree, first.node) {
                            if self.execution.get(&index) == Some(&self.execution_scope) {
                                self.declarations
                                    .get_mut(&index)
                                    .expect("function receiver")
                                    .mutated = true;
                            }
                        } else if self.execution_scope == ExecutionScope::Runtime {
                            self.global_mutations.insert(text(tree, first.node));
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
            if let Some(shadowed) = self
                .scopes
                .iter()
                .rev()
                .find_map(|scope| scope.get("self"))
                .copied()
            {
                self.shadows.insert(method.get(), shadowed);
            }

            self.scopes
                .last_mut()
                .expect("method scope")
                .insert("self".to_owned(), method.get());

            self.execution.insert(method.get(), self.execution_scope);

            self.declarations.insert(
                method.get(),
                Binding {
                    name: *method,
                    category: Category::Receiver,
                    value: None,
                    read: true,
                    assigned: false,
                    mutated: false,
                },
            );
        }

        if let NodeKind::Parameters { parameters, .. } = &tree.node(*parameters).kind {
            for parameter in tree.list(parameters) {
                if let NodeKind::Binding { name, .. } = &tree.node(parameter.node).kind {
                    self.bind_name(tree, *name, Category::Parameter, None);
                }
            }
        }

        if let Some(body) = body {
            self.visit(tree, *body);
        }

        self.scopes.pop();
        self.execution_scope = execution_scope;
    }

    fn visit(&mut self, tree: &Tree<'_>, node: NodeIndex) {
        if self.completion != Completion::Complete {
            return;
        }

        if let Some(completion) = self.options.interrupted() {
            self.completion = completion;

            return;
        }

        self.execution.insert(node.get(), self.execution_scope);

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

/// Collects lexical declarations, occurrences and writes without inference.
#[must_use]
pub fn bindings(tree: &SnapshotTree<'_>, options: &Options) -> BindingResult {
    let mut bindings = BindingResult::collect(tree, options);

    bindings.recovery = tree
        .syntax()
        .diagnostics
        .iter()
        .map(|diagnostic| Span {
            start: diagnostic.span.start,
            end: diagnostic.span.end,
        })
        .collect();

    if tree.coverage() != &Completion::Complete {
        bindings.completion = tree.coverage().clone();
    }

    bindings.availability = if bindings.completion != Completion::Complete {
        Availability::Unavailable(Reason::FailedPrerequisite)
    } else if bindings.recovery.is_empty() {
        Availability::Available
    } else {
        Availability::Partial(Reason::UnsupportedSyntax)
    };

    bindings
}

fn text(tree: &Tree<'_>, node: NodeIndex) -> String {
    String::from_utf8_lossy(tree.text(node)).into_owned()
}

#[cfg(test)]
mod tests {
    use instar_shared::{Options, SourceIdentity, SourceSnapshot, SourceVersion};

    use crate::{SyntaxSettings, bindings, imports, parse};

    use super::ExecutionScope;

    #[test]
    fn type_function_effects_do_not_change_runtime_bindings() {
        let source = SourceSnapshot::from_bytes(
            SourceVersion {
                identity: SourceIdentity::new(1),
                revision: 1,
            },
            br#"
local runtime_loader = require
local runtime_table = {}
type function transform(value)
    require = replacement
    script.Parent = replacement
    runtime_loader = replacement
    runtime_table.changed = true
    local sandbox = {}
    sandbox.changed = value
    local state = value
    state = nil
    local unavailable = require("vm")
    function helper()
        workspace = replacement
    end
    return state
end
local module = require("runtime")
runtime_loader("alias")
"#,
        )
        .expect("UTF-8 regression source");

        let options = Options::default();
        let parsed = parse(&source, &SyntaxSettings::default(), &options);
        assert_eq!(parsed.diagnostics.len(), 0);
        let tree = parsed.tree.syntax();
        let facts = bindings(&parsed.tree, &options);
        assert!(facts.global_assignments.is_empty());
        assert!(facts.global_mutations.is_empty());

        let declaration = |name: &[u8]| {
            facts
                .declarations
                .values()
                .find(|binding| tree.text(binding.name) == name)
                .expect("lexical declaration")
        };

        let loader = declaration(b"runtime_loader");
        assert!(loader.read);
        assert!(!loader.assigned);
        assert!(facts.runtime(loader.name));
        let table = declaration(b"runtime_table");
        assert!(!table.read);
        assert!(!table.mutated);
        let sandbox = declaration(b"sandbox");
        assert!(sandbox.read);
        assert!(sandbox.mutated);

        assert!(matches!(
            facts.execution.get(&sandbox.name.get()),
            Some(ExecutionScope::TypeFunction(_))
        ));

        assert!(declaration(b"state").assigned);
        assert!(declaration(b"state").read);
        assert!(declaration(b"value").read);

        let write = facts
            .writes
            .iter()
            .find(|write| facts.references.get(*write) == Some(&loader.name.get()))
            .expect("retained cross-execution lexical write");

        assert!(matches!(
            facts.execution.get(write),
            Some(ExecutionScope::TypeFunction(_))
        ));

        let imports = imports(&parsed.tree, &options);

        assert_eq!(
            imports
                .iter()
                .filter_map(|import| import.request.as_deref())
                .collect::<Vec<_>>(),
            vec!["runtime"]
        );
    }
}
