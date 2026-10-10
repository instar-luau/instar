use std::{io, time::Instant};

use instar_analysis::{Options, error::invalid};
use instar_bridge::frontend::Hover;
use vermis::tree::NodeKind;

use crate::{
    analysis::{Entry, Origin, locate},
    project::Project,
};

impl Project {
    /// Returns inferred types for unannotated bindings in an exact module context.
    ///
    /// # Errors
    /// Returns invalid ranges, unavailable environments, interruption, or source agreement failures.
    pub fn hints(
        &mut self,
        entry: &Entry,
        range: [usize; 2],
        options: &Options,
    ) -> io::Result<Vec<Hover<Origin>>> {
        let started = Instant::now();

        self.during(
            |project| {
                let source = project.source(&entry.source)?;

                if source.text.get(range[0]..range[1]).is_none() {
                    return Err(invalid("invalid inlay hint range"));
                }

                let tree = vermis::parse(source.text.as_bytes());
                let (name, preparation) = project.semantic(entry, options, started)?;

                let frontend = project
                    .frontend
                    .as_mut()
                    .ok_or_else(|| invalid("shared native frontend is unavailable"))?;

                let mut result = Vec::new();

                for node in &tree.nodes {
                    options.check(started)?;

                    let NodeKind::Binding {
                        name: binding,
                        colon: None,
                        annotation: None,
                    } = node.kind
                    else {
                        continue;
                    };

                    let span = tree.node(binding).span;

                    if span.start == span.end || span.end < range[0] || span.end > range[1] {
                        continue;
                    }

                    let Some(hover) =
                        frontend.hover(&name, span.start, &options.remaining(started))?
                    else {
                        continue;
                    };

                    let location = locate(&hover.location, &preparation.report.sources)?;

                    if location.revision != source.revision
                        || location.range != [span.start, span.end]
                    {
                        return Err(invalid("inlay hint source revision mismatch"));
                    }

                    result.push(Hover {
                        location,
                        text: hover.text,
                        documentation: hover.documentation,
                        deprecated: hover.deprecated,
                    });
                }

                options.check(started)?;
                result.sort_by_key(|hint| hint.location.range);

                Ok(result)
            },
            options,
        )
    }
}
