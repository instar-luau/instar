mod blocks;
mod chains;
mod comments;
mod conditional;
mod expressions;
mod rewrite;
mod sorting;
mod types;

use std::{io, ops::Range};

use vermis::{Children, Kind, Parts, TokenKind, Tree, View};

use super::Options;
use super::document::Document;

use crate::project::configuration::format::{
    CallStyle, Collapse, Expansion, Parentheses, Semicolons, Separation,
};

pub(super) fn emit<'tree, 'source>(
    source: &'source str,
    tree: &'tree Tree<'source>,
    options: &'tree Options,
    held: &'tree [Range<usize>],
) -> io::Result<Document<'source>> {
    DocumentBuilder::new(source, tree, options, held).root()
}

pub(super) fn node<'tree, 'source>(
    source: &'source str,
    tree: &'tree Tree<'source>,
    options: &'tree Options,
    held: &'tree [Range<usize>],
    view: View<'tree, 'source>,
) -> io::Result<Document<'source>> {
    DocumentBuilder::new(source, tree, options, held).node(view)
}

pub(super) use rewrite::prepare;

pub(super) struct DocumentBuilder<'tree, 'source> {
    source: &'source str,
    tree: &'tree Tree<'source>,
    options: &'tree Options,
    held: &'tree [Range<usize>],
}

impl<'tree, 'source> DocumentBuilder<'tree, 'source> {
    pub(super) fn new(
        source: &'source str,
        tree: &'tree Tree<'source>,
        options: &'tree Options,
        held: &'tree [Range<usize>],
    ) -> Self {
        Self {
            source,
            tree,
            options,
            held,
        }
    }

    pub(super) fn root(&self) -> io::Result<Document<'source>> {
        let root = self
            .tree
            .root_view()
            .ok_or_else(|| io::Error::other("missing syntax root"))?;

        let Parts::Root { block } = root
            .parts()
            .ok_or_else(|| io::Error::other("invalid syntax root"))?
        else {
            return Err(io::Error::other("invalid syntax root"));
        };

