use std::ops::Range;

use vermis::tree::Tree;

use super::document::Document;
use crate::format::{Error, directives::Directive};

#[derive(Clone)]
struct Protection<'regions> {
    regions: &'regions [Range<usize>],
    emitted: Vec<bool>,
    active: bool,
}

impl Protection<'_> {
    fn apply(&mut self, document: Vec<Document>) -> Vec<Document> {
        let mut result = Vec::with_capacity(document.len());

        for item in document {
            if let Document::Choice(choice) = &item
                && self.regions.iter().all(|region| {
                    region.end <= choice.range.start || choice.range.end <= region.start
                })
            {
                self.active = false;
                result.push(item);
                continue;
            }

            match item {
                Document::Token(index) => {
                    let region = self
                        .regions
                        .iter()
                        .position(|range| range.contains(&index.get()));

                    if let Some(region) = region {
                        let range = &self.regions[region];

                        if !self.emitted[region] {
                            result.push(Document::Verbatim(range.clone()));
                            self.emitted[region] = true;
                        }

                        self.active = index.get() + 1 < range.end;
                    } else {
                        self.active = false;
                        result.push(Document::Token(index));
                    }
                }

                Document::Indent {
                    conditional,
                    content,
                } => {
                    let content = self.apply(content);

                    if !content.is_empty() {
                        result.push(Document::Indent {
                            conditional,
                            content,
                        });
                    }
                }

                Document::Group { layout, content } => {
                    let content = self.apply(content);

                    if !content.is_empty() {
                        result.push(Document::Group { layout, content });
                    }
                }

                Document::Choice(mut choice) => {
                    let mut alternative = self.clone();
                    choice.inline = self.apply(choice.inline.to_vec()).into();
                    choice.expanded = alternative.apply(choice.expanded.to_vec()).into();
                    result.push(Document::Choice(choice));
                }

                other => {
                    if !self.active {
                        result.push(other);
                    }
                }
            }
        }

        result
    }
}

pub(super) fn apply(
    tree: &Tree<'_>,
    document: Vec<Document>,
    directives: &[&Directive],
) -> Result<Vec<Document>, Error> {
    if directives.is_empty() {
        return Ok(document);
    }

    let regions: Vec<_> = directives
        .iter()
        .map(|directive| {
            let start = tree
                .tokens
                .partition_point(|token| token.span.end <= directive.scope.start);

            let end = tree
                .tokens
                .partition_point(|token| token.span.start < directive.scope.end);

            start..end
        })
        .collect();

    let mut protection = Protection {
        regions: &regions,
        emitted: vec![false; regions.len()],
        active: false,
    };

    let document = protection.apply(document);

    for (directive, emitted) in directives.iter().zip(protection.emitted) {
        if !emitted {
            return Err(Error::internal(
                directive.scope.clone(),
                "cannot locate formatting-disabled source",
            ));
        }
    }

    Ok(document)
}
