use std::{
    borrow::Cow,
    collections::{BTreeMap, BTreeSet},
    num::NonZeroUsize,
    str,
};

use instar_bridge::{
    HoverKind, Snapshot, Subject,
    types::{
        Application, ApplicationState, Function, Generic, Named, Pack, PackIndex, Primitive,
        Provenance, Singleton, Table, Type, TypeIndex, Unresolved,
    },
};

use unicode_width::UnicodeWidthStr;

use super::{
    spelling::{identifier, property_name, qualified, quoted},
    surfaces::Section,
};

use crate::{
    format::{
        self, Error, Prepared,
        fragment::{self, Grammar},
    },
    settings::format::{Configuration, Indentation},
};

const DETAIL_BYTES: usize = 32 * 1024;
const DETAIL_LINES: usize = 256;

#[derive(Clone)]
pub(super) struct Rendered {
    pub(super) text: String,
    pub(super) grammar: Option<Grammar>,
    pub(super) is_declaration: bool,
    is_compound: bool,
    pub(super) is_abbreviated: bool,
}

pub(super) struct Renderer<'snapshot> {
    snapshot: &'snapshot Snapshot,
    is_compact: bool,
    width: usize,
    configuration: Prepared,
    active: Vec<Subject>,
    names: BTreeMap<(bool, usize), String>,
    visits: usize,
    tables: usize,
}

impl Rendered {
    fn source(text: impl Into<String>, grammar: Grammar) -> Self {
        Self {
            text: text.into(),
            grammar: Some(grammar),
            is_declaration: matches!(grammar, Grammar::Declaration),
            is_compound: false,
            is_abbreviated: false,
        }
    }

    pub(super) fn display(text: impl Into<String>, is_abbreviated: bool) -> Self {
        Self {
            text: text.into(),
            grammar: None,
            is_declaration: false,
            is_compound: false,
            is_abbreviated,
        }
    }
}

impl<'snapshot> Renderer<'snapshot> {
    pub(super) fn new(
        snapshot: &'snapshot Snapshot,
        is_compact: bool,
        width: usize,
    ) -> Result<Self, Error> {
        let configuration = Configuration {
            width: NonZeroUsize::new(if is_compact { usize::MAX } else { 80 })
                .ok_or_else(|| Error::internal(0..0, "Presentation width is zero."))?,
            indent_style: Indentation::Spaces,
            should_emit_final_newline: false,
            ..Configuration::default()
        };

        let configuration = Prepared::new(&configuration).map_err(|error| {
            Error::internal(
                0..0,
                format!("Cannot prepare presentation formatting. {error}"),
            )
        })?;

        Ok(Self {
            snapshot,
            is_compact,
            width,
            configuration,
            active: Vec::new(),
            names: generic_names(snapshot)?,
            visits: 0,
            tables: 0,
        })
    }

    pub(super) fn render(&mut self, subject: Subject, depth: usize) -> Result<Rendered, Error> {
        if self.active.contains(&subject) {
            if let Subject::Type(index) = subject
                && let Some(Type::Named(named)) = self.snapshot.types.get(index)
                && matches!(named.provenance, Provenance::Alias | Provenance::External)
            {
                let name = self.spelling(named)?;

                if named.type_arguments.is_empty() && named.pack_arguments.is_empty() {
                    return Ok(Rendered::source(name, Grammar::Annotation));
                }

                let arguments = named
                    .type_arguments
                    .iter()
                    .map(|_| "…")
                    .chain(named.pack_arguments.iter().map(|_| "(…)"))
                    .collect::<Vec<_>>()
                    .join(", ");

                return Ok(Rendered::display(format!("{name}<{arguments}>"), true));
            }

            return Ok(Rendered::display("…", true));
        }

        self.visits += 1;

        if depth >= 64 || self.visits > 16_384 {
            return Ok(Rendered::display("<omitted>", true));
        }

        self.active.push(subject);

        let result = match subject {
            Subject::Type(index) => {
                let node = self.snapshot.types.get(index).ok_or_else(|| {
                    Error::internal(0..0, "Presentation type reference is out of bounds.")
                })?;

                self.render_type(node, depth)
            }

            Subject::Pack(index) => {
                let node = self.snapshot.packs.get(index).ok_or_else(|| {
                    Error::internal(0..0, "Presentation pack reference is out of bounds.")
                })?;

                self.render_pack(node, depth)
            }
        };

        self.active.pop();
        let result = result?;

        if !self.is_compact && result.text.len() > DETAIL_BYTES - 512 {
            return Ok(Rendered::display("<omitted>", true));
        }

        Ok(result)
    }

