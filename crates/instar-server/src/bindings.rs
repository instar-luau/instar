use std::collections::{BTreeMap, HashMap};

use tower_lsp_server::ls_types::{SemanticTokenModifier, SemanticTokenType};

use vermis::{
    token::{Keyword, Span, Symbol, TokenKind},
    tree::{NodeIndex, NodeKind, Tree},
};

type BindingId = usize;

// Discriminants are indices into LEGEND, not LSP's other kind enums.
#[derive(Clone, Copy)]
#[repr(u32)]
pub(crate) enum SemanticKind {
    Variable,
    Function,
    Property,
    Type,
    Namespace,
    Parameter,
    Method,
    TypeParameter,
    Class,
    Keyword,
    String,
    Number,
    Comment,
    Operator,
}

impl SemanticKind {
    pub(crate) const LEGEND: [SemanticTokenType; 14] = [
        SemanticTokenType::VARIABLE,
        SemanticTokenType::FUNCTION,
        SemanticTokenType::PROPERTY,
        SemanticTokenType::TYPE,
        SemanticTokenType::NAMESPACE,
        SemanticTokenType::PARAMETER,
        SemanticTokenType::METHOD,
        SemanticTokenType::TYPE_PARAMETER,
        SemanticTokenType::CLASS,
        SemanticTokenType::KEYWORD,
        SemanticTokenType::STRING,
        SemanticTokenType::NUMBER,
        SemanticTokenType::COMMENT,
        SemanticTokenType::OPERATOR,
    ];
}

pub(crate) const MODIFIER_LEGEND: [SemanticTokenModifier; 2] = [
    SemanticTokenModifier::READONLY,
    SemanticTokenModifier::DECLARATION,
];

// Bit positions correspond to MODIFIER_LEGEND.
const READONLY: u32 = 1 << 0;

const DECLARATION: u32 = 1 << 1;

#[derive(Default)]
struct Scope {
    parent: Option<usize>,
    names: HashMap<String, Vec<BindingId>>,
}

struct Binding {
    visible: usize,
    kind: SemanticKind,
    loader: bool,
    written: bool,
}

pub(crate) struct Import {
    pub(crate) span: Span,
    callee: Span,
    scope: usize,
}

pub(crate) struct Index {
    scopes: Vec<Scope>,
    bindings: Vec<Binding>,
    pub(crate) imports: Vec<Import>,
    literals: Vec<Span>,
    tokens: BTreeMap<(usize, usize), (SemanticKind, u32)>,
    require_written: bool,
}

impl Index {
    pub(crate) fn new(tree: &Tree<'_>) -> Self {
        let mut index = Self {
            scopes: vec![Scope::default()],
            bindings: Vec::new(),
            imports: Vec::new(),
            literals: Vec::new(),
            tokens: BTreeMap::new(),
            require_written: false,
        };

        for token in &tree.tokens {
            if matches!(
                token.kind,
                TokenKind::QuotedString | TokenKind::RawString | TokenKind::MalformedString
            ) {
                index.literals.push(token.span);
            }

            let kind = match token.kind {
                TokenKind::Keyword(_) => SemanticKind::Keyword,

                TokenKind::QuotedString
                | TokenKind::RawString
                | TokenKind::InterpolatedStringStart
                | TokenKind::InterpolatedStringMiddle
                | TokenKind::InterpolatedStringEnd
                | TokenKind::InterpolatedStringSimple => SemanticKind::String,

                TokenKind::Number => SemanticKind::Number,

                TokenKind::Comment | TokenKind::BlockComment => SemanticKind::Comment,

                TokenKind::Symbol(
                    Symbol::Equal
                    | Symbol::LessThanOrEqual
                    | Symbol::GreaterThanOrEqual
                    | Symbol::NotEqual
                    | Symbol::Concatenate
                    | Symbol::Ellipsis
                    | Symbol::Arrow
                    | Symbol::DoubleColon
                    | Symbol::FloorDivide
                    | Symbol::AddAssignment
                    | Symbol::SubtractAssignment
                    | Symbol::MultiplyAssignment
                    | Symbol::DivideAssignment
                    | Symbol::FloorDivideAssignment
                    | Symbol::ModuloAssignment
                    | Symbol::PowerAssignment
                    | Symbol::ConcatenateAssignment,
                ) => SemanticKind::Operator,

                _ => continue,
            };

            index.token(token.span, kind, false);
        }

        index.visit(tree, tree.root, 0);

        index.imports = std::mem::take(&mut index.imports)
            .into_iter()
            .filter(|site| {
                index.loader(
                    site.callee.bytes(tree.source),
                    site.scope,
                    site.callee.start,
                )
            })
            .collect();

        index
    }

