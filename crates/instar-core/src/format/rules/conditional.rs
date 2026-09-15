use super::{Document, DocumentBuilder, Kind, Parts, View, io};
use crate::project::configuration::format::{ConditionalExpansion, ConditionalStyle, Placement};

impl<'tree, 'source> DocumentBuilder<'tree, 'source> {
    pub(super) fn conditional(&self, view: View<'tree, 'source>) -> io::Result<Document<'source>> {
        let options = &self.options.conditionals;
        let mut branches = Vec::new();
        let mut current = view;

        let otherwise = loop {
            let Some(Parts::Conditional {
                condition,
                truthy,
                falsy,
            }) = current.parts()
            else {
                return Err(io::Error::other("invalid conditional expression"));
            };

            branches.push((self.node(condition)?, self.node(truthy)?));

            if falsy.kind() == Kind::Conditional && self.text(falsy).starts_with("elseif") {
                current = falsy;
            } else {
                break self.node(falsy)?;
            }
        };

        let mut flat = Vec::new();

        for (index, (condition, value)) in branches.iter().enumerate() {
            flat.extend([
                Document::text(if index == 0 { "if " } else { "elseif " }),
                condition.clone(),
                Document::text(" then "),
                value.clone(),
                Document::Line,
            ]);
        }

        flat.extend([Document::text("else "), otherwise.clone()]);
        let flat = Document::sequence(flat);

        if options.expand == ConditionalExpansion::Never {
            return Ok(flat.indent().group());
        }

        let nested = self.tree.nodes.iter().any(|node| {
            node.kind == Kind::Conditional
                && node.span.start < view.span().start
                && node.span.end >= view.span().end
        });

        let wide = flat.width().is_none_or(|width| width > options.width);
        let open = wide || options.expand == ConditionalExpansion::Always && !nested;

        let split = if open { Document::Hard } else { Document::Line };

        let mut documents = Vec::new();

        for (index, (condition, value)) in branches.into_iter().enumerate() {
            let keyword = Document::text(if index == 0 { "if " } else { "elseif " });

            match options.style {
                ConditionalStyle::Block => {
                    if index != 0 {
                        documents.push(split.clone());
                    }

                    documents.extend([
                        keyword,
                        condition,
                        Document::text(" then"),
                        self.conditional_indent(Document::sequence([split.clone(), value])),
                    ]);
                }

                ConditionalStyle::Leading => {
                    documents.push(if index == 0 {
                        keyword
                    } else {
                        self.conditional_indent(Document::sequence([split.clone(), keyword]))
                    });

                    documents.extend([
                        condition,
                        self.conditional_indent(Document::sequence([
                            split.clone(),
                            Document::text("then "),
                            value,
                        ])),
                    ]);
                }
            }
        }

        match options.style {
            ConditionalStyle::Block => documents.extend([
                split.clone(),
                Document::text("else"),
                self.conditional_indent(Document::sequence([split, otherwise])),
            ]),

            ConditionalStyle::Leading => {
                documents.push(self.conditional_indent(Document::sequence([
                    split,
                    Document::text("else "),
                    otherwise,
                ])));
            }
        }

        Ok(Document::sequence(documents).group())
    }

    fn conditional_indent(&self, mut document: Document<'source>) -> Document<'source> {
        for _ in 0..self.options.conditionals.indentation {
            document = document.indent();
        }

        document
    }

    pub(super) fn values(
        &self,
        values: vermis::Children<'tree, 'source>,
    ) -> io::Result<Document<'source>> {
        let single_conditional = values.clone().count() == 1
            && values
                .clone()
                .next()
                .is_some_and(|value| value.kind() == Kind::Conditional);

        let document = self.list(values)?;

        if single_conditional
            && self.options.conditionals.placement == Placement::NextLine
            && document.width().is_none()
        {
            Ok(self.conditional_indent(Document::sequence([Document::Hard, document])))
        } else {
            Ok(Document::sequence([Document::text(" "), document]))
        }
    }
}
