use std::{collections::BTreeMap, iter, ops::Range, sync::Arc};

use reqwest::Url;

use vermis::{
    token::TokenKind as LexicalKind,
    tree::{NodeIndex, NodeKind, Tree},
};

use crate::{
    analysis::{
        Access, Analysis, Binding, BindingKind, Fold, FoldKind, Import, ImportExpression, Link,
        Occurrence, Symbol, SymbolKind, Token as SemanticToken, TokenKind,
        color::colors,
        model::{Identity, Resolution},
    },
    syntax::{
        bindings::{Bindings, Category, text},
        expression::unwrap,
        literal,
    },
};

impl Resolution {
    fn new(tree: &Tree<'_>, bindings: &Bindings) -> Self {
        let mut identities: Vec<_> = tree
            .nodes
            .iter()
            .enumerate()
            .filter_map(|(index, node)| {
                if !matches!(node.kind, NodeKind::Name { .. }) {
                    return None;
                }

                let identifier = NodeIndex::new(index);

                let identity = if let Some(local) = bindings.references[index] {
                    Identity::Value(local)
                } else if let Some(alias) = bindings.type_references[index] {
                    Identity::Type(alias)
                } else if bindings.is_type_name[index] {
                    Identity::External(text(tree, identifier))
                } else if bindings.accesses[index] != 0 {
                    Identity::Global(text(tree, identifier))
                } else {
                    return None;
                };

                Some((node.span.start, identity))
            })
            .collect();

        identities.sort_by_key(|(offset, _)| *offset);

        Self {
            identities: identities
                .into_iter()
                .map(|(_, identity)| identity)
                .collect(),
            scopes: bindings
                .locals
                .iter()
                .map(|binding| binding.scope)
                .chain(bindings.types.iter().map(|binding| binding.scope))
                .collect(),
            is_complete: tree.diagnostics.is_empty()
                && !tree
                    .nodes
                    .iter()
                    .any(|node| matches!(node.kind, NodeKind::Error | NodeKind::Missing { .. })),
        }
    }
}

pub(super) fn resolve(source: &str) -> Resolution {
    let prefix = usize::from(source.starts_with('\u{feff}')) * 3;
    let tree = vermis::parse(&source.as_bytes()[prefix..]);

    Resolution::new(&tree, &Bindings::new(&tree))
}