    fn token(&mut self, span: Span, kind: SemanticKind, declaration: bool) {
        self.tokens.insert(
            (span.start, span.end),
            (kind, if declaration { DECLARATION } else { 0 }),
        );
    }

    fn scope(&mut self, parent: Option<usize>) -> usize {
        let id = self.scopes.len();

        self.scopes.push(Scope {
            parent,
            names: HashMap::new(),
        });

        id
    }

    fn lookup(&self, name: &str, mut scope: usize, offset: usize) -> Option<BindingId> {
        loop {
            let candidate = self.scopes[scope].names.get(name).and_then(|ids| {
                ids.iter()
                    .rev()
                    .copied()
                    .find(|&id| self.bindings[id].visible <= offset)
            });

            if candidate.is_some() {
                return candidate;
            }

            scope = self.scopes[scope].parent?;
        }
    }

    fn loader(&self, name: &[u8], scope: usize, offset: usize) -> bool {
        let name = String::from_utf8_lossy(name);

        self.lookup(&name, scope, offset).map_or_else(
            || name == "require" && !self.require_written,
            |id| self.bindings[id].loader && !self.bindings[id].written,
        )
    }

    fn declare(
        &mut self,
        tree: &Tree<'_>,
        name: NodeIndex,
        scope: usize,
        visible: usize,
        kind: SemanticKind,
        loader: bool,
    ) {
        let span = tree.node(name).span;
        let name = String::from_utf8_lossy(tree.text(name)).into_owned();
        let id = self.bindings.len();

        self.scopes[scope].names.entry(name).or_default().push(id);

        self.bindings.push(Binding {
            visible,
            kind,
            loader,
            written: false,
        });

        self.token(span, kind, true);
    }

    fn bind(
        &mut self,
        tree: &Tree<'_>,
        node: NodeIndex,
        scope: usize,
        visible: usize,
        kind: SemanticKind,
        loader: bool,
    ) {
        if let NodeKind::Binding {
            name, annotation, ..
        } = tree.node(node).kind
        {
            if let Some(annotation) = annotation {
                self.annotation(tree, annotation, scope);
            }

            self.declare(tree, name, scope, visible, kind, loader);
        } else {
            self.annotation(tree, node, scope);
        }
    }

    fn reference(&mut self, tree: &Tree<'_>, node: NodeIndex, scope: usize, write: bool) {
        let span = tree.node(node).span;
        let name = String::from_utf8_lossy(tree.text(node));
        let binding = self.lookup(&name, scope, span.start);

        if write {
            if let Some(id) = binding {
                self.bindings[id].written = true;
            } else if name == "require" {
                self.require_written = true;
            }
        }

        self.token(
            span,
            binding.map_or(SemanticKind::Variable, |id| self.bindings[id].kind),
            false,
        );
    }

    fn scoped(&mut self, tree: &Tree<'_>, node: NodeIndex, parent: usize) {
        let scope = self.scope(Some(parent));
        self.visit(tree, node, scope);
    }

