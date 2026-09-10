mod blocks;
mod chains;
mod comments;
mod conditional;
mod expressions;
mod types;

use std::{io, ops::Range};

use vermis::{Children, Kind, Parts, TokenKind, Tree, View};

use super::{
    Options,
    configuration::{CallStyle, Collapse, Expansion, Parentheses, Semicolons, Spacing},
    document::Document,
};

pub(super) struct Emitter<'tree, 'source> {
    source: &'source str,
    tree: &'tree Tree<'source>,
    options: &'tree Options,
    held: &'tree [Range<usize>],
}

impl<'tree, 'source> Emitter<'tree, 'source> {
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

        let comma = if table && self.options.trailing_comma {
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

        let options = &self.options.function_declaration;

        self.parenthesized(parameters.children(), options.expand, options.indent)
    }

    fn arguments(&self, values: Children<'tree, 'source>) -> io::Result<Document<'source>> {
        let options = &self.options.function_call;
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

        self.parenthesized(values, options.expand, options.indent)
    }

    fn parenthesized(
        &self,
        children: Children<'tree, 'source>,
        expand: Expansion,
        indent: usize,
    ) -> io::Result<Document<'source>> {
        if children.clone().next().is_none() {
            return Ok(Document::text("()"));
        }

        let edge = if expand == Expansion::Always {
            Document::Hard
        } else if self.options.space_inside_parens {
            Document::Line
        } else {
            Document::Soft
        };

        let mut content = Document::sequence([edge.clone(), self.list(children)?]);

        if expand != Expansion::Never {
            for _ in 0..indent {
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

    #[allow(clippy::too_many_lines)]
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
            return Ok(Document::text(super::literals::quote(
                self.text(view),
                self.options.quote_style,
            )));
        }

        if view.kind() == Kind::Number {
            return Ok(Document::text(super::literals::number(
                self.text(view),
                self.options.leading_zero,
            )));
        }

        let parts = view
            .parts()
            .ok_or_else(|| io::Error::other("unsupported syntax shape"))?;

        let document = match parts {
            Parts::Markup { .. }
            | Parts::Tag { .. }
            | Parts::MarkupName { .. }
            | Parts::MarkupAttributes { .. }
            | Parts::MarkupChildren { .. }
            | Parts::MarkupAttribute { .. }
            | Parts::MarkupSpread { .. }
            | Parts::MarkupInferred { .. }
            | Parts::MarkupExpression { .. } => {
                return Err(io::Error::other("formatting requires Luau syntax"));
            }

            Parts::Root { .. } => return self.root(),
            Parts::Block { .. } => return self.block(view, view.span().start, view.span().end),

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
                    self.options.space_inside_parens,
                    false,
                    false,
                );
            }

            Parts::Arguments { values } => {
                if let Some(value) = values
                    .clone()
                    .next()
                    .filter(|_| values.clone().count() == 1)
                {
                    let bare = match self.options.call_parentheses {
                        Parentheses::Always => false,
                        Parentheses::NoSingleString => value.kind() == Kind::String,
                        Parentheses::NoSingleTable => value.kind() == Kind::Table,
                        Parentheses::None => matches!(value.kind(), Kind::String | Kind::Table),
                        Parentheses::Input => !self.text(view).starts_with('('),
                    };

                    if bare {
                        return Ok(Document::sequence([Document::text(" "), self.node(value)?]));
                    }
                }

                let arguments = self.arguments(values)?;

                return Ok(Document::sequence([
                    Document::text(
                        if matches!(
                            self.options.space_after_function_names,
                            Spacing::Calls | Spacing::Always
                        ) {
                            " "
                        } else {
                            ""
                        },
                    ),
                    arguments,
                ]));
            }

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
                            self.options.space_after_function_names,
                            Spacing::Definitions | Spacing::Always
                        ) {
                            " "
                        } else {
                            ""
                        },
                    ),
                    self.parameters(parameters)?,
                ]);

                if let Some(returns) = returns {
                    documents.extend([Document::text(": "), self.node(returns)?]);
                }

                if let Some(body) = body {
                    let (start, end) = self.boundaries(body);

                    if body.children().next().is_none() && self.gap(start, end).is_empty() {
                        documents.extend([Document::Hard, Document::text("end")]);
                    } else if matches!(
                        self.options.collapse_simple_statement,
                        Collapse::FunctionOnly | Collapse::Always
                    ) && let Some(collapsed) = self.collapsed(body)?
                    {
                        documents.extend([collapsed, Document::text("end")]);
                    } else {
                        documents.extend([self.body(body)?, Document::Hard, Document::text("end")]);
                    }
                }

                Document::sequence(documents).group()
            }

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
                        self.options.collapse_simple_statement,
                        Collapse::ConditionalOnly | Collapse::Always
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
                    if self.options.space_inside_brackets || self.text(key).starts_with('[') {
                        "[ "
                    } else {
                        "["
                    },
                ),
                self.node(key)?,
                Document::text(
                    if self.options.space_inside_brackets || self.text(key).starts_with('[') {
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

            Parts::Table { fields } => {
                let text = self.text(view);

                let forced = text.strip_prefix('{').is_some_and(|rest| {
                    rest.trim_start_matches([' ', '\t', '\r']).starts_with('\n')
                }) || self.options.magic_trailing_comma
                    && text.trim_end_matches('}').trim_end().ends_with(',');

                return self.delimited(
                    "{",
                    "}",
                    fields,
                    self.options.space_inside_braces,
                    true,
                    forced,
                );
            }

            Parts::TableField {
                key,
                value,
                indexed,
            } => {
                let spaced = indexed
                    && (self.options.space_inside_brackets
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
                if !self.options.table_types.enabled && self.text(view).contains('\n') {
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

            Parts::TypePack { types } => {
                return self.delimited(
                    "(",
                    ")",
                    types,
                    self.options.space_inside_parens,
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
        };

        Ok(document)
    }
}
