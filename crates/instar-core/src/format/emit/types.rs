use super::{Document, Emitter, io};

impl<'tree, 'source> Emitter<'tree, 'source> {
    pub(super) fn operators(
        &self,
        view: vermis::View<'tree, 'source>,
    ) -> io::Result<Document<'source>> {
        use crate::format::configuration::TypeExpansion;
        use vermis::{Kind, Parts};
        let mut pending = vec![view];
        let mut members = Vec::new();
        let mut overloads = view.kind() == Kind::TypeIntersection;

        while let Some(member) = pending.pop() {
            if member.kind() == view.kind() {
                pending.extend(member.children().rev());
            } else {
                let mut signature = member;

                while let Some(Parts::TypeGroup { annotation }) = signature.parts() {
                    signature = annotation;
                }

                overloads &= matches!(signature.parts(), Some(Parts::TypeFunction { .. }));
                members.push(self.node(member)?);
            }
        }

        let nested = self.tree.nodes.iter().any(|node| {
            matches!(
                node.kind,
                Kind::TypeGroup | Kind::TypeArguments | Kind::TypePack
            ) && node.span.start < view.span().start
                && node.span.end >= view.span().end
        });

        let expansion = if nested {
            TypeExpansion::Needed
        } else {
            self.options.types.operators.expand
        };

        let operator = if view.kind() == Kind::TypeUnion {
            "| "
        } else {
            "& "
        };

        if expansion == TypeExpansion::Needed && overloads {
            let flat = Document::join(&Document::text(" & "), members.clone());
            let mut members = members.into_iter();
            let first = members.next().expect("overload signature");

            let expanded = Document::sequence([
                first,
                Document::sequence(
                    members.flat_map(|member| [Document::Hard, Document::text("& "), member]),
                )
                .indent(),
            ]);

            return Ok(if self.text(view).contains('\n') {
                expanded
            } else {
                Document::Choice(Box::new(flat), Box::new(expanded)).group()
            });
        }

        if expansion == TypeExpansion::Needed {
            return Ok(Document::sequence([
                Document::text(if self.text(view).starts_with(['|', '&']) {
                    operator
                } else {
                    ""
                }),
                Document::join(&Document::text(format!(" {operator}")), members),
            ]));
        }

        let flat =
            Document::join(&Document::text(format!(" {operator}")), members.clone()).flattened();

        let expanded = Document::sequence(
            members
                .into_iter()
                .flat_map(|member| [Document::Hard, Document::text(operator), member]),
        )
        .indent();

        Ok(if expansion == TypeExpansion::Always {
            expanded
        } else {
            Document::Choice(Box::new(flat), Box::new(expanded)).group()
        })
    }

    pub(super) fn table_type(
        &self,
        fields: vermis::Children<'tree, 'source>,
    ) -> io::Result<Document<'source>> {
        if fields.clone().next().is_none() {
            return Ok(Document::text("{}"));
        }

        let options = &self.options.types.tables;
        let separator = options.separator.text();

        let mut previous = None;
        let mut gaps = Vec::new();

        let fields = fields
            .map(|field| {
                gaps.push(
                    if options.blank_lines == crate::format::configuration::Gaps::Preserve {
                        previous.map_or(0, |end| {
                            self.source[end..field.span().start]
                                .matches('\n')
                                .count()
                                .saturating_sub(1)
                        })
                    } else {
                        0
                    },
                );

                previous = Some(field.span().end);

                self.node(field)
            })
            .collect::<io::Result<Vec<_>>>()?;

        let flat = Document::sequence([
            Document::text("{ "),
            Document::join(&Document::text(format!("{separator} ")), fields.clone()),
            Document::text(" }"),
        ]);

        if !options.enabled {
            return Ok(Document::Flat(Box::new(flat)));
        }

        let forced = gaps.iter().any(|gap| *gap > 0)
            || flat.width().is_none_or(|width| width > options.width);

        let edge = if forced {
            Document::Hard
        } else if self.options.spacing.braces {
            Document::Line
        } else {
            Document::Soft
        };

        let suffix = if self.options.trailing_separator {
            Document::Choice(
                Box::new(Document::text("")),
                Box::new(Document::text(separator)),
            )
        } else {
            Document::text("")
        };

        let mut contents = Vec::new();

        for (index, (field, gap)) in fields.into_iter().zip(gaps).enumerate() {
            if index > 0 {
                contents.push(Document::text(separator));

                if gap > 0 {
                    contents.extend(std::iter::repeat_n(Document::Hard, gap + 1));
                } else {
                    contents.push(Document::Line);
                }
            }

            contents.push(field);
        }

        Ok(Document::sequence([
            Document::text("{"),
            Document::sequence([edge.clone(), Document::sequence(contents), suffix]).indent(),
            edge,
            Document::text("}"),
        ])
        .group())
    }
}