    fn class(&mut self, tree: &Tree<'_>, node: NodeIndex, scope: usize) {
        let NodeKind::Class {
            name, ref members, ..
        } = tree.node(node).kind
        else {
            return;
        };

        self.token(tree.node(name).span, SemanticKind::Class, true);

        for member in tree.list(members) {
            if matches!(
                tree.node(member.node).kind,
                NodeKind::Function { body: Some(_), .. }
            ) {
                self.function(tree, member.node, scope, true);
            } else {
                self.annotation(tree, member.node, scope);
            }
        }
    }

    fn function(&mut self, tree: &Tree<'_>, node: NodeIndex, parent: usize, class_method: bool) {
        let NodeKind::Function {
            prefix,
            name,
            generics,
            parameters,
            returns,
            body,
            ..
        } = tree.node(node).kind
        else {
            return;
        };

        let type_function =
            prefix.is_some_and(|prefix| tree.token(prefix).bytes(tree.source) == b"type");

        let mut method = false;

        if let Some(name) = name {
            if class_method {
                self.token(tree.node(name).span, SemanticKind::Method, true);
            } else if prefix.is_some_and(|prefix| {
                tree.token(prefix).kind == TokenKind::Keyword(Keyword::Local)
                    || tree.token(prefix).bytes(tree.source) == b"const"
            }) {
                self.declare(
                    tree,
                    name,
                    parent,
                    tree.node(name).span.start,
                    SemanticKind::Function,
                    false,
                );
            } else if type_function {
                self.token(tree.node(name).span, SemanticKind::Function, true);
            } else if let NodeKind::FunctionName {
                ref path,
                method: selected,
                ..
            } = tree.node(name).kind
            {
                let path = tree.list(path);
                let write = path.len() == 1 && selected.is_none();

                for (i, part) in path.iter().enumerate() {
                    if i == 0 {
                        self.reference(tree, part.node, parent, write);
                    } else {
                        self.token(tree.node(part.node).span, SemanticKind::Property, false);
                    }
                }

                if let Some(selected) = selected {
                    self.token(tree.node(selected).span, SemanticKind::Method, true);
                    method = true;
                }
            } else {
                self.reference(tree, name, parent, true);
            }
        }

        let scope = self.scope((!type_function).then_some(parent));

        if method {
            let id = self.bindings.len();
            self.scopes[scope].names.insert("self".into(), vec![id]);

            self.bindings.push(Binding {
                visible: tree.node(parameters).span.start,
                kind: SemanticKind::Parameter,
                loader: false,
                written: false,
            });
        }

        if let Some(generics) = generics {
            self.annotation(tree, generics, scope);
        }

        for parameter in tree.children(parameters) {
            self.bind(
                tree,
                parameter,
                scope,
                tree.node(parameters).span.end,
                SemanticKind::Parameter,
                false,
            );
        }

        if let Some(returns) = returns {
            self.annotation(tree, returns, scope);
        }

        if let Some(body) = body {
            self.visit(tree, body, scope);
        }
    }