    pub(super) fn combine(
        &self,
        grammar: Grammar,
        is_source: bool,
        children: &[Rendered],
        build: impl FnOnce(&[Rendered]) -> String,
    ) -> Result<Rendered, Error> {
        let is_source = is_source && children.iter().all(|child| child.grammar.is_some());
        let mut children = Cow::Borrowed(children);

        if !is_source {
            for child in children
                .to_mut()
                .iter_mut()
                .filter(|child| child.grammar.is_some())
            {
                child.text = self.validate(child)?;
            }
        }

        let text = build(&children);

        Ok(Rendered {
            text,
            grammar: is_source.then_some(grammar),
            is_declaration: matches!(grammar, Grammar::Declaration)
                || children.iter().any(|child| child.is_declaration),
            is_compound: false,
            is_abbreviated: children.iter().any(|child| child.is_abbreviated),
        })
    }

    pub(super) fn validate(&self, rendered: &Rendered) -> Result<String, Error> {
        let grammar = rendered.grammar.ok_or_else(|| {
            Error::internal(
                0..0,
                "Display-only presentation cannot be formatted as source.",
            )
        })?;

        fragment::format(&rendered.text, grammar, &self.configuration).map_err(|error| {
            Error::internal(
                error.range,
                format!("Cannot format generated presentation. {}", error.message),
            )
        })
    }

    pub(super) fn section(&self, rendered: &Rendered) -> Result<Section, Error> {
        let content = if rendered.grammar.is_some() {
            self.validate(rendered)?
        } else {
            rendered.text.clone()
        };

        Ok(Section { content })
    }

    fn generic(&self, generic: &Generic, is_pack: bool) -> Result<String, Error> {
        self.names
            .get(&(is_pack, generic.identity))
            .cloned()
            .ok_or_else(|| Error::internal(0..0, "Presentation generic identity has no spelling."))
    }

    fn render_type(&mut self, node: &Type, depth: usize) -> Result<Rendered, Error> {
        match node {
            Type::Primitive(Primitive::Function) => Ok(Rendered::display("function", false)),

            Type::Primitive(primitive) => Ok(Rendered::source(
                primitive_name(*primitive),
                Grammar::Annotation,
            )),

            Type::Singleton(Singleton::Boolean(is_true)) => {
                Ok(Rendered::source(is_true.to_string(), Grammar::Annotation))
            }

            Type::Singleton(Singleton::String(bytes)) => {
                let text = quoted(bytes)?;

                if self.is_compact && columns(&text) > self.width {
                    Ok(Rendered::display("<string singleton>", true))
                } else {
                    Ok(Rendered::source(text, Grammar::Annotation))
                }
            }

            Type::Generic(generic) => Ok(Rendered::source(
                self.generic(generic, false)?,
                Grammar::Annotation,
            )),

            Type::Named(named) => self.named(named, depth),
            Type::Function(function) => self.function(function, depth, None),

            Type::Table(table) => {
                self.tables += 1;
                let result = self.table(table, depth);
                self.tables -= 1;

                result
            }

            Type::Union(members) => self.members(members, " | ", depth),
            Type::Intersection(members) => self.members(members, " & ", depth),

            Type::Metatable { table, metatable } => {
                let children = self.arguments("setmetatable", &[*table, *metatable], &[], depth)?;
                let is_source = children.iter().all(|child| !child.is_declaration);

                self.combine(Grammar::Annotation, is_source, &children, |children| {
                    applied("setmetatable", children)
                })
            }

            Type::Negation(index) => {
                let child = self.render(Subject::Type(*index), depth + 1)?;

                self.combine(Grammar::Annotation, false, &[child], |children| {
                    format!("~({})", children[0].text)
                })
            }

            Type::Application(application) => self.application(application, false, depth),
            Type::Reference(index) => self.render(Subject::Type(*index), depth + 1),
            Type::Unresolved(state) => Ok(Self::unresolved(*state, false)),
            Type::Omitted => Ok(Rendered::display("<omitted>", true)),
        }
    }

