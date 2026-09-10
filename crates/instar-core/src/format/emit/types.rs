use super::{Document, Emitter, io};

impl<'tree, 'source> Emitter<'tree, 'source> {
    pub(super) fn operators(
        &self,
        view: vermis::View<'tree, 'source>,
    ) -> io::Result<Document<'source>> {
        use crate::format::configuration::TypeExpansion;
        use vermis::Kind;
        let mut pending = vec![view];
        let mut members = Vec::new();

        while let Some(member) = pending.pop() {
            if member.kind() == view.kind() {
                pending.extend(member.children().rev());
            } else {
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
            TypeExpansion::Auto
        } else {
            self.options.type_operators.expand
        };

        let operator = if view.kind() == Kind::TypeUnion {
            "| "
        } else {
            "& "
        };

        if expansion == TypeExpansion::Auto {
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

        let options = &self.options.table_types;
        let separator = options.separator.text();

        let fields = fields
            .map(|field| self.node(field))
            .collect::<io::Result<Vec<_>>>()?;

        let flat = Document::sequence([
            Document::text("{ "),
            Document::join(&Document::text(format!("{separator} ")), fields.clone()),
            Document::text(" }"),
        ]);

        if !options.enabled {
            return Ok(Document::Flat(Box::new(flat)));
        }

        let forced = flat.width().is_none_or(|width| width > options.width);

        let edge = if forced {
            Document::Hard
        } else if self.options.space_inside_braces {
            Document::Line
        } else {
            Document::Soft
        };

        let suffix = if self.options.trailing_comma {
            Document::Choice(
                Box::new(Document::text("")),
                Box::new(Document::text(separator)),
            )
        } else {
            Document::text("")
        };

        Ok(Document::sequence([
            Document::text("{"),
            Document::sequence([
                edge.clone(),
                Document::join(
                    &Document::sequence([Document::text(separator), Document::Line]),
                    fields,
                ),
                suffix,
            ])
            .indent(),
            edge,
            Document::text("}"),
        ])
        .group())
    }
}