/// Analyzes complete or incomplete source without applying strict lint or format validation.
#[must_use]
pub fn analyze(source: impl Into<Arc<str>>) -> Analysis {
    let source = source.into();
    let prefix = usize::from(source.starts_with('\u{feff}')) * 3;
    let tree = vermis::parse(&source.as_bytes()[prefix..]);
    let bindings = Bindings::new(&tree);

    let mut result = Analysis {
        symbols: outline(&tree, tree.root, prefix),
        tokens: Vec::new(),
        folds: Vec::new(),
        imports: Vec::new(),
        links: Vec::new(),
        colors: colors(&tree, &bindings, prefix),
        bindings: Vec::new(),
        source: Arc::clone(&source),
        selections: iter::once(0..source.len()).collect(),
        resolution: Resolution::new(&tree, &bindings),
    };

    let mut tokens = BTreeMap::new();

    for token in &tree.tokens {
        let kind = match token.kind {
            LexicalKind::Comment | LexicalKind::BlockComment | LexicalKind::MalformedComment => {
                TokenKind::Comment
            }

            LexicalKind::Name => TokenKind::Variable,
            LexicalKind::Number => TokenKind::Number,

            LexicalKind::RawString
            | LexicalKind::QuotedString
            | LexicalKind::MalformedString
            | LexicalKind::InterpolatedStringStart
            | LexicalKind::InterpolatedStringMiddle
            | LexicalKind::InterpolatedStringEnd
            | LexicalKind::InterpolatedStringSimple => TokenKind::String,

            LexicalKind::Keyword(_) => TokenKind::Keyword,
            LexicalKind::Symbol(_) => TokenKind::Operator,
            LexicalKind::Attribute => TokenKind::Decorator,
            _ => continue,
        };

        let range = token.span.start + prefix..token.span.end + prefix;

        if range.is_empty() {
            continue;
        }

        result.selections.push(range.clone());

        tokens.insert(
            token.span.start,
            SemanticToken {
                range,
                kind,
                is_declaration: false,
                is_read_only: false,
                access: Access::None,
            },
        );

        if matches!(kind, TokenKind::Comment | TokenKind::String) {
            documentation(
                &source,
                &(token.span.start + prefix..token.span.end + prefix),
                &mut result.links,
            );
        }
    }

    for local in &bindings.locals {
        let kind = match local.category {
            Category::Local => BindingKind::Local,
            Category::Parameter => BindingKind::Parameter,
            Category::Loop => BindingKind::Loop,
        };

        result.bindings.push(Binding {
            name: local.name.clone(),
            kind,
            declaration: range(&tree, local.declaration, prefix),
            occurrences: Vec::new(),
            can_rename: text(&tree, local.declaration) == local.name,
            is_exported: false,
        });
    }

    for binding in &bindings.types {
        result.bindings.push(Binding {
            name: binding.name.clone(),
            kind: if matches!(tree.node(binding.statement).kind, NodeKind::Generic { .. }) {
                BindingKind::TypeParameter
            } else {
                BindingKind::TypeAlias
            },
            declaration: range(&tree, binding.declaration, prefix),
            occurrences: Vec::new(),
            can_rename: true,
            is_exported: bindings.is_exported(&tree, binding.statement)
                && (matches!(tree.node(binding.statement).kind, NodeKind::TypeAlias { .. })
                    || matches!(tree.node(binding.statement).kind, NodeKind::Function { prefix: Some(prefix), .. }
                        if tree.token(prefix).bytes(tree.source) == b"type")),
        });
    }

    for (index, node) in tree.nodes.iter().enumerate() {
        let identifier = NodeIndex::new(index);

        if node.span.start < node.span.end {
            result.selections.push(range(&tree, identifier, prefix));
        }

        classify(&tree, identifier, &mut tokens);

        if matches!(node.kind, NodeKind::Name { .. }) {
            let local = bindings.references[index];
            let alias = bindings.type_references[index];
            let target = local.or_else(|| alias.map(|alias| bindings.locals.len() + alias));

            if let Some(target) = target {
                let binding = &mut result.bindings[target];
                let is_declaration = binding.declaration == range(&tree, identifier, prefix);
                let access = bindings.accesses[index];

                let access = Access::new(
                    !is_declaration && (access & 1 != 0 || alias.is_some()),
                    is_declaration || access & 2 != 0,
                );

                binding.occurrences.push(Occurrence {
                    range: range(&tree, identifier, prefix),
                    access,
                });

                if let Some(token) = tokens.get_mut(&node.span.start) {
                    token.is_declaration = is_declaration;

                    token.access = access;

                    token.kind = match binding.kind {
                        BindingKind::Parameter => TokenKind::Parameter,
                        BindingKind::TypeAlias => TokenKind::Type,
                        BindingKind::TypeParameter => TokenKind::TypeParameter,

                        BindingKind::Local | BindingKind::Loop => {
                            let local = &bindings.locals[target];

                            token.is_read_only = matches!(
                                tree.node(local.statement).kind,
                                NodeKind::Constant { .. }
                            );

                            if local.value.is_some_and(|value| {
                                matches!(
                                    tree.node(unwrap(&tree, value)).kind,
                                    NodeKind::Function { .. }
                                )
                            }) {
                                TokenKind::Function
                            } else {
                                TokenKind::Variable
                            }
                        }
                    };
                }
            } else if let Some(token) = tokens.get_mut(&node.span.start) {
                token.access = Access::new(
                    bindings.accesses[index] & 1 != 0,
                    bindings.accesses[index] & 2 != 0,
                );
            }
        }

        if is_foldable(&node.kind) && is_multiline(&tree, node.span.start..node.span.end) {
            result.folds.push(Fold {
                range: range(&tree, identifier, prefix),
                kind: FoldKind::Region,
            });
        }

        if let NodeKind::Call { callee, arguments } = node.kind
            && bindings.is_global(&tree, callee, "require")
            && let NodeKind::Arguments { values, .. } = &tree.node(arguments).kind
            && let [argument] = tree.list(values)
        {
            let argument = unwrap(&tree, argument.node);

            let expression = if matches!(tree.node(argument).kind, NodeKind::String { .. }) {
                literal::decode(tree.text(argument))
                    .and_then(|bytes| String::from_utf8(bytes).ok())
                    .map(ImportExpression::Path)
            } else if is_instance(&tree, &bindings, argument) {
                Some(ImportExpression::Instance(text(&tree, argument)))
            } else {
                None
            };

            if let Some(expression) = expression {
                result.imports.push(Import {
                    range: range(&tree, argument, prefix),
                    expression,
                });
            }
        }
    }

    for binding in &mut result.bindings {
        binding
            .occurrences
            .sort_by_key(|occurrence| occurrence.range.start);

        binding
            .occurrences
            .dedup_by(|left, right| left.range == right.range);
    }

    result.tokens = tokens.into_values().collect();
    result.imports.sort_by_key(|import| import.range.start);
    comments(&tree, prefix, &mut result.folds);
    import_folds(&tree, &result.imports, prefix, &mut result.folds);

    result
        .folds
        .sort_by_key(|fold| (fold.range.start, fold.range.end));

    result
        .folds
        .dedup_by(|left, right| left.range == right.range);

    result.links.sort_by_key(|link| link.range.start);

    result
        .selections
        .sort_by_key(|range| (range.end - range.start, range.start));

    result.selections.dedup();

    result
}