    fn render_pack(&mut self, node: &Pack, depth: usize) -> Result<Rendered, Error> {
        match node {
            Pack::Sequence { head, tail } => {
                let mut elements = head.clone();

                let mut tail = match tail {
                    Some(tail) => {
                        let (following, tail) = self.sequence(*tail)?;
                        elements.extend(following);

                        tail
                    }

                    None => None,
                };

                if let Some(index) = tail
                    && self.is_hidden(index)?
                {
                    tail = None;
                }

                let mut children = Vec::with_capacity(elements.len() + usize::from(tail.is_some()));

                for index in elements {
                    children.push(self.render(Subject::Type(index), depth + 1)?);
                }

                if let Some(index) = tail {
                    children.push(self.render(Subject::Pack(index), depth + 1)?);
                }

                let text = format!("({})", joined(&children));

                let is_abbreviated =
                    self.is_compact && columns(&text) > self.width && children.len() > 1;

                let is_source =
                    !is_abbreviated && children.iter().all(|child| !child.is_declaration);

                let mut rendered =
                    self.combine(Grammar::Argument, is_source, &children, |children| {
                        if is_abbreviated {
                            match tail {
                                Some(_) => format!(
                                    "(…, {})",
                                    children
                                        .last()
                                        .expect("Internal error: missing pack tail.")
                                        .text
                                ),

                                None => "(…)".to_owned(),
                            }
                        } else {
                            format!("({})", joined(children))
                        }
                    })?;

                rendered.is_abbreviated |= is_abbreviated;

                Ok(rendered)
            }

            Pack::Variadic { element, .. } => {
                let child = self.render(Subject::Type(*element), depth + 1)?;
                let is_source = !child.is_declaration;

                self.combine(Grammar::Argument, is_source, &[child], |children| {
                    format!("...{}", parenthesized(&children[0]))
                })
            }

            Pack::Generic(generic) => Ok(Rendered::source(
                format!("{}...", self.generic(generic, true)?),
                Grammar::Argument,
            )),

            Pack::Application(application) => self.application(application, true, depth),
            Pack::Reference(index) => self.render(Subject::Pack(*index), depth + 1),
            Pack::Unresolved(state) => Ok(Self::unresolved(*state, true)),
            Pack::Omitted => Ok(Rendered::display("<omitted>", true)),
        }
    }

    pub(super) fn returns(&mut self, index: PackIndex, depth: usize) -> Result<Rendered, Error> {
        let (head, tail) = self.sequence(index)?;

        let has_visible_tail = match tail {
            Some(index) => !self.is_hidden(index)?,
            None => false,
        };

        if head.len() == 1 && !has_visible_tail {
            self.render(Subject::Type(head[0]), depth)
        } else {
            self.render(Subject::Pack(index), depth)
        }
    }

    fn named(&mut self, named: &Named, depth: usize) -> Result<Rendered, Error> {
        let is_verified = matches!(named.provenance, Provenance::Alias | Provenance::External);

        let name = self.spelling(named)?;

        let children =
            self.arguments(&name, &named.type_arguments, &named.pack_arguments, depth)?;

        let is_source = (is_verified
            || (identifier(&named.name).is_ok()
                && named
                    .qualifier
                    .as_deref()
                    .is_none_or(|name| identifier(name).is_ok())))
            && children.iter().all(|child| !child.is_declaration);

        self.combine(Grammar::Annotation, is_source, &children, |children| {
            applied(&name, children)
        })
    }

