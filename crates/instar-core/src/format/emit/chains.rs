use super::{Document, Emitter, Parts, View, io};
use crate::configuration::format::Chain;

impl<'tree, 'source> Emitter<'tree, 'source> {
    pub(super) fn chain(
        &self,
        view: View<'tree, 'source>,
    ) -> io::Result<Option<Document<'source>>> {
        let options = &self.options.calls.chains;

        if options.style == Chain::Preserve {
            return Ok(None);
        }

        let mut current = view;
        let mut steps = Vec::new();

        loop {
            match current.parts() {
                Some(Parts::MethodCall {
                    receiver,
                    method,
                    types,
                    arguments,
                }) => {
                    steps.push(Document::sequence([
                        Document::text(":"),
                        self.node(method)?,
                        Document::text(if types.is_some() { "<" } else { "" }),
                        self.optional(types)?,
                        Document::text(if types.is_some() { ">" } else { "" }),
                        self.node(arguments)?,
                    ]));

                    current = receiver;
                }

                Some(Parts::Call { callee, arguments }) => {
                    let (callee, types) = match callee.parts() {
                        Some(Parts::Instantiate {
                            expression,
                            arguments,
                        }) => (expression, Some(arguments)),

                        _ => (callee, None),
                    };

                    let Some(Parts::Field { receiver, name }) = callee.parts() else {
                        break;
                    };

                    steps.push(Document::sequence([
                        Document::text("."),
                        self.node(name)?,
                        Document::text(if types.is_some() { "<" } else { "" }),
                        self.optional(types)?,
                        Document::text(if types.is_some() { ">" } else { "" }),
                        self.node(arguments)?,
                    ]));

                    current = receiver;
                }

                _ => break,
            }
        }

        if steps.len() < 2 {
            return Ok(None);
        }

        let forced = options.minimum_calls > 0 && steps.len() >= options.minimum_calls;
        let mut head = vec![self.node(current)?];
        let mut tail = Vec::new();

        for (index, step) in steps.into_iter().rev().enumerate() {
            if index == 0 && options.style == Chain::Method {
                head.push(step);
            } else {
                tail.extend([
                    if forced {
                        Document::Hard
                    } else {
                        Document::Soft
                    },
                    step,
                ]);
            }
        }

        head.push(Document::sequence(tail).indent());

        Ok(Some(Document::sequence(head).group()))
    }
}
