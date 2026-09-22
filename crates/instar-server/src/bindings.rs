use std::collections::{BTreeMap, HashMap};

use tower_lsp_server::ls_types::{SemanticTokenModifier, SemanticTokenType};
use vermis::{Kind, Parts, Span, TokenKind, Tree, View};

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
                TokenKind::QuotedString
                    | TokenKind::RawString
                    | TokenKind::Error(vermis::LexError::BrokenString)
            ) {
                index.literals.push(token.span);
            }

            let kind = match token.kind {
                TokenKind::Keyword(_) => SemanticKind::Keyword,

                TokenKind::QuotedString | TokenKind::RawString | TokenKind::Interpolated(_) => {
                    SemanticKind::String
                }

                TokenKind::Number => SemanticKind::Number,

                TokenKind::Comment | TokenKind::BlockComment | TokenKind::MarkupComment => {
                    SemanticKind::Comment
                }

                TokenKind::Operator(_) => SemanticKind::Operator,
                _ => continue,
            };

            index.token(token.span, kind, false);
        }

        if let Some(root) = tree.view(tree.root) {
            index.visit(root, 0);
        }

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
        name: View<'_, '_>,
        scope: usize,
        visible: usize,
        kind: SemanticKind,
        loader: bool,
    ) {
        let span = name.span();
        let name = String::from_utf8_lossy(name.text()).into_owned();
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
        node: View<'_, '_>,
        scope: usize,
        visible: usize,
        kind: SemanticKind,
        loader: bool,
    ) {
        if let Some(Parts::Binding { name, annotation }) = node.parts() {
            if let Some(annotation) = annotation {
                self.annotation(annotation, scope);
            }

            self.declare(name, scope, visible, kind, loader);
        } else {
            self.annotation(node, scope);
        }
    }

    fn reference(&mut self, node: View<'_, '_>, scope: usize, write: bool) {
        let span = node.span();
        let name = String::from_utf8_lossy(node.text());
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

    fn scoped(&mut self, node: View<'_, '_>, parent: usize) {
        let scope = self.scope(Some(parent));
        self.visit(node, scope);
    }

    fn function(&mut self, node: View<'_, '_>, parent: usize) {
        let Some(Parts::Function {
            name,
            generics,
            parameters,
            returns,
            body,
            ..
        }) = node.parts()
        else {
            return;
        };

        let mut method = false;

        if let Some(name) = name {
            if node.kind() == Kind::LocalFunction {
                self.declare(
                    name,
                    parent,
                    name.span().start,
                    SemanticKind::Function,
                    false,
                );
            } else if node.kind() == Kind::TypeFunction {
                self.token(name.span(), SemanticKind::Function, true);
            } else if let Some(Parts::FunctionName {
                path,
                method: selected,
            }) = name.parts()
            {
                let write = path.clone().count() == 1 && selected.is_none();

                for (i, part) in path.enumerate() {
                    if i == 0 {
                        self.reference(part, parent, write);
                    } else {
                        self.token(part.span(), SemanticKind::Property, false);
                    }
                }

                if let Some(selected) = selected {
                    self.token(selected.span(), SemanticKind::Method, true);
                    method = true;
                }
            } else {
                self.reference(name, parent, true);
            }
        }

        let scope = self.scope((node.kind() != Kind::TypeFunction).then_some(parent));

        if method {
            let id = self.bindings.len();
            self.scopes[scope].names.insert("self".into(), vec![id]);

            self.bindings.push(Binding {
                visible: parameters.span().start,
                kind: SemanticKind::Parameter,
                loader: false,
                written: false,
            });
        }

        if let Some(generics) = generics {
            self.annotation(generics, scope);
        }

        for parameter in parameters.children() {
            self.bind(
                parameter,
                scope,
                parameters.span().end,
                SemanticKind::Parameter,
                false,
            );
        }

        if let Some(returns) = returns {
            self.annotation(returns, scope);
        }

        if let Some(body) = body {
            self.visit(body, scope);
        }
    }

    fn annotation(&mut self, node: View<'_, '_>, scope: usize) {
        match node.parts() {
            Some(Parts::TypeOf { expression, .. }) => self.visit(expression, scope),

            Some(Parts::TypeName {
                namespace,
                name,
                arguments,
            }) => {
                if let Some(namespace) = namespace {
                    self.visit(namespace, scope);
                }

                self.token(name.span(), SemanticKind::Type, false);

                if let Some(arguments) = arguments {
                    self.annotation(arguments, scope);
                }
            }

            Some(Parts::Generic { name, default }) => {
                self.token(name.span(), SemanticKind::TypeParameter, true);

                if let Some(default) = default {
                    self.annotation(default, scope);
                }
            }

            Some(Parts::TypeField {
                key, annotation, ..
            }) => {
                self.token(key.span(), SemanticKind::Property, true);
                self.annotation(annotation, scope);
            }

            Some(Parts::TypeParameter { name, annotation }) => {
                self.token(name.span(), SemanticKind::Parameter, true);
                self.annotation(annotation, scope);
            }

            _ => {
                for child in node.children() {
                    self.annotation(child, scope);
                }
            }
        }
    }

    fn local(&mut self, node: View<'_, '_>, scope: usize) {
        let Some(Parts::Local { bindings, values }) = node.parts() else {
            return;
        };

        let values = values.collect::<Vec<_>>();

        let kinds = values.iter().map(|value| {
            let loader = value.kind() == Kind::Name && self.loader(value.text(), scope, value.span().start);

            let import = matches!(value.parts(), Some(Parts::Call { callee, .. })
                if callee.kind() == Kind::Name && self.loader(callee.text(), scope, callee.span().start));

            let kind = if import { SemanticKind::Namespace } else if value.kind() == Kind::Function || loader { SemanticKind::Function } else { SemanticKind::Variable };

            (kind, loader)
        }).collect::<Vec<_>>();

        for value in values {
            self.visit(value, scope);
        }

        for (i, binding) in bindings.enumerate() {
            let (kind, loader) = kinds
                .get(i)
                .copied()
                .unwrap_or((SemanticKind::Variable, false));

            self.bind(binding, scope, node.span().end, kind, loader);

            if node.kind() == Kind::Constant
                && let Some(Parts::Binding { name, .. }) = binding.parts()
                && let Some((_, modifiers)) =
                    self.tokens.get_mut(&(name.span().start, name.span().end))
            {
                *modifiers |= READONLY;
            }
        }
    }

    fn visit(&mut self, node: View<'_, '_>, scope: usize) {
        if node.kind() == Kind::Name {
            self.reference(node, scope, false);

            return;
        }

        match node.parts() {
            Some(Parts::Local { .. }) => self.local(node, scope),
            Some(Parts::Function { .. }) => self.function(node, scope),

            Some(Parts::If {
                branches,
                otherwise,
            }) => {
                for branch in branches {
                    self.scoped(branch, scope);
                }

                if let Some(otherwise) = otherwise {
                    self.scoped(otherwise, scope);
                }
            }

            Some(Parts::While { condition, body }) => {
                self.visit(condition, scope);
                self.scoped(body, scope);
            }

            Some(Parts::Repeat { body, condition }) => {
                let scope = self.scope(Some(scope));
                self.visit(body, scope);
                self.visit(condition, scope);
            }

            Some(Parts::NumericFor {
                binding,
                start,
                end,
                step,
                body,
            }) => {
                self.visit(start, scope);
                self.visit(end, scope);

                if let Some(step) = step {
                    self.visit(step, scope);
                }

                let scope = self.scope(Some(scope));

                self.bind(
                    binding,
                    scope,
                    body.span().start,
                    SemanticKind::Variable,
                    false,
                );

                self.visit(body, scope);
            }

            Some(Parts::GenericFor {
                bindings,
                values,
                body,
            }) => {
                for value in values {
                    self.visit(value, scope);
                }

                let scope = self.scope(Some(scope));

                for binding in bindings {
                    self.bind(
                        binding,
                        scope,
                        body.span().start,
                        SemanticKind::Variable,
                        false,
                    );
                }

                self.visit(body, scope);
            }

            Some(Parts::Body { body }) if node.kind() == Kind::Do => self.scoped(body, scope),

            Some(Parts::Assignment {
                targets, values, ..
            }) => {
                for value in values {
                    self.visit(value, scope);
                }

                for target in targets {
                    if target.kind() == Kind::Name {
                        self.reference(target, scope, true);
                    } else {
                        self.visit(target, scope);
                    }
                }
            }

            _ => self.expression(node, scope),
        }
    }

    fn expression(&mut self, node: View<'_, '_>, scope: usize) {
        match node.parts() {
            Some(Parts::Call { callee, arguments }) => {
                if callee.kind() == Kind::Name
                    && let Some(argument) = arguments.children().next()
                {
                    self.imports.push(Import {
                        span: argument.span(),
                        callee: callee.span(),
                        scope,
                    });
                }

                self.visit(callee, scope);

                for argument in arguments.children() {
                    self.visit(argument, scope);
                }
            }

            Some(Parts::Field { receiver, name }) => {
                self.visit(receiver, scope);
                self.token(name.span(), SemanticKind::Property, false);
            }

            Some(Parts::MethodCall {
                receiver,
                method,
                arguments,
                types,
            }) => {
                self.visit(receiver, scope);
                self.token(method.span(), SemanticKind::Method, false);

                if let Some(types) = types {
                    self.annotation(types, scope);
                }

                self.visit(arguments, scope);
            }

            Some(Parts::TableField {
                key,
                value,
                indexed,
            }) => {
                if let Some(key) = key {
                    if indexed {
                        self.visit(key, scope);
                    } else {
                        self.token(key.span(), SemanticKind::Property, true);
                    }
                }

                self.visit(value, scope);
            }

            Some(Parts::Assertion {
                expression,
                annotation,
            }) => {
                self.visit(expression, scope);
                self.annotation(annotation, scope);
            }

            Some(Parts::Instantiate {
                expression,
                arguments,
            }) => {
                self.visit(expression, scope);
                self.annotation(arguments, scope);
            }

            Some(Parts::TypeAlias {
                name,
                generics,
                annotation,
            }) => {
                self.token(name.span(), SemanticKind::Type, true);

                if let Some(generics) = generics {
                    self.annotation(generics, scope);
                }

                self.annotation(annotation, scope);
            }

            Some(Parts::Class { name, members, .. }) => {
                self.token(name.span(), SemanticKind::Class, true);

                for member in members {
                    self.annotation(member, scope);
                }
            }

            Some(Parts::Declaration { name, annotation }) => {
                self.token(name.span(), SemanticKind::Variable, true);
                self.annotation(annotation, scope);
            }

            _ => {
                for child in node.children() {
                    self.visit(child, scope);
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