    fn spelling(&self, named: &Named) -> Result<String, Error> {
        if named.provenance == Provenance::External
            && named.qualifier.is_none()
            && identifier(&named.name).is_err()
        {
            fragment::nominal(&named.name)?;

            self.validate(&Rendered::source(&named.name, Grammar::Annotation))
        } else {
            qualified(
                &named.name,
                named.qualifier.as_deref(),
                matches!(named.provenance, Provenance::Alias | Provenance::External),
            )
        }
    }

    fn application(
        &mut self,
        application: &Application,
        is_pack: bool,
        depth: usize,
    ) -> Result<Rendered, Error> {
        let name = qualified(&application.name, application.qualifier.as_deref(), true)?;

        let children = self.arguments(
            &name,
            &application.type_arguments,
            &application.pack_arguments,
            depth,
        )?;

        let state = match application.state {
            ApplicationState::Pending => "pending",
            ApplicationState::Unreduced => "unreduced",
        };

        self.combine(Grammar::Argument, false, &children, |children| {
            format!(
                "{}{} <{state}>",
                applied(&name, children),
                if is_pack { "..." } else { "" }
            )
        })
    }

    fn arguments(
        &mut self,
        name: &str,
        types: &[TypeIndex],
        packs: &[PackIndex],
        depth: usize,
    ) -> Result<Vec<Rendered>, Error> {
        let mut children = Vec::with_capacity(types.len() + packs.len());
        let width = self.width;

        if self.is_compact && (!types.is_empty() || !packs.is_empty()) {
            let count = types.len() + packs.len();

            self.width = (self
                .width
                .saturating_sub(columns(name) + 2 + (count - 1) * 2)
                / count)
                .max(1);
        }

        for index in types {
            children.push(self.render(Subject::Type(*index), depth + 1)?);
        }

        for index in packs {
            children.push(self.render(Subject::Pack(*index), depth + 1)?);
        }

        self.width = width;

        Ok(children)
    }

    pub(super) fn binders(&mut self, function: &Function) -> Result<(String, bool), Error> {
        let mut names = Vec::with_capacity(function.generics.len() + function.generic_packs.len());
        let mut is_source = true;

        for index in &function.generics {
            let rendered = self.render(Subject::Type(*index), 0)?;
            is_source &= rendered.grammar.is_some();
            names.push(rendered.text);
        }

        for index in &function.generic_packs {
            let rendered = self.render(Subject::Pack(*index), 0)?;
            is_source &= rendered.grammar.is_some();

            names.push(if rendered.grammar.is_some() {
                rendered.text
            } else {
                format!("{}...", rendered.text)
            });
        }

        let text = if names.is_empty() {
            String::new()
        } else {
            format!("<{}>", names.join(", "))
        };

        Ok((text, is_source))
    }

    fn function(
        &mut self,
        function: &Function,
        depth: usize,
        declaration: Option<&str>,
    ) -> Result<Rendered, Error> {
        let mut prefixes = Vec::new();
        let mut children = Vec::new();
        let (head, tail) = self.sequence(function.arguments)?;

        for (position, index) in head.into_iter().enumerate() {
            let child = self.render(Subject::Type(index), depth + 1)?;

            let prefix = match function
                .argument_names
                .get(position)
                .and_then(Option::as_deref)
            {
                Some(name) => format!("{}: ", identifier(name)?),
                None => String::new(),
            };

            prefixes.push(prefix);
            children.push(child);
        }

        if let Some(index) = tail
            && !self.is_hidden(index)?
        {
            let child = self.render(Subject::Pack(index), depth + 1)?;
            prefixes.push(String::new());
            children.push(child);
        }

        let returns = self.returns(function.returns, depth + 1)?;
        let (binders, has_complete_binders) = self.binders(function)?;

        let attribute = if function.checked { "@checked " } else { "" };

        let header = format!("{attribute}{}{binders}", declaration.unwrap_or_default());
        children.push(returns);

        let separator = if declaration.is_some() { ": " } else { " -> " };

        let build = |children: &[Rendered]| function_text(&header, &prefixes, children, separator);

        if self.is_compact && columns(&build(&children)) > self.width {
            let mut rendered = Rendered::display("(…) -> …", true);
            rendered.is_compound = true;

            return Ok(rendered);
        }

        let is_source = declaration.is_none()
            && has_complete_binders
            && children.iter().all(|child| !child.is_declaration);

        let mut rendered = self.combine(
            if function.checked {
                Grammar::Declaration
            } else {
                Grammar::Annotation
            },
            is_source,
            &children,
            build,
        )?;

        rendered.is_compound = true;
        rendered.is_abbreviated |= !has_complete_binders;

        Ok(rendered)
    }