impl Analysis {
    /// Returns nested source selections from the smallest token to the complete document.
    #[must_use]
    pub fn selection(&self, offset: usize) -> Vec<Range<usize>> {
        if !self.source.is_char_boundary(offset) {
            return Vec::new();
        }

        let mut ranges: Vec<Range<usize>> = Vec::new();

        for range in &self.selections {
            if range.start <= offset
                && (offset < range.end || offset == self.source.len() && offset == range.end)
                && ranges.last().is_none_or(|previous| {
                    range.start <= previous.start && range.end >= previous.end
                })
            {
                ranges.push(range.clone());
            }
        }

        ranges
    }
}

fn classify(tree: &Tree<'_>, node: NodeIndex, tokens: &mut BTreeMap<usize, SemanticToken>) {
    let mut mark = |name: NodeIndex, kind| {
        if let Some(token) = tokens.get_mut(&tree.node(name).span.start) {
            token.kind = kind;
        }
    };

    match &tree.node(node).kind {
        NodeKind::Field { name, .. }
        | NodeKind::TypeField {
            key: name,
            opening: None,
            ..
        }
        | NodeKind::TableField {
            key: Some(name),
            opening: None,
            ..
        } => mark(*name, TokenKind::Property),

        NodeKind::MethodCall { method, .. } => mark(*method, TokenKind::Method),

        NodeKind::Call { callee, .. } => {
            let callee = unwrap(tree, *callee);

            match tree.node(callee).kind {
                NodeKind::Name { .. } => mark(callee, TokenKind::Function),
                NodeKind::Field { name, .. } => mark(name, TokenKind::Function),
                _ => {}
            }
        }

        NodeKind::TypeName {
            name, namespace, ..
        } => {
            mark(*name, TokenKind::Type);

            if let Some(namespace) = namespace {
                mark(*namespace, TokenKind::Namespace);
            }
        }

        NodeKind::Generic { name, .. } | NodeKind::GenericPack { name, .. } => {
            mark(*name, TokenKind::TypeParameter);
        }

        NodeKind::TypeAlias { keyword, .. }
        | NodeKind::Constant { keyword, .. }
        | NodeKind::Export { keyword, .. }
        | NodeKind::Declaration { keyword, .. }
        | NodeKind::Continue { keyword }
        | NodeKind::TypeOf { keyword, .. } => {
            if let Some(token) = tokens.get_mut(&tree.token(*keyword).span.start) {
                token.kind = TokenKind::Keyword;
            }
        }

        NodeKind::FunctionName { path, method, .. } => {
            for (index, component) in tree.list(path).iter().enumerate() {
                if index == 0 && tree.list(path).len() > 1 {
                    continue;
                }

                mark(
                    component.node,
                    if index + 1 == tree.list(path).len() && method.is_none() {
                        TokenKind::Function
                    } else {
                        TokenKind::Property
                    },
                );
            }

            if let Some(method) = method {
                mark(*method, TokenKind::Method);
            }
        }

        NodeKind::Class { name, .. } => mark(*name, TokenKind::Type),
        _ => {}
    }

    match &tree.node(node).kind {
        NodeKind::Function { prefix, name, .. } => {
            if let Some(prefix) = prefix
                && let Some(token) = tokens.get_mut(&tree.token(*prefix).span.start)
            {
                token.kind = TokenKind::Keyword;
            }

            if let Some(name) = name {
                let declaration = match &tree.node(*name).kind {
                    NodeKind::FunctionName { path, method, .. } => {
                        method.or_else(|| tree.list(path).last().map(|entry| entry.node))
                    }

                    NodeKind::Name { .. } => Some(*name),
                    _ => None,
                };

                if let Some(declaration) = declaration
                    && let Some(token) = tokens.get_mut(&tree.node(declaration).span.start)
                {
                    token.kind = if matches!(
                        tree.node(*name).kind,
                        NodeKind::FunctionName {
                            method: Some(_),
                            ..
                        }
                    ) {
                        TokenKind::Method
                    } else {
                        TokenKind::Function
                    };

                    token.is_declaration = true;
                }
            }
        }

        NodeKind::Class {
            name,
            keyword,
            open,
            ..
        } => {
            if let Some(token) = tokens.get_mut(&tree.node(*name).span.start) {
                token.is_declaration = true;
            }

            for keyword in [keyword, open].into_iter().flatten() {
                if let Some(token) = tokens.get_mut(&tree.token(*keyword).span.start) {
                    token.kind = TokenKind::Keyword;
                }
            }
        }

        NodeKind::TypeField {
            access: Some(access),
            ..
        }
        | NodeKind::TypeTable {
            access: Some(access),
            ..
        }
        | NodeKind::TypeIndexer {
            access: Some(access),
            ..
        } => {
            if let Some(token) = tokens.get_mut(&tree.token(*access).span.start) {
                token.kind = TokenKind::Keyword;
            }
        }

        _ => {}
    }
}

