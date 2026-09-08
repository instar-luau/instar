//! Lexical facts over one retained syntax revision, without type checking.

use std::collections::BTreeMap;

use la_arena::{Arena, Idx};
use text_size::TextRange;

use crate::syntax::{Parse, SyntaxKind as K, SyntaxNode, SyntaxToken, string_bytes};

pub type ScopeId = Idx<Scope>;
pub type DeclarationId = Idx<Declaration>;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ScopeKind {
    Module,
    Block,
    Function,
    TypeFunction,
    Class,
    Type,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum Namespace {
    Value,
    Type,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DeclarationKind {
    Local,
    LocalFunction,
    Parameter,
    Loop,
    ImplicitSelf,
    TypeAlias,
    TypeParameter,
    TypePackParameter,
    TypeFunction,
    Class,
    Global,
    DeclaredFunction,
    ExternType,
}

#[derive(Debug)]
pub struct Scope {
    pub parent: Option<ScopeId>,
    pub kind: ScopeKind,
    pub range: TextRange,
}

#[derive(Debug)]
pub struct Declaration {
    pub name: String,
    pub range: TextRange,
    pub scope: ScopeId,
    pub namespace: Namespace,
    pub kind: DeclarationKind,
    pub is_const: bool,
    pub is_exported: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Access {
    Read,
    Write,
    ReadWrite,
}

#[derive(Debug)]
pub struct Reference {
    pub name: String,
    pub range: TextRange,
    pub scope: ScopeId,
    pub namespace: Namespace,
    pub access: Access,
    /// None denotes a name not bound lexically in this module, not a type error.
    pub declaration: Option<DeclarationId>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Capture {
    pub function: ScopeId,
    pub declaration: DeclarationId,
}

/// A call to a lexically unshadowed `require`; runtime global mutation is not evaluated.
#[derive(Debug)]
pub struct RequireSite {
    pub range: TextRange,
    pub scope: ScopeId,
    /// Exactly one argument, retaining structural syntax for the resolver.
    pub argument: Option<SyntaxNode>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SemanticError {
    pub range: TextRange,
    pub message: String,
}

#[derive(Debug)]
pub struct Semantics {
    parse: Parse,
    scopes: Arena<Scope>,
    declarations: Arena<Declaration>,
    references: Vec<Reference>,
    captures: Vec<Capture>,
    requires: Vec<RequireSite>,
    errors: Vec<SemanticError>,
}

impl Semantics {
    #[must_use]
    pub fn new(parse: Parse) -> Self {
        let root = parse.syntax();
        let mut facts = Self {
            parse,
            scopes: Arena::default(),
            declarations: Arena::default(),
            references: Vec::new(),
            captures: Vec::new(),
            requires: Vec::new(),
            errors: Vec::new(),
        };
        let module = facts.scopes.alloc(Scope {
            parent: None,
            kind: ScopeKind::Module,
            range: facts.parse.source_range(root.text_range()),
        });
        let mut builder = Builder {
            facts,
            names: BTreeMap::new(),
            exports: BTreeMap::new(),
            module_return: None,
        };
        for node in root.children() {
            if node.kind() == K::Block {
                builder.children(&node, module);
            } else {
                builder.visit(&node, module);
            }
        }
        builder.facts
    }

    #[must_use]
    pub const fn parse(&self) -> &Parse {
        &self.parse
    }

    /// False means syntax or semantic errors prevent complete facts.
    #[must_use]
    pub fn is_complete(&self) -> bool {
        self.parse.errors().is_empty() && self.errors.is_empty()
    }

    #[must_use]
    pub fn errors(&self) -> &[SemanticError] {
        &self.errors
    }

    #[must_use]
    pub const fn scopes(&self) -> &Arena<Scope> {
        &self.scopes
    }

    #[must_use]
    pub const fn declarations(&self) -> &Arena<Declaration> {
        &self.declarations
    }

    #[must_use]
    pub fn references(&self) -> &[Reference] {
        &self.references
    }

    #[must_use]
    pub fn captures(&self) -> &[Capture] {
        &self.captures
    }

    #[must_use]
    pub fn requires(&self) -> &[RequireSite] {
        &self.requires
    }
}

fn child(node: &SyntaxNode, kind: K) -> Option<SyntaxNode> {
    node.children().find(|node| node.kind() == kind)
}

fn identifier(node: &SyntaxNode) -> Option<SyntaxToken> {
    node.descendants_with_tokens()
        .filter_map(rowan::NodeOrToken::into_token)
        .find(|token| token.kind() == K::Identifier)
}

fn has_token(node: &SyntaxNode, text: &str) -> bool {
    node.children_with_tokens()
        .filter_map(rowan::NodeOrToken::into_token)
        .any(|token| token.text() == text)
}

fn attribute_literal(node: &SyntaxNode) -> bool {
    let mut pending = vec![node.clone()];
    while let Some(node) = pending.pop() {
        match node.kind() {
            K::LiteralExpression => {
                if node
                    .children_with_tokens()
                    .filter_map(rowan::NodeOrToken::into_token)
                    .any(|token| token.kind() == K::Number && token.text().ends_with('i'))
                {
                    return false;
                }
            }
            K::TableExpression => {
                for field in node.children() {
                    if field.kind() != K::TableField || has_token(&field, "[") {
                        return false;
                    }
                    pending.extend(field.children().filter(|node| node.kind() != K::Name));
                }
            }
            _ => return false,
        }
    }
    true
}

struct Builder {
    facts: Semantics,
    names: BTreeMap<(ScopeId, Namespace, String), DeclarationId>,
    exports: BTreeMap<String, TextRange>,
    module_return: Option<TextRange>,
}

impl Builder {
    fn error(&mut self, node: &SyntaxNode, message: &str) {
        let mut tokens = node
            .descendants_with_tokens()
            .filter_map(rowan::NodeOrToken::into_token)
            .filter(|token| !token.kind().is_trivia());
        let span = tokens.next().map_or(node.text_range(), |first| {
            TextRange::new(
                first.text_range().start(),
                tokens.last().unwrap_or(first).text_range().end(),
            )
        });
        let range = self.facts.parse.source_range(span);
        if !self
            .facts
            .errors
            .iter()
            .any(|error| error.range == range && error.message == message)
        {
            self.facts.errors.push(SemanticError {
                range,
                message: message.into(),
            });
        }
    }

    fn ancestor(&self, mut scope: ScopeId, kind: ScopeKind) -> Option<ScopeId> {
        loop {
            if self.facts.scopes[scope].kind == kind {
                return Some(scope);
            }
            scope = self.facts.scopes[scope].parent?;
        }
    }

    fn is_within(&self, mut scope: ScopeId, owner: ScopeId) -> bool {
        loop {
            if scope == owner {
                return true;
            }
            let Some(parent) = self.facts.scopes[scope].parent else {
                return false;
            };
            scope = parent;
        }
    }

    fn scope(&mut self, parent: ScopeId, node: &SyntaxNode, kind: ScopeKind) -> ScopeId {
        self.facts.scopes.alloc(Scope {
            parent: Some(parent),
            kind,
            range: self.facts.parse.source_range(node.text_range()),
        })
    }

    fn lookup(
        &self,
        mut scope: ScopeId,
        namespace: Namespace,
        name: &str,
    ) -> Option<DeclarationId> {
        loop {
            if let Some(id) = self.names.get(&(scope, namespace, name.to_owned())) {
                return Some(*id);
            }
            scope = self.facts.scopes[scope].parent?;
        }
    }

    fn declare(
        &mut self,
        node: &SyntaxNode,
        scope: ScopeId,
        kind: DeclarationKind,
        namespace: Namespace,
    ) {
        if let Some(token) = identifier(node) {
            self.insert(
                token.text().to_owned(),
                self.facts.parse.source_range(token.text_range()),
                scope,
                kind,
                namespace,
            );
            let id = self
                .lookup(scope, namespace, token.text())
                .expect("inserted binding");
            let owner = node.ancestors().find(|owner| {
                matches!(
                    owner.kind(),
                    K::LocalStatement
                        | K::ConditionalBinding
                        | K::ClassStatement
                        | K::TypeAlias
                        | K::TypeFunction
                        | K::Block
                )
            });
            let exported = matches!(
                kind,
                DeclarationKind::Local
                    | DeclarationKind::LocalFunction
                    | DeclarationKind::Class
                    | DeclarationKind::TypeAlias
                    | DeclarationKind::TypeFunction
            ) && owner
                .as_ref()
                .and_then(SyntaxNode::parent)
                .is_some_and(|node| node.kind() == K::ExportStatement);
            let constant = (matches!(
                kind,
                DeclarationKind::Local | DeclarationKind::LocalFunction
            ) && owner.is_some_and(|node| has_token(&node, "const")))
                || kind == DeclarationKind::Class
                || (exported && kind == DeclarationKind::LocalFunction);
            self.facts.declarations[id].is_const = constant;
            self.facts.declarations[id].is_exported = exported;
        }
    }

    fn insert(
        &mut self,
        name: String,
        range: TextRange,
        scope: ScopeId,
        kind: DeclarationKind,
        namespace: Namespace,
    ) {
        let id = self.facts.declarations.alloc(Declaration {
            name: name.clone(),
            range,
            scope,
            namespace,
            kind,
            is_const: false,
            is_exported: false,
        });
        self.names.insert((scope, namespace, name), id);
    }

    fn reference(
        &mut self,
        node: &SyntaxNode,
        scope: ScopeId,
        namespace: Namespace,
        access: Access,
    ) {
        let Some(token) = identifier(node) else {
            return;
        };
        let declaration = self.lookup(scope, namespace, token.text());
        self.facts.references.push(Reference {
            name: token.text().to_owned(),
            range: self.facts.parse.source_range(token.text_range()),
            scope,
            namespace,
            access,
            declaration,
        });
        if let Some(id) = declaration {
            if namespace == Namespace::Value
                && access != Access::Read
                && self.facts.declarations[id].is_const
            {
                let declaration = &self.facts.declarations[id];
                let message = if declaration.kind == DeclarationKind::Class {
                    let line = self
                        .facts
                        .parse
                        .source()
                        .position(
                            declaration.range.start(),
                            crate::source::PositionEncoding::Utf8,
                        )
                        .expect("declaration boundary")
                        .line
                        + 1;
                    format!(
                        "'{}' refers to a class and cannot be used as a variable name (defined on line {line})",
                        token.text()
                    )
                } else {
                    format!(
                        "Variable '{}' is constant and may not be reassigned",
                        token.text()
                    )
                };
                self.error(node, &message);
            }
            if namespace == Namespace::Value
                && let Some(boundary) = self.ancestor(scope, ScopeKind::TypeFunction)
                && !self.is_within(self.facts.declarations[id].scope, boundary)
            {
                self.error(
                    node,
                    &format!(
                        "Type function cannot reference outer local '{}'",
                        token.text()
                    ),
                );
            }
        }
        if let Some(declaration) = declaration.filter(|id| {
            namespace == Namespace::Value
                && !matches!(
                    self.facts.declarations[*id].kind,
                    DeclarationKind::Global | DeclarationKind::DeclaredFunction
                )
        }) {
            let owner = self.facts.declarations[declaration].scope;
            let mut current = scope;
            while current != owner {
                let entry = &self.facts.scopes[current];
                if matches!(entry.kind, ScopeKind::Function | ScopeKind::TypeFunction) {
                    let capture = Capture {
                        function: current,
                        declaration,
                    };
                    if !self.facts.captures.contains(&capture) {
                        self.facts.captures.push(capture);
                    }
                }
                let Some(parent) = entry.parent else { break };
                current = parent;
            }
        }
    }

    fn children(&mut self, node: &SyntaxNode, scope: ScopeId) {
        for node in node.children() {
            self.visit(&node, scope);
        }
    }

    fn bindings(&mut self, node: &SyntaxNode, scope: ScopeId, kind: DeclarationKind) {
        for binding in node.children().filter(|node| node.kind() == K::Binding) {
            if let Some(name) = child(&binding, K::Name) {
                self.declare(&name, scope, kind, Namespace::Value);
            }
        }
    }

    fn function(
        &mut self,
        body: &SyntaxNode,
        parent: ScopeId,
        method: bool,
        local: Option<&SyntaxNode>,
    ) {
        // Parser::parseFunctionBody resolves the entire signature before introducing
        // the recursive function name, implicit self and parameters.
        let signature_kind = if body
            .parent()
            .is_some_and(|node| node.kind() == K::TypeFunction)
        {
            ScopeKind::TypeFunction
        } else {
            ScopeKind::Type
        };
        let signature = self.scope(parent, body, signature_kind);
        for node in body.children().filter(|node| node.kind() != K::Block) {
            self.visit(&node, signature);
        }
        if let Some(name) = local {
            self.declare(
                name,
                parent,
                DeclarationKind::LocalFunction,
                Namespace::Value,
            );
        }
        let scope = self.scope(signature, body, ScopeKind::Function);
        if method {
            self.insert(
                "self".into(),
                TextRange::empty(self.facts.parse.source_range(body.text_range()).start()),
                scope,
                DeclarationKind::ImplicitSelf,
                Namespace::Value,
            );
        }
        for node in body.children() {
            match node.kind() {
                K::Parameters => self.bindings(&node, scope, DeclarationKind::Parameter),
                K::Block => self.children(&node, scope),
                _ => {}
            }
        }
    }

    fn local(&mut self, node: &SyntaxNode, scope: ScopeId) {
        if let Some(function) = child(node, K::FunctionStatement) {
            if let Some(body) = child(&function, K::FunctionBody) {
                self.function(&body, scope, false, child(&function, K::Name).as_ref());
            }
        } else {
            self.children(node, scope);
            self.bindings(node, scope, DeclarationKind::Local);
        }
    }

    fn assignment(&mut self, node: &SyntaxNode, scope: ScopeId) {
        let mut lists = node
            .children()
            .filter(|node| node.kind() == K::ExpressionList);
        if let Some(left) = lists.next() {
            let access = if has_token(node, "=") {
                Access::Write
            } else {
                Access::ReadWrite
            };
            for target in left.children() {
                if target.kind() == K::NameExpression {
                    self.reference(&target, scope, Namespace::Value, access);
                } else {
                    // Assigning t.x or t[i] reads t and i, not a variable named x.
                    self.visit(&target, scope);
                }
            }
        }
        for right in lists {
            self.visit(&right, scope);
        }
    }

    fn call(&mut self, node: &SyntaxNode, scope: ScopeId) {
        if self.ancestor(scope, ScopeKind::TypeFunction).is_some() {
            return;
        }
        if node.children().next().is_some_and(|mut callee| {
            while matches!(
                callee.kind(),
                K::ParenthesizedExpression
                    | K::TypeAssertionExpression
                    | K::InstantiationExpression
            ) {
                let Some(inner) = callee.children().next() else {
                    return false;
                };
                callee = inner;
            }
            callee.kind() == K::NameExpression
                && identifier(&callee).is_some_and(|token| token.text() == "require")
        }) && self.lookup(scope, Namespace::Value, "require").is_none()
        {
            let argument = child(node, K::Arguments).and_then(|args| {
                let container = child(&args, K::ExpressionList).unwrap_or(args);
                let mut arguments = container.children();
                let first = arguments.next();
                if arguments.next().is_none() {
                    first
                } else {
                    None
                }
            });
            self.facts.requires.push(RequireSite {
                range: self.facts.parse.source_range(node.text_range()),
                scope,
                argument,
            });
        }
    }

    fn export(&mut self, node: &SyntaxNode, scope: ScopeId) {
        self.children(node, scope);
        let declarations: Vec<_> = self
            .facts
            .declarations
            .iter()
            .filter(|(_, declaration)| {
                declaration.scope == scope
                    && declaration.namespace == Namespace::Value
                    && self
                        .facts
                        .parse
                        .source_range(node.text_range())
                        .contains_range(declaration.range)
                    && declaration.is_exported
            })
            .map(|(_, declaration)| (declaration.name.clone(), declaration.range))
            .collect();
        if !declarations.is_empty() && self.module_return.is_some() {
            self.error(
                node,
                "Exporting values is not compatible with top-level return (export/return conflict)",
            );
        }
        for (name, range) in declarations {
            if self.exports.insert(name.clone(), range).is_some() {
                self.error(node, &format!("Duplicate exported identifier '{name}'"));
            }
        }
    }

    fn class(&mut self, node: &SyntaxNode, scope: ScopeId) {
        if let Some(name) = child(node, K::Name) {
            if identifier(&name)
                .and_then(|name| self.lookup(scope, Namespace::Type, name.text()))
                .is_some_and(|id| self.facts.declarations[id].kind == DeclarationKind::Class)
                && let Some(token) = identifier(&name)
            {
                self.error(
                    &name,
                    &format!(
                        "A class named '{}' has already been declared in this module",
                        token.text()
                    ),
                );
            }
            self.declare(&name, scope, DeclarationKind::Class, Namespace::Value);
            self.declare(&name, scope, DeclarationKind::Class, Namespace::Type);
        }
        if let Some(base) = child(node, K::ClassBase) {
            self.children(&base, scope);
        }
        let class = self.scope(scope, node, ScopeKind::Class);
        let mut names = BTreeMap::new();
        if let Some(body) = child(node, K::ClassBody) {
            for member in body.children() {
                if let Some(name_node) = child(&member, K::Name)
                    && let Some(name) = identifier(&name_node)
                {
                    if names
                        .insert(name.text().to_owned(), name.text_range())
                        .is_some()
                    {
                        self.error(
                            &name_node,
                            &format!("Duplicate class member '{}'", name.text()),
                        );
                    }
                    let method = member.kind() == K::ClassMethod;
                    let permitted = [
                        "__init",
                        "__call",
                        "__concat",
                        "__unm",
                        "__add",
                        "__sub",
                        "__mul",
                        "__div",
                        "__mod",
                        "__pow",
                        "__tostring",
                        "__eq",
                        "__lt",
                        "__le",
                        "__iter",
                        "__len",
                        "__idiv",
                    ];
                    if name.text() == "new" {
                        self.error(&name_node, if method {
                            "Class methods cannot be named 'new'.  Name it '__init' to define a constructor."
                        } else {
                            "Class properties cannot be named 'new'. Define a method named '__init' to define a constructor."
                        });
                    } else if name.text().starts_with("__") {
                        if !method {
                            self.error(&name_node, "Class properties cannot start with '__'");
                        } else if ["__index", "__newindex", "__mode", "__metatable", "__type"]
                            .contains(&name.text())
                        {
                            self.error(
                                &name_node,
                                &format!("Classes cannot define '{}' as a metamethod", name.text()),
                            );
                        } else if !permitted.contains(&name.text()) {
                            self.error(&name_node, &format!("Cannot use '{}' as a method name: names starting with '__' are reserved", name.text()));
                        }
                    }
                }
                if let Some(body) = child(&member, K::FunctionBody) {
                    if let Some(params) = child(&body, K::Parameters)
                        && let Some(first) = child(&params, K::Binding)
                        && identifier(&first).is_some_and(|name| name.text() == "self")
                        && child(&first, K::TypeAnnotation).is_some()
                    {
                        self.error(&first, "class method self cannot be annotated");
                    }
                    self.function(&body, class, false, None);
                } else {
                    self.children(&member, class);
                }
            }
        }
    }

    fn declared_function(&mut self, node: &SyntaxNode, parent: ScopeId, method: bool) {
        let scope = self.scope(parent, node, ScopeKind::Type);
        for part in node.children() {
            if part.kind() == K::Parameters {
                for (index, binding) in part
                    .children()
                    .filter(|node| node.kind() == K::Binding)
                    .enumerate()
                {
                    let self_parameter =
                        identifier(&binding).is_some_and(|name| name.text() == "self");
                    let annotated = child(&binding, K::TypeAnnotation).is_some();
                    if method && index == 0 {
                        if !self_parameter || annotated {
                            self.error(
                                &binding,
                                "extern method requires unannotated self as its first parameter",
                            );
                        }
                    } else if !annotated {
                        self.error(&binding, "declaration parameter must be annotated");
                    }
                }
                if method && child(&part, K::Binding).is_none() {
                    self.error(&part, "extern method requires self");
                }
            }
            self.visit(&part, scope);
        }
        if !method && let Some(name) = child(node, K::Name) {
            self.declare(
                &name,
                parent,
                DeclarationKind::DeclaredFunction,
                Namespace::Value,
            );
        }
    }

    fn attributes(&mut self, node: &SyntaxNode) {
        for attribute in node.children().filter(|node| node.kind() == K::Attribute) {
            let name = child(&attribute, K::Name)
                .and_then(|node| identifier(&node))
                .map(|token| token.text().to_owned());
            let arguments = child(&attribute, K::AttributeArguments)
                .and_then(|node| child(&node, K::Arguments));
            let Some(arguments) = arguments else { continue };
            let container = child(&arguments, K::ExpressionList).unwrap_or(arguments);
            let values: Vec<_> = container.children().collect();
            for value in &values {
                if !attribute_literal(value) {
                    self.error(
                        value,
                        "attribute arguments must be literals or literal tables",
                    );
                }
            }
            if name.as_deref() == Some("deprecated") {
                if values.len() != 1 || values[0].kind() != K::TableExpression {
                    self.error(
                        &attribute,
                        "deprecated accepts one table with string use/reason fields",
                    );
                    continue;
                }
                for field in values[0]
                    .children()
                    .filter(|node| node.kind() == K::TableField)
                {
                    let name = child(&field, K::Name).and_then(|node| identifier(&node));
                    let string = child(&field, K::LiteralExpression).is_some_and(|node| {
                        node.children_with_tokens()
                            .filter_map(rowan::NodeOrToken::into_token)
                            .any(|token| {
                                token.kind() == K::String
                                    && string_bytes(&self.facts.parse, &token).is_ok()
                            })
                    });
                    if !name.is_some_and(|name| matches!(name.text(), "use" | "reason")) || !string
                    {
                        self.error(
                            &field,
                            "deprecated fields must be use or reason with string values",
                        );
                    }
                }
            }
        }
    }

    #[expect(
        clippy::too_many_lines,
        reason = "syntax-role dispatch keeps binding and scope ownership explicit"
    )]
    fn visit(&mut self, node: &SyntaxNode, scope: ScopeId) {
        // Pratt chains can be arbitrarily deep without nesting the source grammar.
        // Walk expressions iteratively; recursive calls below follow lexical scopes.
        let mut pending = vec![node.clone()];
        while let Some(current) = pending.pop() {
            let node = &current;
            // A malformed statement may hide declarations: never mine its body for facts.
            if !self.facts.parse.errors().is_empty()
                && node.descendants().any(|node| node.kind() == K::Error)
            {
                continue;
            }
            if node.kind() == K::CallExpression {
                self.call(node, scope);
            }
            if let Some(attributes) = child(node, K::Attributes) {
                self.attributes(&attributes);
            }
            match node.kind() {
                K::Error | K::ErrorStatement | K::ErrorExpression | K::ErrorType | K::Name => {}
                K::Attributes => self.attributes(node),
                K::ExportStatement => self.export(node, scope),
                K::ClassStatement => self.class(node, scope),
                K::TypeFunction => {
                    if let Some(name) = child(node, K::Name) {
                        self.declare(&name, scope, DeclarationKind::TypeFunction, Namespace::Type);
                    }
                    if let Some(body) = child(node, K::FunctionBody) {
                        self.function(&body, scope, false, None);
                    }
                }
                K::DeclareGlobal => {
                    self.children(node, scope);
                    if let Some(name) = child(node, K::Name) {
                        self.declare(&name, scope, DeclarationKind::Global, Namespace::Value);
                    }
                }
                K::DeclareFunction => self.declared_function(node, scope, false),
                K::ExternMethod => self.declared_function(node, scope, true),
                K::DeclareExtern => {
                    if let Some(name) = child(node, K::Name) {
                        self.declare(&name, scope, DeclarationKind::ExternType, Namespace::Type);
                    }
                    self.children(node, scope);
                }
                K::IfBranch => {
                    if let Some(binding) = child(node, K::ConditionalBinding) {
                        self.children(&binding, scope);
                        let branch = self.scope(scope, node, ScopeKind::Block);
                        self.bindings(&binding, branch, DeclarationKind::Local);
                        if let Some(body) = child(node, K::Block) {
                            self.children(&body, branch);
                        }
                    } else {
                        self.children(node, scope);
                    }
                }
                K::ReturnStatement => {
                    if self.ancestor(scope, ScopeKind::Function).is_none()
                        && self.ancestor(scope, ScopeKind::TypeFunction).is_none()
                    {
                        self.module_return = Some(self.facts.parse.source_range(node.text_range()));
                        if !self.exports.is_empty() {
                            self.error(node, "Exporting values is not compatible with top-level return (export/return conflict)");
                        }
                    }
                    self.children(node, scope);
                }
                K::NameExpression => self.reference(node, scope, Namespace::Value, Access::Read),
                K::TypeName => {
                    // A qualified type T.Member refers to value T; Member is a field.
                    let namespace = if has_token(node, ".") {
                        Namespace::Value
                    } else {
                        Namespace::Type
                    };
                    self.reference(node, scope, namespace, Access::Read);
                }
                K::LocalStatement => self.local(node, scope),
                K::AssignmentStatement => self.assignment(node, scope),
                K::FunctionStatement => {
                    let name = child(node, K::FunctionName);
                    if let Some(name) = &name {
                        let access = if has_token(name, ".") || has_token(name, ":") {
                            Access::Read
                        } else {
                            Access::Write
                        };
                        self.reference(name, scope, Namespace::Value, access);
                    }
                    if let Some(body) = child(node, K::FunctionBody) {
                        self.function(
                            &body,
                            scope,
                            name.is_some_and(|name| has_token(&name, ":")),
                            None,
                        );
                    }
                }
                K::FunctionExpression => {
                    if let Some(body) = child(node, K::FunctionBody) {
                        self.function(&body, scope, false, None);
                    }
                }
                K::Block => {
                    let block = self.scope(scope, node, ScopeKind::Block);
                    self.children(node, block);
                }
                K::RepeatStatement => {
                    let block = self.scope(scope, node, ScopeKind::Block);
                    for part in node.children() {
                        if part.kind() == K::Block {
                            self.children(&part, block);
                        } else {
                            self.visit(&part, block);
                        }
                    }
                }
                K::ForStatement => {
                    for part in node.children().filter(|node| node.kind() != K::Block) {
                        self.visit(&part, scope);
                    }
                    let block = self.scope(scope, node, ScopeKind::Block);
                    self.bindings(node, block, DeclarationKind::Loop);
                    if let Some(body) = child(node, K::Block) {
                        self.children(&body, block);
                    }
                }
                K::TypeAlias => {
                    if let Some(name) = child(node, K::Name) {
                        self.declare(&name, scope, DeclarationKind::TypeAlias, Namespace::Type);
                    }
                    let generics = self.scope(scope, node, ScopeKind::Type);
                    self.children(node, generics);
                }
                K::GenericParameters => {
                    for parameter in node.children() {
                        if let Some(name) = child(&parameter, K::Name) {
                            let kind = if parameter.kind() == K::GenericPackParameter {
                                DeclarationKind::TypePackParameter
                            } else {
                                DeclarationKind::TypeParameter
                            };
                            self.declare(&name, scope, kind, Namespace::Type);
                        }
                        self.children(&parameter, scope);
                    }
                }
                K::GenericTypePack => self.reference(node, scope, Namespace::Type, Access::Read),
                // Generic function types have their own type-parameter environment.
                K::FunctionType if child(node, K::GenericParameters).is_some() => {
                    let generics = self.scope(scope, node, ScopeKind::Type);
                    self.children(node, generics);
                }
                _ => {
                    let children: Vec<_> = node.children().collect();
                    pending.extend(children.into_iter().rev());
                }
            }
        }
    }
}