    fn sequence(&self, mut index: PackIndex) -> Result<(Vec<TypeIndex>, Option<PackIndex>), Error> {
        let mut seen = BTreeSet::new();
        let mut head = Vec::new();

        while seen.insert(index) {
            match self.snapshot.packs.get(index) {
                Some(Pack::Sequence {
                    head: elements,
                    tail,
                }) => {
                    head.extend(elements);

                    match tail {
                        Some(tail) => index = *tail,
                        None => return Ok((head, None)),
                    }
                }

                Some(Pack::Reference(reference)) => index = *reference,
                Some(_) => return Ok((head, Some(index))),

                None => {
                    return Err(Error::internal(
                        0..0,
                        "Function pack reference is out of bounds.",
                    ));
                }
            }
        }

        Ok((head, Some(index)))
    }

    fn is_hidden(&self, mut index: PackIndex) -> Result<bool, Error> {
        let mut seen = BTreeSet::new();

        while seen.insert(index) {
            match self.snapshot.packs.get(index) {
                Some(Pack::Variadic {
                    hidden: is_hidden, ..
                }) => return Ok(*is_hidden),

                Some(Pack::Reference(reference)) => index = *reference,
                Some(_) => return Ok(false),

                None => {
                    return Err(Error::internal(
                        0..0,
                        "Variadic pack reference is out of bounds.",
                    ));
                }
            }
        }

        Ok(false)
    }

    fn table(&mut self, table: &Table, depth: usize) -> Result<Rendered, Error> {
        if self.is_compact && (table.properties.len() > 2 || self.tables > 2) {
            return Ok(Rendered::display("{ … }", true));
        }

        let mut children = Vec::new();
        let mut is_shared = true;

        for property in &table.properties {
            let (name, is_named) = property_name(&property.name)?;

            match (property.read, property.write) {
                (Some(read), Some(write)) if property.shared && read == write => {
                    let child = self.render(Subject::Type(read), depth + 1)?;
                    let is_source = is_named || !child.is_declaration;

                    children.push(self.combine(
                        Grammar::Field,
                        is_source,
                        &[child],
                        |children| format!("{name}: {}", children[0].text),
                    )?);
                }

                (read, write) => {
                    is_shared &= read.is_none() || write.is_none();

                    for (access, index) in [("read", read), ("write", write)] {
                        if let Some(index) = index {
                            let child = self.render(Subject::Type(index), depth + 1)?;
                            let is_source = is_named || !child.is_declaration;

                            children.push(self.combine(
                                Grammar::Field,
                                is_source,
                                &[child],
                                |children| format!("{access} {name}: {}", children[0].text),
                            )?);
                        }
                    }
                }
            }
        }

        if let Some(indexer) = &table.indexer {
            let key = self.render(Subject::Type(indexer.key), depth + 1)?;
            let value = self.render(Subject::Type(indexer.value), depth + 1)?;

            let access = if indexer.readonly { "read " } else { "" };

            let is_source = !key.is_declaration && !value.is_declaration;

            children.push(
                self.combine(Grammar::Field, is_source, &[key, value], |children| {
                    format!("{access}[{}]: {}", children[0].text, children[1].text)
                })?,
            );
        }

        let build = |children: &[Rendered]| {
            if children.is_empty() {
                "{}".to_owned()
            } else {
                format!("{{ {} }}", joined(children))
            }
        };

        let text = build(&children);

        if self.is_compact
            && (columns(&text) > self.width || children.iter().any(|child| child.is_abbreviated))
        {
            return Ok(Rendered::display("{ … }", true));
        }

        let grammar = if children.iter().any(|child| child.is_declaration) {
            Grammar::Declaration
        } else {
            Grammar::Annotation
        };

        self.combine(grammar, is_shared, &children, build)
    }