fn outline(tree: &Tree<'_>, node: NodeIndex, prefix: usize) -> Vec<Symbol> {
    let symbol = |name, kind, declaration, children| Symbol {
        name: text(tree, name),
        kind,
        range: range(tree, declaration, prefix),
        selection: range(tree, name, prefix),
        children,
    };

    let mut symbols: Vec<Symbol> = match &tree.node(node).kind {
        NodeKind::Local {
            bindings, values, ..
        }
        | NodeKind::Constant {
            bindings, values, ..
        } => tree
            .list(bindings)
            .iter()
            .enumerate()
            .filter_map(|(index, binding)| {
                let name = match tree.node(binding.node).kind {
                    NodeKind::Binding { name, .. } => name,
                    NodeKind::Name { .. } => binding.node,
                    _ => return None,
                };

                let value = tree.list(values).get(index).map(|value| value.node);

                let kind = if value.is_some_and(|value| {
                    matches!(
                        tree.node(unwrap(tree, value)).kind,
                        NodeKind::Function { .. }
                    )
                }) {
                    SymbolKind::Function
                } else if matches!(tree.node(node).kind, NodeKind::Constant { .. }) {
                    SymbolKind::Constant
                } else {
                    SymbolKind::Variable
                };

                Some(symbol(
                    name,
                    kind,
                    node,
                    value.map_or_else(Vec::new, |value| outline(tree, value, prefix)),
                ))
            })
            .collect(),

        NodeKind::Function {
            name: Some(name),
            body,
            ..
        } => vec![symbol(
            *name,
            if matches!(
                tree.node(*name).kind,
                NodeKind::FunctionName {
                    method: Some(_),
                    ..
                }
            ) {
                SymbolKind::Method
            } else {
                SymbolKind::Function
            },
            node,
            body.map_or_else(Vec::new, |body| outline(tree, body, prefix)),
        )],

        NodeKind::TypeAlias {
            name, annotation, ..
        } => vec![symbol(
            *name,
            SymbolKind::Type,
            node,
            outline(tree, *annotation, prefix),
        )],

        NodeKind::Class { name, members, .. } => vec![symbol(
            *name,
            SymbolKind::Class,
            node,
            tree.list(members)
                .iter()
                .flat_map(|member| outline(tree, member.node, prefix))
                .collect(),
        )],

        NodeKind::TableField {
            opening: None,
            key: Some(name),
            value,
            ..
        } => vec![symbol(
            *name,
            if matches!(
                tree.node(unwrap(tree, *value)).kind,
                NodeKind::Function { .. }
            ) {
                SymbolKind::Function
            } else {
                SymbolKind::Property
            },
            node,
            outline(tree, *value, prefix),
        )],

        NodeKind::TypeField {
            opening: None,
            key,
            annotation,
            ..
        } => vec![symbol(
            *key,
            SymbolKind::Property,
            node,
            outline(tree, *annotation, prefix),
        )],

        NodeKind::Property { binding, .. } => {
            if let NodeKind::Binding { name, .. } = tree.node(*binding).kind {
                vec![symbol(name, SymbolKind::Property, node, Vec::new())]
            } else {
                Vec::new()
            }
        }

        _ => tree
            .children(node)
            .into_iter()
            .flat_map(|child| outline(tree, child, prefix))
            .collect(),
    };

    symbols.retain(|symbol| !symbol.name.is_empty() && !symbol.selection.is_empty());

    symbols
}