    fn annotation(&mut self, tree: &Tree<'_>, node: NodeIndex, scope: usize) {
        match tree.node(node).kind {
            NodeKind::TypeOf { expression, .. } => self.visit(tree, expression, scope),

            NodeKind::TypeAlias {
                name,
                generics,
                annotation,
                ..
            } => {
                self.token(tree.node(name).span, SemanticKind::Type, true);

                if let Some(generics) = generics {
                    self.annotation(tree, generics, scope);
                }

                self.annotation(tree, annotation, scope);
            }

            NodeKind::TypeName {
                namespace,
                name,
                arguments,
                ..
            } => {
                if let Some(namespace) = namespace {
                    self.visit(tree, namespace, scope);
                }

                self.token(tree.node(name).span, SemanticKind::Type, false);

                if let Some(arguments) = arguments {
                    self.annotation(tree, arguments, scope);
                }
            }

            NodeKind::Generic { name, default, .. } => {
                self.token(tree.node(name).span, SemanticKind::TypeParameter, true);

                if let Some(default) = default {
                    self.annotation(tree, default, scope);
                }
            }

            NodeKind::GenericPack { name, .. } => {
                self.token(tree.node(name).span, SemanticKind::TypeParameter, false);
            }

            NodeKind::TypeField {
                key, annotation, ..
            } => {
                self.token(tree.node(key).span, SemanticKind::Property, true);
                self.annotation(tree, annotation, scope);
            }

            NodeKind::TypeIndexer {
                key, annotation, ..
            } => {
                self.annotation(tree, key, scope);
                self.annotation(tree, annotation, scope);
            }

            NodeKind::TypeParameter {
                name, annotation, ..
            } => {
                self.token(tree.node(name).span, SemanticKind::Parameter, true);
                self.annotation(tree, annotation, scope);
            }

            _ => {
                for child in tree.children(node) {
                    self.annotation(tree, child, scope);
                }
            }
        }
    }

    fn local(&mut self, tree: &Tree<'_>, node: NodeIndex, scope: usize) {
        let (bindings, values) = match &tree.node(node).kind {
            NodeKind::Local {
                bindings, values, ..
            }
            | NodeKind::Constant {
                bindings, values, ..
            } => (tree.list(bindings), tree.list(values)),

            _ => return,
        };

        let kinds = values
            .iter()
            .map(|value| {
                let value = value.node;

                let loader = matches!(tree.node(value).kind, NodeKind::Name { .. })
                    && self.loader(tree.text(value), scope, tree.node(value).span.start);

                let import = matches!(tree.node(value).kind, NodeKind::Call { callee, .. }
                if matches!(tree.node(callee).kind, NodeKind::Name { .. })
                    && self.loader(tree.text(callee), scope, tree.node(callee).span.start));

                let kind = if import {
                    SemanticKind::Namespace
                } else if matches!(tree.node(value).kind, NodeKind::Function { .. }) || loader {
                    SemanticKind::Function
                } else {
                    SemanticKind::Variable
                };

                (kind, loader)
            })
            .collect::<Vec<_>>();

        for value in values {
            self.visit(tree, value.node, scope);
        }

        for (i, binding) in bindings.iter().enumerate() {
            let (kind, loader) = kinds
                .get(i)
                .copied()
                .unwrap_or((SemanticKind::Variable, false));

            self.bind(
                tree,
                binding.node,
                scope,
                tree.node(node).span.end,
                kind,
                loader,
            );

            if matches!(tree.node(node).kind, NodeKind::Constant { .. })
                && let NodeKind::Binding { name, .. } = tree.node(binding.node).kind
                && let Some((_, modifiers)) = self
                    .tokens
                    .get_mut(&(tree.node(name).span.start, tree.node(name).span.end))
            {
                *modifiers |= READONLY;
            }
        }
    }