    fn members(
        &mut self,
        members: &[TypeIndex],
        separator: &str,
        depth: usize,
    ) -> Result<Rendered, Error> {
        if members.is_empty() {
            return Err(Error::internal(
                0..0,
                "Presentation union or intersection is empty.",
            ));
        }

        let count = if self.is_compact {
            members.len().min(2)
        } else {
            members.len()
        };

        let mut children = Vec::with_capacity(count);

        for index in members.iter().take(count) {
            children.push(self.render(Subject::Type(*index), depth + 1)?);
        }

        let mut is_omitted = count < members.len();

        let mut text = children
            .iter()
            .map(parenthesized)
            .collect::<Vec<_>>()
            .join(separator);

        if is_omitted {
            text.push_str(separator);
            text.push('…');
        }

        if self.is_compact {
            while columns(&text) > self.width && children.len() > 1 {
                children.pop();
                is_omitted = true;
                text = format!("{}{separator}…", parenthesized(&children[0]));
            }

            if columns(&text) > self.width && is_omitted {
                return Ok(Rendered::display("…", true));
            }
        }

        let is_source = !is_omitted && children.iter().all(|child| !child.is_declaration);

        let mut result = self.combine(Grammar::Annotation, is_source, &children, |children| {
            let mut text = children
                .iter()
                .map(parenthesized)
                .collect::<Vec<_>>()
                .join(separator);

            if is_omitted {
                text.push_str(separator);
                text.push('…');
            }

            text
        })?;

        result.is_abbreviated |= is_omitted;
        result.is_compound = true;

        Ok(result)
    }

    fn unresolved(state: Unresolved, is_pack: bool) -> Rendered {
        let state = match state {
            Unresolved::Free => "unresolved",
            Unresolved::Blocked => "blocked",
            Unresolved::Lazy => "lazy",
            Unresolved::Error => "error",
            Unresolved::InternalRefinement => "internal refinement",
        };

        let text = format!("<{state}{}>", if is_pack { " pack" } else { "" });

        Rendered::display(text, false)
    }

    fn declaration(
        &self,
        rendered: Rendered,
        symbol: &[u8],
        is_pack: bool,
    ) -> Result<Rendered, Error> {
        if symbol.is_empty() {
            return Ok(rendered);
        }

        let (name, is_named) = property_name(symbol)?;
        let is_source = !is_pack && (is_named || !rendered.is_declaration);

        self.combine(Grammar::Field, is_source, &[rendered], |children| {
            format!("{name}: {}", children[0].text)
        })
    }

    fn statement(
        &self,
        prefix: &str,
        rendered: &Rendered,
        is_source: bool,
    ) -> Result<Section, Error> {
        let mut section = self.section(rendered)?;
        section.content = format!("{prefix}{}", section.content);

        if is_source && rendered.grammar.is_some() && !rendered.is_declaration {
            section.content = format::format(&section.content, &self.configuration)?;
        }

        Ok(section)
    }