fn is_foldable(kind: &NodeKind) -> bool {
    matches!(
        kind,
        NodeKind::Function { .. }
            | NodeKind::If { .. }
            | NodeKind::Branch { .. }
            | NodeKind::Else { .. }
            | NodeKind::While { .. }
            | NodeKind::Repeat { .. }
            | NodeKind::NumericFor { .. }
            | NodeKind::GenericFor { .. }
            | NodeKind::Do { .. }
            | NodeKind::Table { .. }
            | NodeKind::TypeTable { .. }
            | NodeKind::Class { .. }
            | NodeKind::Arguments { .. }
            | NodeKind::Parameters { .. }
            | NodeKind::TypePack { .. }
    )
}

fn import_folds(tree: &Tree<'_>, imports: &[Import], prefix: usize, folds: &mut Vec<Fold>) {
    for node in &tree.nodes {
        let NodeKind::Block { statements } = &node.kind else {
            continue;
        };

        let mut group: Option<Range<usize>> = None;
        let mut count = 0;

        for statement in tree.list(statements) {
            let span = tree.node(statement.node).span;

            let is_imported = matches!(
                tree.node(statement.node).kind,
                NodeKind::Local { .. } | NodeKind::Constant { .. }
            ) && {
                let first =
                    imports.partition_point(|import| import.range.start < span.start + prefix);

                imports[first..]
                    .iter()
                    .take_while(|import| import.range.start <= span.end + prefix)
                    .any(|import| import.range.end <= span.end + prefix)
            };

            if is_imported
                && let Some(previous) = &mut group
                && tree
                    .source
                    .get(previous.end..span.start)
                    .is_some_and(|gap| gap.iter().all(u8::is_ascii_whitespace))
            {
                previous.end = span.end;
                count += 1;
                continue;
            }

            if count > 1
                && let Some(previous) = group.take()
            {
                folds.push(Fold {
                    range: previous.start + prefix..previous.end + prefix,
                    kind: FoldKind::Imports,
                });
            }

            group = is_imported.then_some(span.start..span.end);
            count = usize::from(is_imported);
        }

        if count > 1
            && let Some(group) = group
        {
            folds.push(Fold {
                range: group.start + prefix..group.end + prefix,
                kind: FoldKind::Imports,
            });
        }
    }
}