        self.block(block, 0, self.source.len())
    }

    fn text(&self, view: View<'_, '_>) -> &'source str {
        &self.source[view.span().start..view.span().end]
    }

    fn optional(&self, view: Option<View<'tree, 'source>>) -> io::Result<Document<'source>> {
        view.map_or_else(|| Ok(Document::text("")), |view| self.node(view))
    }

    fn list(&self, children: Children<'tree, 'source>) -> io::Result<Document<'source>> {
        Ok(Document::join(
            &Document::sequence([Document::text(","), Document::Line]),
            children
                .map(|child| self.node(child))
                .collect::<io::Result<Vec<_>>>()?,
        ))
    }

    fn delimited(
        &self,
        opening: &'static str,
        closing: &'static str,
        children: Children<'tree, 'source>,
        spaces: bool,
        table: bool,
        forced: bool,
    ) -> io::Result<Document<'source>> {
        if children.clone().next().is_none() {
            return Ok(Document::sequence([
                Document::text(opening),
                Document::text(closing),
            ]));
        }

        let edge = if forced {
            Document::Hard
        } else if spaces {
            Document::Line
        } else {
            Document::Soft
        };

        let comma = if table && self.options.trailing_separator {
            Document::Choice(Box::new(Document::text("")), Box::new(Document::text(",")))
        } else {
            Document::text("")
        };

        Ok(Document::sequence([
            Document::text(opening),
            Document::sequence([edge.clone(), self.list(children)?, comma]).indent(),
            edge,
            Document::text(closing),
        ])
        .group())
    }

    fn block(
        &self,
        view: View<'tree, 'source>,
        start: usize,
        end: usize,
    ) -> io::Result<Document<'source>> {
        let mut parts = Vec::new();
        let mut cursor = start;

        let mut statements = view.children().peekable();

        while let Some(statement) = statements.next() {
            let gap = &self.source[cursor..statement.span().start];

            if self
                .held
                .iter()
                .any(|range| range.start <= cursor && range.end >= statement.span().start)
            {
                parts.push(Document::text(gap));
                parts.push(self.node(statement)?);
                cursor = statement.span().end;
                continue;
            }

            parts.extend(self.trivia(cursor, statement.span().start, !parts.is_empty(), true));

            parts.push(self.node(statement)?);
            cursor = statement.span().end;

            if self.options.semicolons == Semicolons::Always
                || statements
                    .peek()
                    .is_some_and(|next| self.text(*next).starts_with('('))
            {
                parts.push(Document::text(";"));
            }
        }

        parts.extend(self.trivia(cursor, end, !parts.is_empty(), false));

        Ok(Document::sequence(parts))
    }

    fn gap(&self, start: usize, end: usize) -> &'source str {
        let tokens: Vec<_> = self
            .tree
            .tokens
            .iter()
            .filter(|token| {
                token.span.start >= start
                    && token.span.end <= end
                    && matches!(token.kind, TokenKind::Comment | TokenKind::BlockComment)
            })
            .collect();

        tokens
            .first()
            .zip(tokens.last())
            .map_or("", |(first, last)| {
                &self.source[first.span.start..last.span.end]
            })
    }

    fn parameters(&self, parameters: View<'tree, 'source>) -> io::Result<Document<'source>> {
        if self.has_own_comments(parameters) {
            return self.commented(parameters);
        }

        let options = &self.options.functions.parameters;

        self.parenthesized(parameters.children(), options.expand, options.indentation)
    }

    fn arguments(&self, values: Children<'tree, 'source>) -> io::Result<Document<'source>> {
        let options = &self.options.calls;
        let last = values.clone().next_back();

        let hanging = last.is_some_and(|value| {
            matches!(value.kind(), Kind::Function | Kind::Table)
                || value.kind() == Kind::String && self.text(value).contains('\n')
        });

        if (values.clone().count() == 1
            && (hanging || last.is_some_and(|value| value.kind() == Kind::String)))
            || (hanging
                && options.style == CallStyle::HugLast
                && options.expand != Expansion::Always)
        {
            return Ok(Document::sequence([
                Document::text("("),
                Document::join(
                    &Document::text(", "),
                    values
                        .map(|value| self.node(value))
                        .collect::<io::Result<Vec<_>>>()?,
                ),
                Document::text(")"),
            ]));
        }

        self.parenthesized(values, options.expand, options.indentation)
    }

    fn parenthesized(
        &self,
        children: Children<'tree, 'source>,
        expand: Expansion,
        indentation: usize,
    ) -> io::Result<Document<'source>> {
        if children.clone().next().is_none() {
            return Ok(Document::text("()"));
        }

        let edge = if expand == Expansion::Always {
            Document::Hard
        } else if self.options.spacing.parentheses {
            Document::Line
        } else {
            Document::Soft
        };

        let mut content = Document::sequence([edge.clone(), self.list(children)?]);

        if expand != Expansion::Never {
            for _ in 0..indentation {
                content = content.indent();
            }
        }

        let document =
            Document::sequence([Document::text("("), content, edge, Document::text(")")]);

        Ok(if expand == Expansion::Never {
            Document::Flat(Box::new(document))
        } else {
            document.group()
        })
    }

    fn has_own_comments(&self, view: View<'tree, 'source>) -> bool {
        self.tree.tokens.iter().any(|token| {
            matches!(token.kind, TokenKind::Comment | TokenKind::BlockComment)
                && token.span.start >= view.span().start
                && token.span.end <= view.span().end
                && !view.children().any(|child| {
                    let (start, end) = if child.kind() == Kind::Block {
                        self.boundaries(child)
                    } else {
                        (child.span().start, child.span().end)
                    };

                    token.span.start >= start && token.span.end <= end
                })
        })
    }

    pub(in crate::format) fn node(
        &self,
        view: View<'tree, 'source>,
    ) -> io::Result<Document<'source>> {
        if self.has_own_comments(view)
            && matches!(
                view.kind(),
                Kind::Table | Kind::Arguments | Kind::Parameters
            )
        {
            return self.commented(view);
        }

        if view.kind() != Kind::Block
            && (self.has_own_comments(view)
                || self
                    .held
                    .iter()
                    .any(|range| range.start < view.span().end && range.end > view.span().start))
        {
            return Ok(Document::text(self.text(view)));
        }

        if matches!(view.kind(), Kind::Call | Kind::MethodCall)
            && let Some(document) = self.chain(view)?
        {
            return Ok(document);
        }

        if view.kind() == Kind::String {
            return Ok(Document::text(crate::format::literals::quote(
                self.text(view),
                self.options.quotes,
            )));
        }

        if view.kind() == Kind::Number {
            return Ok(Document::text(crate::format::literals::number(
                self.text(view),
                self.options.leading_zero,
            )));
        }

        let parts = view
            .parts()
            .ok_or_else(|| io::Error::other("unsupported syntax shape"))?;

        self.layout(view, parts)
    }

    fn layout(
        &self,
        view: View<'tree, 'source>,
        parts: Parts<'tree, 'source>,
    ) -> io::Result<Document<'source>> {
        match parts {
            Parts::Markup { .. }
            | Parts::Tag { .. }
            | Parts::MarkupName { .. }
            | Parts::MarkupAttributes { .. }
            | Parts::MarkupChildren { .. }
            | Parts::MarkupAttribute { .. }
            | Parts::MarkupSpread { .. }
            | Parts::MarkupInferred { .. }
            | Parts::MarkupExpression { .. }
            | Parts::Root { .. }
            | Parts::Block { .. } => self.fundamentals(view, &parts),

            Parts::Local { .. }
            | Parts::Assignment { .. }
            | Parts::CallStatement { .. }
            | Parts::Binding { .. }
            | Parts::Parameters { .. } => self.bindings(view, parts),

            Parts::Arguments { .. } => self.invocation(view, parts),
            Parts::Function { .. } => self.function(view, &parts),

            Parts::Returns { .. }
            | Parts::FunctionName { .. }
            | Parts::If { .. }
            | Parts::Branch { .. }
            | Parts::Body { .. } => self.branches(view, parts),

            Parts::While { .. }
            | Parts::Repeat { .. }
            | Parts::NumericFor { .. }
            | Parts::GenericFor { .. }
            | Parts::Return { .. } => self.loops(parts),

            Parts::Export { .. }
            | Parts::TypeAlias { .. }
            | Parts::Declaration { .. }
            | Parts::ClassDeclaration { .. }
            | Parts::Class { .. }
            | Parts::Property { .. }
            | Parts::Extends { .. } => self.declarations(view, parts),

            Parts::Leaf
            | Parts::Attributes { .. }
            | Parts::Attribute { .. }
            | Parts::Generics { .. }
            | Parts::Generic { .. }
            | Parts::Variadic { .. }
            | Parts::Unary { .. }
            | Parts::Binary { .. }
            | Parts::Group { .. }
            | Parts::TypeOf { .. }
            | Parts::Call { .. } => self.expressions(view, parts),

            Parts::MethodCall { .. }
            | Parts::Field { .. }
            | Parts::Index { .. }
            | Parts::Instantiate { .. }
            | Parts::Assertion { .. }
            | Parts::Conditional { .. }
            | Parts::Interpolation { .. } => self.access(view, parts),

            Parts::Table { .. } | Parts::TableField { .. } => self.tables(view, parts),

            Parts::TypeName { .. }
            | Parts::TypeTable { .. }
            | Parts::TypeField { .. }
            | Parts::TypeFunction { .. }
            | Parts::TypeGroup { .. } => self.types(view, parts),

            Parts::TypePack { .. }
            | Parts::VariadicType { .. }
            | Parts::TypeParameter { .. }
            | Parts::TypeArguments { .. }
            | Parts::TypeUnion { .. }
            | Parts::TypeIntersection { .. }
            | Parts::TypeOptional { .. } => self.packs(view, parts),
        }
    }

    fn fundamentals(
        &self,
        view: View<'tree, 'source>,
        parts: &Parts<'tree, 'source>,
    ) -> io::Result<Document<'source>> {
        match parts {
            Parts::Markup { .. }
            | Parts::Tag { .. }
            | Parts::MarkupName { .. }
            | Parts::MarkupAttributes { .. }
            | Parts::MarkupChildren { .. }
            | Parts::MarkupAttribute { .. }
            | Parts::MarkupSpread { .. }
            | Parts::MarkupInferred { .. }
            | Parts::MarkupExpression { .. } => {
                Err(io::Error::other("formatting requires Luau syntax"))
            }

            Parts::Root { .. } => self.root(),
            Parts::Block { .. } => self.block(view, view.span().start, view.span().end),

            _ => unreachable!(),
        }
    }

    fn bindings(
        &self,
        view: View<'tree, 'source>,
        parts: Parts<'tree, 'source>,
    ) -> io::Result<Document<'source>> {
        let document = match parts {
            Parts::Local { bindings, values } => {
                let initialized = values.clone().next().is_some();

                Document::sequence([
                    Document::text(if view.kind() == Kind::Constant {
                        "const "
                    } else {
                        "local "
                    }),
                    self.list(bindings)?,
                    Document::text(if initialized { " =" } else { "" }),
                    if initialized {
                        self.values(values)?
                    } else {
                        Document::text("")
                    },
                ])
                .group()
            }

            Parts::Assignment {
                targets,
                operator,
                values,
            } => Document::sequence([
                self.list(targets)?,
                Document::text(" "),
                self.node(operator)?,
                self.values(values)?,
            ])
            .group(),

            Parts::CallStatement { call } => self.node(call)?,

            Parts::Binding { name, annotation } => Document::sequence([
                self.node(name)?,
                Document::text(if annotation.is_some() { ": " } else { "" }),
                self.optional(annotation)?,
            ]),

            Parts::Parameters { parameters } => {
                return self.delimited(
                    "(",
                    ")",
                    parameters,
                    self.options.spacing.parentheses,
                    false,
                    false,
                );
            }

            _ => unreachable!(),
        };

        Ok(document)
    }

    fn invocation(
        &self,
        view: View<'tree, 'source>,
        parts: Parts<'tree, 'source>,
    ) -> io::Result<Document<'source>> {
        match parts {
            Parts::Arguments { values } => {
                if let Some(value) = values
                    .clone()
                    .next()
                    .filter(|_| values.clone().count() == 1)
                {
                    let bare = match self.options.calls.parentheses {
                        Parentheses::Always => false,
                        Parentheses::OmitString => value.kind() == Kind::String,
                        Parentheses::OmitTable => value.kind() == Kind::Table,

                        Parentheses::OmitOptional => {
                            matches!(value.kind(), Kind::String | Kind::Table)
                        }

                        Parentheses::Preserve => !self.text(view).starts_with('('),
                    };

                    if bare {
                        return Ok(Document::sequence([Document::text(" "), self.node(value)?]));
                    }
                }

                let arguments = self.arguments(values)?;

                Ok(Document::sequence([
                    Document::text(
                        if matches!(
                            self.options.spacing.function_names,
                            Separation::Calls | Separation::Always
                        ) {
                            " "
                        } else {
                            ""
                        },
                    ),
                    arguments,
                ]))
            }

            _ => unreachable!(),
        }
    }

    fn function(
        &self,
        view: View<'tree, 'source>,
        parts: &Parts<'tree, 'source>,
    ) -> io::Result<Document<'source>> {
        let document = match *parts {
            Parts::Function {
                attributes,
                name,
                generics,
                parameters,
                returns,
                body,
            } => {
                let prefix = match view.kind() {
                    Kind::LocalFunction if self.text(view).trim_start().starts_with("const") => {
                        "const function"
                    }

                    Kind::LocalFunction => "local function",
                    Kind::TypeFunction => "type function",
                    Kind::Declaration => "declare function",
                    Kind::Method if self.text(view).starts_with("public") => "public function",
                    _ => "function",
                };

                let mut documents = Vec::new();

                if let Some(attributes) = attributes {
                    documents.extend([self.node(attributes)?, Document::Hard]);
                }

                documents.extend([
                    Document::text(prefix),
                    Document::text(if name.is_some() { " " } else { "" }),
                    self.optional(name)?,
                    self.optional(generics)?,
                    Document::text(
                        if matches!(
                            self.options.spacing.function_names,
                            Separation::Definitions | Separation::Always
                        ) {
                            " "
                        } else {
                            ""
                        },
                    ),
                    self.parameters(parameters)?,
                ]);

                let parameter_index = documents.len() - 1;

                if let Some(returns) = returns {
                    documents.extend([Document::text(": "), self.node(returns)?]);
                }

                if body.is_none()
                    && self.options.functions.parameters.expand == Expansion::Needed
                    && parameters.children().next().is_some()
                    && self.gap(view.span().start, view.span().end).is_empty()
                    && documents.iter().all(|document| document.width().is_some())
                {
                    let flat = Document::sequence(documents.clone()).flattened();

                    documents[parameter_index] = self.parenthesized(
                        parameters.children(),
                        Expansion::Always,
                        self.options.functions.parameters.indentation,
                    )?;

                    return Ok(Document::Choice(
                        Box::new(flat),
                        Box::new(Document::sequence(documents)),
                    )
                    .group());
                }

                if let Some(body) = body {
                    let (start, end) = self.boundaries(body);

                    if body.children().next().is_none() && self.gap(start, end).is_empty() {
                        documents.extend([Document::Hard, Document::text("end")]);
                    } else if matches!(
                        self.options.blocks.collapse,
                        Collapse::Functions | Collapse::Always
                    ) && let Some(collapsed) = self.collapsed(body)?
                    {
                        documents.extend([collapsed, Document::text("end")]);
                    } else {
                        documents.extend([self.body(body)?, Document::Hard, Document::text("end")]);
                    }
                }

                Document::sequence(documents).group()
            }

            _ => unreachable!(),
        };

        Ok(document)
    }

    fn branches(
        &self,
        view: View<'tree, 'source>,
        parts: Parts<'tree, 'source>,
    ) -> io::Result<Document<'source>> {
        let document = match parts {
            Parts::Returns { annotation } => self.node(annotation)?,

            Parts::FunctionName { path, method } => Document::sequence([
                Document::join(
                    &Document::text("."),
                    path.map(|part| self.node(part))
                        .collect::<io::Result<Vec<_>>>()?,
                ),
                Document::text(if method.is_some() { ":" } else { "" }),
                self.optional(method)?,
            ]),

            Parts::If {
                branches,
                otherwise,
            } => {
                if otherwise.is_none()
                    && branches.clone().count() == 1
                    && matches!(
                        self.options.blocks.collapse,
                        Collapse::Conditionals | Collapse::Always
                    )
                    && let Some(Parts::Branch { condition, body }) =
                        branches.clone().next().and_then(View::parts)
                    && let Some(collapsed) = self.collapsed(body)?
                {
                    return Ok(Document::sequence([
                        Document::text("if "),
                        self.node(condition)?,
                        Document::text(" then"),
                        collapsed,
                        Document::text("end"),
                    ])
                    .group());
                }

                let mut documents = Vec::new();

                for (index, branch) in branches.enumerate() {
                    if index != 0 {
                        documents.extend([Document::Hard, Document::text("else")]);
                    }

                    documents.push(self.node(branch)?);
                }

                if let Some(otherwise) = otherwise {
                    documents.extend([Document::Hard, self.node(otherwise)?]);
                }

                documents.extend([Document::Hard, Document::text("end")]);

                Document::sequence(documents)
            }

            Parts::Branch { condition, body } => Document::sequence([
                Document::text("if "),
                self.node(condition)?,
                Document::text(" then"),
                self.body(body)?,
            ]),

            Parts::Body { body } => {
                let mut documents = vec![
                    Document::text(if view.kind() == Kind::Else {
                        "else"
                    } else {
                        "do"
                    }),
                    self.body(body)?,
                ];

                if view.kind() == Kind::Do {
                    documents.extend([Document::Hard, Document::text("end")]);
                }

                Document::sequence(documents)
            }

            _ => unreachable!(),
        };

        Ok(document)
    }

    fn loops(&self, parts: Parts<'tree, 'source>) -> io::Result<Document<'source>> {
        let document = match parts {
            Parts::While { condition, body } => Document::sequence([
                Document::text("while "),
                self.node(condition)?,
                Document::text(" do"),
                self.body(body)?,
                Document::Hard,
                Document::text("end"),
            ]),

            Parts::Repeat { body, condition } => Document::sequence([
                Document::text("repeat"),
                self.body(body)?,
                Document::Hard,
                Document::text("until "),
                self.node(condition)?,
            ]),

            Parts::NumericFor {
                binding,
                start,
                end,
                step,
                body,
            } => Document::sequence([
                Document::text("for "),
                self.node(binding)?,
                Document::text(" = "),
                self.node(start)?,
                Document::text(", "),
                self.node(end)?,
                Document::text(if step.is_some() { ", " } else { "" }),
                self.optional(step)?,
                Document::text(" do"),
                self.body(body)?,
                Document::Hard,
                Document::text("end"),
            ]),

            Parts::GenericFor {
                bindings,
                values,
                body,
            } => Document::sequence([
                Document::text("for "),
                self.list(bindings)?.group(),
                Document::text(" in "),
                self.list(values)?.group(),
                Document::text(" do"),
                self.body(body)?,
                Document::Hard,
                Document::text("end"),
            ]),

            Parts::Return { values } => Document::sequence([
                Document::text(if values.clone().next().is_some() {
                    "return "
                } else {
                    "return"
                }),
                self.list(values)?,
            ])
            .group(),

            _ => unreachable!(),
        };

        Ok(document)
    }

    fn declarations(
        &self,
        view: View<'tree, 'source>,
        parts: Parts<'tree, 'source>,
    ) -> io::Result<Document<'source>> {
        let document = match parts {
            Parts::Export {
                attributes,
                declaration,
            } => Document::sequence([
                self.optional(attributes)?,
                Document::text(if attributes.is_some() {
                    "\nexport "
                } else {
                    "export "
                }),
                self.node(declaration)?,
            ]),

            Parts::TypeAlias {
                name,
                generics,
                annotation,
            } => Document::sequence([
                Document::text("type "),
                self.node(name)?,
                self.optional(generics)?,
                Document::text(" = "),
                self.node(annotation)?,
            ])
            .group(),

            Parts::Declaration { name, annotation } => Document::sequence([
                Document::text("declare "),
                self.node(name)?,
                Document::text(": "),
                self.node(annotation)?,
            ]),

            Parts::ClassDeclaration { class } => {
                Document::sequence([Document::text("declare extern "), self.node(class)?])
            }

            Parts::Class {
                name,
                extends,
                members,
            } => Document::sequence([
                Document::text(if self.text(view).starts_with("type") {
                    "type "
                } else if self.text(view).starts_with("open") {
                    "open class "
                } else {
                    "class "
                }),
                self.node(name)?,
                self.optional(extends)?,
                Document::text(if self.text(view).starts_with("type") {
                    " with"
                } else {
                    ""
                }),
                Document::sequence(
                    members
                        .map(|member| Ok(Document::sequence([Document::Hard, self.node(member)?])))
                        .collect::<io::Result<Vec<_>>>()?,
                )
                .indent(),
                Document::Hard,
                Document::text("end"),
            ]),

            Parts::Property { binding } => Document::sequence([
                Document::text(&self.source[view.span().start..binding.span().start]),
                self.node(binding)?,
            ]),

            Parts::Extends { superclass } => {
                Document::sequence([Document::text(" extends "), self.node(superclass)?])
            }

            _ => unreachable!(),
        };

        Ok(document)
    }

    fn expressions(
        &self,
        view: View<'tree, 'source>,
        parts: Parts<'tree, 'source>,
    ) -> io::Result<Document<'source>> {
        let document = match parts {
            Parts::Leaf | Parts::Attributes { .. } | Parts::Attribute { .. } => {
                Document::text(self.text(view))
            }

            Parts::Generics { parameters } => {
                return self.delimited("<", ">", parameters, false, false, false);
            }

            Parts::Generic { name, default } => Document::sequence([
                self.node(name)?,
                Document::text(if view.kind() == Kind::GenericPack {
                    "..."
                } else {
                    ""
                }),
                Document::text(if default.is_some() { " = " } else { "" }),
                self.optional(default)?,
            ]),

            Parts::Variadic { annotation } => Document::sequence([
                Document::text("..."),
                Document::text(if annotation.is_some() { ": " } else { "" }),
                self.optional(annotation)?,
            ]),

            Parts::Unary { operator, operand } => Document::sequence([
                self.node(operator)?,
                Document::text(
                    if self.text(operator) == "not"
                        || self.text(operator) == "-" && self.text(operand).starts_with('-')
                    {
                        " "
                    } else {
                        ""
                    },
                ),
                self.node(operand)?,
            ]),

            Parts::Binary { .. } => self.binary(view)?,

            Parts::Group { expression } | Parts::TypeOf { expression, .. } => {
                self.grouped(expression, view.kind() == Kind::TypeOf)?
            }

            Parts::Call { callee, arguments } => {
                Document::sequence([self.node(callee)?, self.node(arguments)?])
            }

            _ => unreachable!(),
        };

        Ok(document)
    }

    fn access(
        &self,
        view: View<'tree, 'source>,
        parts: Parts<'tree, 'source>,
    ) -> io::Result<Document<'source>> {
        let document = match parts {
            Parts::MethodCall {
                receiver,
                method,
                types,
                arguments,
            } => Document::sequence([
                self.node(receiver)?,
                Document::text(":"),
                self.node(method)?,
                Document::text(if types.is_some() { "<" } else { "" }),
                self.optional(types)?,
                Document::text(if types.is_some() { ">" } else { "" }),
                self.node(arguments)?,
            ]),

            Parts::Field { receiver, name } => {
                Document::sequence([self.node(receiver)?, Document::text("."), self.node(name)?])
            }

            Parts::Index { receiver, key } => Document::sequence([
                self.node(receiver)?,
                Document::text(
                    if self.options.spacing.brackets || self.text(key).starts_with('[') {
                        "[ "
                    } else {
                        "["
                    },
                ),
                self.node(key)?,
                Document::text(
                    if self.options.spacing.brackets || self.text(key).starts_with('[') {
                        " ]"
                    } else {
                        "]"
                    },
                ),
            ]),

            Parts::Instantiate {
                expression,
                arguments,
            } => Document::sequence([
                self.node(expression)?,
                Document::text("<"),
                self.node(arguments)?,
                Document::text(">"),
            ]),

            Parts::Assertion {
                expression,
                annotation,
            } => Document::sequence([
                self.node(expression)?,
                Document::text(" :: "),
                self.node(annotation)?,
            ]),

            Parts::Conditional { .. } => self.conditional(view)?,

            Parts::Interpolation { segments } => Document::sequence(
                segments
                    .map(|segment| self.node(segment))
                    .collect::<io::Result<Vec<_>>>()?,
            ),

            _ => unreachable!(),
        };

        Ok(document)
    }

    fn tables(
        &self,
        view: View<'tree, 'source>,
        parts: Parts<'tree, 'source>,
    ) -> io::Result<Document<'source>> {
        let document = match parts {
            Parts::Table { fields } => {
                let text = self.text(view);

                let forced = text.strip_prefix('{').is_some_and(|rest| {
                    rest.trim_start_matches([' ', '\t', '\r']).starts_with('\n')
                }) || self.options.expand_on_trailing_comma
                    && text.trim_end_matches('}').trim_end().ends_with(',');

                return self.delimited("{", "}", fields, self.options.spacing.braces, true, forced);
            }

            Parts::TableField {
                key,
                value,
                indexed,
            } => {
                let spaced = indexed
                    && (self.options.spacing.brackets
                        || key.is_some_and(|key| self.text(key).starts_with('[')));

                Document::sequence([
                    Document::text(if spaced {
                        "[ "
                    } else if indexed {
                        "["
                    } else {
                        ""
                    }),
                    self.optional(key)?,
                    Document::text(if spaced {
                        " ]"
                    } else if indexed {
                        "]"
                    } else {
                        ""
                    }),
                    Document::text(if key.is_some() { " = " } else { "" }),
                    self.node(value)?,
                ])
                .group()
            }

            _ => unreachable!(),
        };

        Ok(document)
    }

    fn types(
        &self,
        view: View<'tree, 'source>,
        parts: Parts<'tree, 'source>,
    ) -> io::Result<Document<'source>> {
        let document = match parts {
            Parts::TypeName {
                namespace,
                name,
                arguments,
            } => Document::sequence([
                self.optional(namespace)?,
                Document::text(if namespace.is_some() { "." } else { "" }),
                self.node(name)?,
                self.optional(arguments)?,
            ]),

            Parts::TypeTable {
                access,
                element,
                fields,
            } => {
                if !self.options.types.tables.enabled && self.text(view).contains('\n') {
                    return Ok(Document::text(self.text(view)));
                }

                if let Some(element) = element {
                    Document::sequence([
                        Document::text("{ "),
                        self.optional(access)?,
                        Document::text(if access.is_some() { " " } else { "" }),
                        self.node(element)?,
                        Document::text(" }"),
                    ])
                    .group()
                } else {
                    return self.table_type(fields);
                }
            }

            Parts::TypeField {
                access,
                key,
                annotation,
            } => {
                let indexed = view.kind() == Kind::TypeIndexer || key.kind() == Kind::String;

                Document::sequence([
                    self.optional(access)?,
                    Document::text(if access.is_some() { " " } else { "" }),
                    Document::text(if indexed { "[" } else { "" }),
                    self.node(key)?,
                    Document::text(if indexed { "]: " } else { ": " }),
                    self.node(annotation)?,
                ])
            }

            Parts::TypeFunction {
                attributes,
                generics,
                parameters,
                returns,
            } => Document::sequence([
                self.optional(attributes)?,
                self.optional(generics)?,
                self.node(parameters)?,
                Document::text(" -> "),
                self.node(returns)?,
            ])
            .group(),

            Parts::TypeGroup { annotation } => Document::sequence([
                Document::text("("),
                self.node(annotation)?,
                Document::text(")"),
            ]),

            _ => unreachable!(),
        };

        Ok(document)
    }

    fn packs(
        &self,
        view: View<'tree, 'source>,
        parts: Parts<'tree, 'source>,
    ) -> io::Result<Document<'source>> {
        let document = match parts {
            Parts::TypePack { types } => {
                return self.delimited(
                    "(",
                    ")",
                    types,
                    self.options.spacing.parentheses,
                    false,
                    false,
                );
            }

            Parts::VariadicType { annotation } => {
                Document::sequence([Document::text("..."), self.node(annotation)?])
            }

            Parts::TypeParameter { name, annotation } => Document::sequence([
                self.node(name)?,
                Document::text(": "),
                self.node(annotation)?,
            ]),

            Parts::TypeArguments { types } => {
                return self.delimited("<", ">", types, false, false, false);
            }

            Parts::TypeUnion { .. } | Parts::TypeIntersection { .. } => self.operators(view)?,

            Parts::TypeOptional { annotation } => {
                Document::sequence([self.node(annotation)?, Document::text("?")])
            }

            _ => unreachable!(),
        };

        Ok(document)
    }
}