    fn alias(&mut self, symbol: &[u8]) -> Result<Section, Error> {
        let root = self.snapshot.root;

        if let Subject::Type(index) = root
            && let Some(Type::Named(named)) = self.snapshot.types.get(index)
        {
            let header = self.render(root, 0)?;

            let body = match named.expansion {
                Some(index) => self.render(Subject::Type(index), 0)?,
                None => Rendered::display("<omitted>", true),
            };

            let prefix = format!("type {} = ", header.text);
            let is_source = header.grammar.is_some() && named.qualifier.is_none();

            self.statement(&prefix, &body, is_source)
        } else {
            let name = str::from_utf8(symbol).map_err(|error| {
                Error::internal(0..0, format!("Alias name is not UTF-8. {error}"))
            })?;

            let prefix = format!("type {} = ", identifier(name)?);
            let body = self.render(root, 0)?;

            self.statement(&prefix, &body, true)
        }
    }

    pub(super) fn finish(
        mut self,
        symbol: &[u8],
        kind: Option<HoverKind>,
    ) -> Result<Vec<Section>, Error> {
        let primary = if kind == Some(HoverKind::Alias)
            || (kind == Some(HoverKind::Type)
                && matches!(
                    self.snapshot.root,
                    Subject::Type(index) if matches!(self.snapshot.types.get(index), Some(Type::Named(_)))
                )) {
            self.alias(symbol)?
        } else {
            let rendered = self.render(self.snapshot.root, 0)?;
            let is_pack = matches!(self.snapshot.root, Subject::Pack(_));

            match kind {
                Some(HoverKind::Local | HoverKind::Constant) => {
                    let name = str::from_utf8(symbol).map_err(|error| {
                        Error::internal(0..0, format!("Binding name is not UTF-8. {error}"))
                    })?;

                    let keyword = if kind == Some(HoverKind::Constant) {
                        "const"
                    } else {
                        "local"
                    };

                    let prefix = format!("{keyword} {}: ", identifier(name)?);

                    self.statement(
                        &prefix,
                        &rendered,
                        !is_pack && kind == Some(HoverKind::Local),
                    )?
                }

                Some(
                    HoverKind::LocalFunction | HoverKind::ConstantFunction | HoverKind::Function,
                ) => {
                    let name = str::from_utf8(symbol).map_err(|error| {
                        Error::internal(0..0, format!("Function name is not UTF-8. {error}"))
                    })?;

                    let prefix = if kind == Some(HoverKind::LocalFunction) {
                        "local function"
                    } else if kind == Some(HoverKind::ConstantFunction) {
                        "const function"
                    } else {
                        "function"
                    };

                    let header = format!("{prefix} {}", identifier(name)?);

                    let rendered = if let Some(function) = root_function(self.snapshot)? {
                        self.function(function, 0, Some(&header))?
                    } else {
                        self.declaration(rendered, symbol, is_pack)?
                    };

                    self.section(&rendered)?
                }

                Some(HoverKind::Type) => self.section(&rendered)?,

                Some(HoverKind::Field) => {
                    let (name, is_named) = property_name(symbol)?;

                    if is_named {
                        self.statement(&format!("local {name}: "), &rendered, !is_pack)?
                    } else {
                        let rendered = self.declaration(rendered, symbol, is_pack)?;

                        self.section(&rendered)?
                    }
                }

                Some(HoverKind::Global | HoverKind::Parameter | HoverKind::Loop) | None => {
                    let rendered = self.declaration(rendered, symbol, is_pack)?;

                    self.section(&rendered)?
                }

                _ => return Err(Error::internal(0..0, "Unknown hover subject kind.")),
            }
        };

        Ok(bounded(primary))
    }
}

pub(super) fn root_function(snapshot: &Snapshot) -> Result<Option<&Function>, Error> {
    let Subject::Type(mut index) = snapshot.root else {
        return Err(Error::internal(0..0, "Signature root is a type pack."));
    };

    let mut seen = BTreeSet::new();

    while seen.insert(index) {
        match snapshot.types.get(index) {
            Some(Type::Function(function)) => return Ok(Some(function)),
            Some(Type::Omitted) => return Ok(None),
            Some(Type::Reference(reference)) => index = *reference,

            Some(Type::Named(Named {
                expansion: Some(expansion),
                ..
            })) => index = *expansion,

            _ => break,
        }
    }

    Err(Error::internal(0..0, "Signature root is not a function."))
}