fn comments(tree: &Tree<'_>, prefix: usize, folds: &mut Vec<Fold>) {
    let mut group: Option<Range<usize>> = None;

    for token in &tree.tokens {
        if token.kind == LexicalKind::Whitespace {
            continue;
        }

        if token.kind == LexicalKind::Comment && !token.bytes(tree.source).starts_with(b"--!") {
            if let Some(previous) = &mut group {
                let gap = &tree.source[previous.end..token.span.start];

                if gap.split(|byte| *byte == b'\n').count() == 2
                    && gap.iter().all(u8::is_ascii_whitespace)
                {
                    previous.end = token.span.end;
                    continue;
                }
            }

            finish_comment(tree, prefix, group.take(), folds);
            group = Some(token.span.start..token.span.end);
        } else {
            finish_comment(tree, prefix, group.take(), folds);

            if matches!(
                token.kind,
                LexicalKind::BlockComment | LexicalKind::MalformedComment
            ) && is_multiline(tree, token.span.start..token.span.end)
            {
                folds.push(Fold {
                    range: token.span.start + prefix..token.span.end + prefix,
                    kind: FoldKind::Comment,
                });
            }
        }
    }

    finish_comment(tree, prefix, group, folds);
}

fn finish_comment(
    tree: &Tree<'_>,
    prefix: usize,
    group: Option<Range<usize>>,
    folds: &mut Vec<Fold>,
) {
    if let Some(group) = group
        && is_multiline(tree, group.clone())
    {
        folds.push(Fold {
            range: group.start + prefix..group.end + prefix,
            kind: FoldKind::Comment,
        });
    }
}

fn documentation(source: &str, range: &Range<usize>, links: &mut Vec<Link>) {
    let Some(content) = source.get(range.start..range.end) else {
        return;
    };

    let mut offset = 0;

    while offset < content.len() {
        let remaining = &content[offset..];

        let start = [remaining.find("https://"), remaining.find("http://")]
            .into_iter()
            .flatten()
            .min();

        let Some(start) = start else { break };

        let start = offset + start;

        let end = content[start..]
            .find(|character: char| {
                character.is_whitespace() || matches!(character, '\'' | '"' | '`' | '<' | '>')
            })
            .map_or(content.len(), |length| start + length);

        let mut address = content[start..end].trim_end_matches(['.', ',', ';', ':']);

        while let Some(closing) = address.as_bytes().last().copied() {
            let opening = match closing {
                b')' => b'(',
                b']' => b'[',
                b'}' => b'{',
                _ => break,
            };

            if address.bytes().filter(|byte| *byte == closing).count()
                <= address.bytes().filter(|byte| *byte == opening).count()
            {
                break;
            }

            address = address[..address.len() - 1]
                .trim_end_matches(['.', ',', ';', ':'])
                .trim_end();
        }

        if let Ok(url) = Url::parse(address)
            && matches!(url.scheme(), "http" | "https")
            && url.host_str().is_some()
        {
            links.push(Link {
                range: range.start + start..range.start + start + address.len(),
                target: url.to_string(),
            });
        }

        offset = end;
    }
}

fn is_instance(tree: &Tree<'_>, bindings: &Bindings, node: NodeIndex) -> bool {
    let node = bindings.value(tree, node);

    match tree.node(node).kind {
        NodeKind::Name { .. } => {
            bindings.is_global(tree, node, "script") || bindings.is_global(tree, node, "game")
        }

        NodeKind::Field { receiver, .. } => is_instance(tree, bindings, receiver),

        NodeKind::Index { receiver, key, .. } => {
            is_instance(tree, bindings, receiver)
                && matches!(tree.node(key).kind, NodeKind::String { .. })
                && literal::decode(tree.text(key)).is_some()
        }

        NodeKind::MethodCall {
            receiver,
            method,
            arguments,
            ..
        } => {
            is_instance(tree, bindings, receiver)
                && matches!(
                    tree.text(method),
                    b"GetService" | b"WaitForChild" | b"FindFirstChild"
                )
                && matches!(&tree.node(arguments).kind, NodeKind::Arguments { values, .. }
                    if matches!(tree.list(values), [argument] if matches!(tree.node(argument.node).kind, NodeKind::String { .. }) && literal::decode(tree.text(argument.node)).is_some()))
        }

        _ => false,
    }
}

fn is_multiline(tree: &Tree<'_>, range: Range<usize>) -> bool {
    tree.source
        .get(range)
        .is_some_and(|bytes| bytes.contains(&b'\n'))
}

fn range(tree: &Tree<'_>, node: NodeIndex, prefix: usize) -> Range<usize> {
    let span = tree.node(node).span;

    span.start + prefix..span.end + prefix
}