    fn visit(&mut self, tree: &Tree<'_>, node: NodeIndex, scope: usize) {
        match &tree.node(node).kind {
            NodeKind::Name { .. } => self.reference(tree, node, scope, false),
            NodeKind::Local { .. } | NodeKind::Constant { .. } => self.local(tree, node, scope),
            NodeKind::Function { .. } => self.function(tree, node, scope, false),

            NodeKind::If {
                branches,
                otherwise,
                ..
            } => {
                for branch in tree.list(branches) {
                    self.scoped(tree, branch.node, scope);
                }

                if let Some(otherwise) = otherwise {
                    self.scoped(tree, *otherwise, scope);
                }
            }

            NodeKind::While {
                condition, body, ..
            } => {
                self.visit(tree, *condition, scope);
                self.scoped(tree, *body, scope);
            }

            NodeKind::Repeat {
                body, condition, ..
            } => {
                let scope = self.scope(Some(scope));
                self.visit(tree, *body, scope);
                self.visit(tree, *condition, scope);
            }

            NodeKind::NumericFor {
                binding,
                start,
                end,
                step,
                body,
                ..
            } => {
                self.visit(tree, *start, scope);
                self.visit(tree, *end, scope);

                if let Some(step) = step {
                    self.visit(tree, *step, scope);
                }

                let scope = self.scope(Some(scope));

                self.bind(
                    tree,
                    *binding,
                    scope,
                    tree.node(*body).span.start,
                    SemanticKind::Variable,
                    false,
                );

                self.visit(tree, *body, scope);
            }

            NodeKind::GenericFor {
                bindings,
                values,
                body,
                ..
            } => {
                for value in tree.list(values) {
                    self.visit(tree, value.node, scope);
                }

                let scope = self.scope(Some(scope));

                for binding in tree.list(bindings) {
                    self.bind(
                        tree,
                        binding.node,
                        scope,
                        tree.node(*body).span.start,
                        SemanticKind::Variable,
                        false,
                    );
                }

                self.visit(tree, *body, scope);
            }

            NodeKind::Do { body, .. } => self.scoped(tree, *body, scope),

            NodeKind::Assignment {
                targets, values, ..
            } => {
                for value in tree.list(values) {
                    self.visit(tree, value.node, scope);
                }

                for target in tree.list(targets) {
                    if matches!(tree.node(target.node).kind, NodeKind::Name { .. }) {
                        self.reference(tree, target.node, scope, true);
                    } else {
                        self.visit(tree, target.node, scope);
                    }
                }
            }

            NodeKind::CompoundAssignment { target, value, .. } => {
                self.visit(tree, *value, scope);

                if matches!(tree.node(*target).kind, NodeKind::Name { .. }) {
                    self.reference(tree, *target, scope, true);
                } else {
                    self.visit(tree, *target, scope);
                }
            }

            _ => self.expression(tree, node, scope),
        }
    }

    fn expression(&mut self, tree: &Tree<'_>, node: NodeIndex, scope: usize) {
        match &tree.node(node).kind {
            NodeKind::Conditional {
                condition,
                truthy,
                falsy,
                ..
            } => {
                let branch = self.scope(Some(scope));
                self.visit(tree, *condition, branch);
                self.visit(tree, *truthy, branch);
                self.visit(tree, *falsy, scope);
            }

            NodeKind::Call { callee, arguments } => {
                if matches!(tree.node(*callee).kind, NodeKind::Name { .. })
                    && let Some(argument) = tree.children(*arguments).first()
                {
                    self.imports.push(Import {
                        span: tree.node(*argument).span,
                        callee: tree.node(*callee).span,
                        scope,
                    });
                }

                self.visit(tree, *callee, scope);

                for argument in tree.children(*arguments) {
                    self.visit(tree, argument, scope);
                }
            }

            NodeKind::Field { receiver, name, .. } => {
                self.visit(tree, *receiver, scope);
                self.token(tree.node(*name).span, SemanticKind::Property, false);
            }

            NodeKind::MethodCall {
                receiver,
                method,
                arguments,
                instantiation,
                ..
            } => {
                self.visit(tree, *receiver, scope);
                self.token(tree.node(*method).span, SemanticKind::Method, false);

                if let Some(instantiation) = instantiation {
                    self.annotation(tree, *instantiation, scope);
                }

                self.visit(tree, *arguments, scope);
            }

            NodeKind::TableField {
                key,
                value,
                opening,
                ..
            } => {
                if let Some(key) = key {
                    if opening.is_some() {
                        self.visit(tree, *key, scope);
                    } else {
                        self.token(tree.node(*key).span, SemanticKind::Property, true);
                    }
                }

                self.visit(tree, *value, scope);
            }

            NodeKind::Assertion {
                expression,
                annotation,
                ..
            }
            | NodeKind::Instantiate {
                expression,
                arguments: annotation,
            } => {
                self.visit(tree, *expression, scope);
                self.annotation(tree, *annotation, scope);
            }

            NodeKind::TypeAlias { .. } => self.annotation(tree, node, scope),

            NodeKind::Class { .. } => self.class(tree, node, scope),

            NodeKind::Declaration { declaration, .. } => {
                if let NodeKind::Binding {
                    name, annotation, ..
                } = tree.node(*declaration).kind
                {
                    self.token(tree.node(name).span, SemanticKind::Variable, true);

                    if let Some(annotation) = annotation {
                        self.annotation(tree, annotation, scope);
                    }
                } else {
                    self.visit(tree, *declaration, scope);
                }
            }

            _ => {
                for child in tree.children(node) {
                    self.visit(tree, child, scope);
                }
            }
        }
    }