fn generic_names(snapshot: &Snapshot) -> Result<BTreeMap<(bool, usize), String>, Error> {
    let generics = snapshot
        .types
        .iter()
        .filter_map(|node| match node {
            Type::Generic(generic) => Some((false, generic)),
            _ => None,
        })
        .chain(snapshot.packs.iter().filter_map(|node| match node {
            Pack::Generic(generic) => Some((true, generic)),
            _ => None,
        }));

    let generics: Vec<_> = generics.collect();
    let mut reserved = BTreeSet::new();

    for (_, generic) in &generics {
        if let Some(name) = &generic.name {
            reserved.insert(identifier(name)?.to_owned());
        }
    }

    for node in &snapshot.types {
        let name = match node {
            Type::Named(named) => Some((&named.name, named.qualifier.as_ref())),

            Type::Application(application) => {
                Some((&application.name, application.qualifier.as_ref()))
            }

            _ => None,
        };

        if let Some((name, qualifier)) = name {
            reserved.insert(name.clone());

            if let Some(qualifier) = qualifier {
                reserved.insert(qualifier.clone());
            }
        }
    }

    for node in &snapshot.packs {
        if let Pack::Application(application) = node {
            reserved.insert(application.name.clone());

            if let Some(qualifier) = &application.qualifier {
                reserved.insert(qualifier.clone());
            }
        }
    }

    let mut names = BTreeMap::new();
    let mut next = 1;

    for (is_pack, generic) in generics {
        if names.contains_key(&(is_pack, generic.identity)) {
            continue;
        }

        let name = match &generic.name {
            Some(name) => name.clone(),

            None => loop {
                let candidate = format!("{}{next}", if is_pack { "P" } else { "T" });
                next += 1;

                if reserved.insert(candidate.clone()) {
                    break candidate;
                }
            },
        };

        names.insert((is_pack, generic.identity), name);
    }

    Ok(names)
}

fn primitive_name(primitive: Primitive) -> &'static str {
    match primitive {
        Primitive::Nil => "nil",
        Primitive::Boolean => "boolean",
        Primitive::Number => "number",
        Primitive::Integer => "integer",
        Primitive::String => "string",
        Primitive::Thread => "thread",
        Primitive::Function => "function",
        Primitive::Table => "table",
        Primitive::Buffer => "buffer",
        Primitive::Any => "any",
        Primitive::Unknown => "unknown",
        Primitive::Never => "never",
    }
}

fn parenthesized(rendered: &Rendered) -> String {
    if rendered.is_compound {
        format!("({})", rendered.text)
    } else {
        rendered.text.clone()
    }
}

fn columns(text: &str) -> usize {
    UnicodeWidthStr::width(text)
}

fn joined(children: &[Rendered]) -> String {
    children
        .iter()
        .map(|child| child.text.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

fn applied(name: &str, children: &[Rendered]) -> String {
    if children.is_empty() {
        name.to_owned()
    } else {
        format!("{name}<{}>", joined(children))
    }
}

pub(super) fn function_text(
    header: &str,
    prefixes: &[String],
    children: &[Rendered],
    separator: &str,
) -> String {
    let parameters = prefixes
        .iter()
        .zip(children)
        .map(|(prefix, child)| format!("{prefix}{}", child.text))
        .collect::<Vec<_>>()
        .join(", ");

    format!(
        "{header}({parameters}){separator}{}",
        children[prefixes.len()].text
    )
}

pub(super) fn bounded(mut section: Section) -> Vec<Section> {
    if section.content.len() + 4 > DETAIL_BYTES - 16
        || section.content.lines().count().max(1) + 2 > DETAIL_LINES - 3
    {
        "<omitted>".clone_into(&mut section.content);
    }

    vec![section]
}
