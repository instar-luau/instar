//! Contextual completion entries with revision-anchored supporting edits.

use std::{io, path::PathBuf, time::Instant};

use instar_analysis::{Edit, Location, Options, error::invalid};
use instar_bridge::frontend::{Suggestion, SuggestionContext};

use crate::{
    analysis::{Entry, Origin, locate},
    project::Project,
};

/// One completion entry and its explicitly requested supporting edits.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Candidate {
    /// Detached completion presentation and insertion information.
    pub entry: Suggestion,

    /// Nonoverlapping source edits to apply with the completion.
    pub edits: Vec<Edit<Origin>>,
}

/// Completion entries in one exact module context and source revision.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Suggestions {
    /// Source range to replace.
    pub location: Location<Origin>,

    /// Native syntactic context.
    pub context: SuggestionContext,

    /// Preceding member operator byte range when available.
    pub index: Option<[usize; 2]>,

    /// Whether a string argument belongs to the native require function.
    pub require: bool,

    /// Whether the entries expose a canonical require binding.
    pub loader: bool,

    /// Decoded string contents before the cursor, when available.
    pub prefix: Option<String>,

    /// Entries in deterministic name and detail order.
    pub entries: Vec<Candidate>,
}

impl Project {
    /// Returns completion entries in an exact module context.
    ///
    /// Nonempty roots enable automatic import discovery within their editor indexing selection.
    ///
    /// # Errors
    /// Returns ambiguous contexts, invalid positions, unavailable environments,
    /// filesystem or indexing failures, interruption, or native source agreement failures.
    pub fn suggest(
        &mut self,
        entry: &Entry,
        offset: usize,
        roots: &[PathBuf],
        options: &Options,
    ) -> io::Result<Suggestions> {
        let started = Instant::now();

        self.during(
            |project| {
                let (name, preparation) = project.semantic(entry, options, started)?;

                let frontend = project
                    .frontend
                    .as_mut()
                    .ok_or_else(|| invalid("shared native frontend is unavailable"))?;

                let mut suggestions =
                    frontend.suggest(&name, offset, &options.remaining(started))?;

                options.check(started)?;

                if suggestions.require
                    && let Some(prefix) = &suggestions.prefix
                {
                    let module = project.module(&entry.source, entry.context.as_ref())?;

                    suggestions
                        .entries
                        .extend(project.imports(&module, prefix)?);

                    suggestions
                        .entries
                        .sort_by(|left, right| left.name.cmp(&right.name));

                    suggestions
                        .entries
                        .dedup_by(|left, right| left.name == right.name);
                }

                let mut suggestions = Suggestions {
                    location: locate(&suggestions.location, &preparation.report.sources)?,
                    context: suggestions.context,
                    index: suggestions.index,
                    require: suggestions.require,
                    loader: suggestions.loader,
                    prefix: suggestions.prefix,
                    entries: suggestions
                        .entries
                        .into_iter()
                        .map(|entry| Candidate {
                            entry,
                            edits: Vec::new(),
                        })
                        .collect(),
                };

                project.automatic_imports(
                    entry,
                    offset,
                    roots,
                    &mut suggestions,
                    options,
                    started,
                )?;

                suggestions.entries.sort_by(|left, right| {
                    left.entry
                        .name
                        .cmp(&right.entry.name)
                        .then_with(|| left.entry.detail.cmp(&right.entry.detail))
                });

                options.check(started)?;

                Ok(suggestions)
            },
            options,
        )
    }
}