    pub(crate) fn import_at(&self, offset: usize) -> Option<&Import> {
        self.imports
            .iter()
            .filter(|site| site.span.start <= offset && offset <= site.span.end)
            .min_by_key(|site| site.span.end - site.span.start)
    }

    pub(crate) fn literal_at(&self, offset: usize) -> Option<Span> {
        self.literals
            .iter()
            .copied()
            .find(|span| span.start <= offset && offset <= span.end)
    }

    pub(crate) fn tokens(&self) -> impl Iterator<Item = (Span, SemanticKind, u32)> + '_ {
        self.tokens
            .iter()
            .map(|(&(start, end), &(kind, modifiers))| (Span { start, end }, kind, modifiers))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generic_pack_references_are_not_declarations() {
        for suffix in ["", "\nlocal ="] {
            let source = format!(
                "type Callback<Arguments...> = (Arguments...) -> Arguments...\n\
                 local function relay<Values...>(...: Values...): Values... return ... end{suffix}"
            );

            let tree = vermis::parse(source.as_bytes());
            assert_eq!(tree.diagnostics.is_empty(), suffix.is_empty(), "{source}");
            let index = Index::new(&tree);

            for name in ["Arguments", "Values"] {
                let tokens: Vec<_> = index
                    .tokens()
                    .filter(|(span, _, _)| span.bytes(source.as_bytes()) == name.as_bytes())
                    .map(|(_, kind, modifiers)| (kind as u32, modifiers))
                    .collect();

                assert_eq!(
                    tokens,
                    [
                        (SemanticKind::TypeParameter as u32, DECLARATION),
                        (SemanticKind::TypeParameter as u32, 0),
                        (SemanticKind::TypeParameter as u32, 0),
                    ],
                    "{name}: {source}"
                );
            }
        }
    }

    #[test]
    fn conditional_bindings_keep_import_scope() {
        for keyword in ["local", "const"] {
            for suffix in ["", "\nlocal ="] {
                let source = format!(
                    "local loader = require\n\
                     local result = if {keyword} loader = false then loader('./hidden') else loader('./outer')\n\
                     loader('./after'){suffix}"
                );

                let tree = vermis::parse(source.as_bytes());
                assert_eq!(tree.diagnostics.is_empty(), suffix.is_empty(), "{source}");
                let index = Index::new(&tree);

                let imports: Vec<_> = index
                    .imports
                    .iter()
                    .map(|site| site.span.bytes(source.as_bytes()))
                    .collect();

                assert_eq!(imports, [b"'./outer'", b"'./after'"], "{source}");

                let tokens: Vec<_> = index
                    .tokens()
                    .filter(|(span, _, _)| span.bytes(source.as_bytes()) == b"loader")
                    .map(|(_, kind, modifiers)| (kind as u32, modifiers))
                    .collect();

                assert_eq!(
                    tokens,
                    [
                        (SemanticKind::Function as u32, DECLARATION),
                        (
                            SemanticKind::Variable as u32,
                            DECLARATION | if keyword == "const" { READONLY } else { 0 },
                        ),
                        (SemanticKind::Variable as u32, 0),
                        (SemanticKind::Function as u32, 0),
                        (SemanticKind::Function as u32, 0),
                    ],
                    "{source}"
                );
            }
        }
    }

    #[test]
    fn class_methods_visit_runtime_bodies() {
        for suffix in ["", "\nlocal ="] {
            let source = format!(
                "local loader = require\n\
                 class Widget\n\
                 function require(self, argument: string)\n\
                 local imported = loader('./inside')\n\
                 local loader = false\n\
                 loader('./hidden')\n\
                 return imported, argument, self\n\
                 end\n\
                 end\n\
                 require('./outside'){suffix}"
            );

            let tree = vermis::parse(source.as_bytes());
            assert_eq!(tree.diagnostics.is_empty(), suffix.is_empty(), "{source}");
            let index = Index::new(&tree);

            let imports: Vec<_> = index
                .imports
                .iter()
                .map(|site| site.span.bytes(source.as_bytes()))
                .collect();

            assert_eq!(
                imports,
                [b"'./inside'".as_slice(), b"'./outside'"],
                "{source}"
            );

            for (name, expected) in [
                ("imported", SemanticKind::Namespace),
                ("argument", SemanticKind::Parameter),
                ("self", SemanticKind::Parameter),
            ] {
                let tokens: Vec<_> = index
                    .tokens()
                    .filter(|(span, _, _)| span.bytes(source.as_bytes()) == name.as_bytes())
                    .map(|(_, kind, modifiers)| (kind as u32, modifiers))
                    .collect();

                assert_eq!(
                    tokens,
                    [(expected as u32, DECLARATION), (expected as u32, 0)],
                    "{name}: {source}"
                );
            }

            let method = source.find("require(self").unwrap();

            assert!(index.tokens().any(|(span, kind, modifiers)| {
                span.start == method
                    && kind as u32 == SemanticKind::Method as u32
                    && modifiers == DECLARATION
            }));
        }
    }

    #[test]
    fn type_indexer_keys_are_annotations() {
        for suffix in ["", "\nlocal ="] {
            let source = format!(
                "local Types = require('./types')\n\
                 type Simple = {{ [string]: number, field: boolean }}\n\
                 type Qualified = {{ [Types.Key]: number }}\n\
                 type Dynamic = {{ [typeof(require('./key'))]: boolean }}{suffix}"
            );

            let tree = vermis::parse(source.as_bytes());
            assert_eq!(tree.diagnostics.is_empty(), suffix.is_empty(), "{source}");
            let index = Index::new(&tree);

            let properties: Vec<_> = index
                .tokens()
                .filter(|(_, kind, _)| *kind as u32 == SemanticKind::Property as u32)
                .map(|(span, _, modifiers)| (span.bytes(source.as_bytes()), modifiers))
                .collect();

            assert_eq!(properties, [(b"field".as_slice(), DECLARATION)], "{source}");

            for name in ["string", "Key"] {
                assert!(
                    index.tokens().any(|(span, kind, modifiers)| {
                        span.bytes(source.as_bytes()) == name.as_bytes()
                            && kind as u32 == SemanticKind::Type as u32
                            && modifiers == 0
                    }),
                    "{name}: {source}"
                );
            }

            let namespaces: Vec<_> = index
                .tokens()
                .filter(|(span, _, _)| span.bytes(source.as_bytes()) == b"Types")
                .map(|(_, kind, modifiers)| (kind as u32, modifiers))
                .collect();

            assert_eq!(
                namespaces,
                [
                    (SemanticKind::Namespace as u32, DECLARATION),
                    (SemanticKind::Namespace as u32, 0),
                ],
                "{source}"
            );

            let imports: Vec<_> = index
                .imports
                .iter()
                .map(|site| site.span.bytes(source.as_bytes()))
                .collect();

            assert_eq!(imports, [b"'./types'".as_slice(), b"'./key'"], "{source}");
        }
    }
}
