//! Revision-anchored editor transformations using shared analysis results.

use std::{io, path::PathBuf, slice, time::Instant};

use instar_analysis::{Completion, Edit, Location, Options, error::invalid};
use instar_lint::{self as lint, Diagnostic};

use crate::{
    analysis::{Entry, Origin},
    project::Project,
};

/// Purpose of an editor transformation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// One diagnostic correction.
    QuickFix,

    /// All compatible safe diagnostic corrections in one source.
    FixAll,

    /// Import organization preserving execution order.
    OrganizeImports,

    /// Extraction of a selected value into a local binding.
    Extract,
}

/// One checked transformation of an immutable source snapshot.
pub struct Action {
    /// User-facing action label.
    pub title: String,

    /// Operation category.
    pub kind: Kind,

    /// Current diagnostics addressed by the action.
    pub diagnostics: Vec<Diagnostic<Origin>>,

    /// Nonoverlapping revision-anchored replacements.
    pub edits: Vec<Edit<PathBuf>>,
}

impl Project {
    /// Plans requested diagnostic corrections and checked source transformations.
    ///
    /// # Errors
    /// Returns invalid ranges, incomplete analysis, source, environment, or interruption failures.
    pub fn actions(
        &mut self,
        entry: &Entry,
        range: [usize; 2],
        kinds: &[Kind],
        options: &Options,
    ) -> io::Result<Vec<Action>> {
        let started = Instant::now();

        self.during(
            |project| {
                let source = project.source(&entry.source)?;

                if source.text.get(range[0]..range[1]).is_none() {
                    return Err(invalid("invalid code action range"));
                }

                let mut actions = if kinds
                    .iter()
                    .any(|kind| matches!(kind, Kind::QuickFix | Kind::FixAll))
                {
                    project.corrections(entry, range, kinds, options, started)?
                } else {
                    Vec::new()
                };

                if kinds.contains(&Kind::OrganizeImports) {
                    let edits = project.organize(entry, options, started)?;

                    if !edits.is_empty() {
                        actions.push(Action {
                            title: "Organize imports without reordering execution".to_owned(),
                            kind: Kind::OrganizeImports,
                            diagnostics: Vec::new(),
                            edits,
                        });
                    }
                }

                if kinds.contains(&Kind::Extract) {
                    let edits = project.extraction(entry, range, options, started)?;

                    if !edits.is_empty() {
                        actions.push(Action {
                            title: "Extract literal into a local binding".to_owned(),
                            kind: Kind::Extract,
                            diagnostics: Vec::new(),
                            edits,
                        });
                    }
                }

                options.check(started)?;

                Ok(actions)
            },
            options,
        )
    }

    fn corrections(
        &mut self,
        entry: &Entry,
        range: [usize; 2],
        kinds: &[Kind],
        options: &Options,
        started: Instant,
    ) -> io::Result<Vec<Action>> {
        let source = self.source(&entry.source)?;
        let linted = self.lint(slice::from_ref(entry), &options.remaining(started))?;

        if let Completion::Incomplete(reason) = linted.completion {
            return Err(reason.error());
        }

        let mut actions = Vec::new();
        let mut combined = Vec::new();
        let mut diagnostics = Vec::new();

        for diagnostic in linted.diagnostics {
            options.check(started)?;

            if diagnostic.location.module.source() != entry.source {
                continue;
            }

            let replacements = lint::fixes(
                &diagnostic,
                &source.text,
                source.revision,
                &options.remaining(started),
            )?;

            let edits = replacements
                .into_iter()
                .map(|edit| Edit {
                    location: Location {
                        module: edit.location.module.source().to_path_buf(),
                        revision: edit.location.revision,
                        range: edit.location.range,
                    },
                    text: edit.text,
                })
                .collect::<Vec<_>>();

            if edits.is_empty() {
                continue;
            }

            combined.extend(edits.iter().cloned());
            diagnostics.push(diagnostic.clone());

            if kinds.contains(&Kind::QuickFix)
                && diagnostic.location.range[0] <= range[1]
                && range[0] <= diagnostic.location.range[1]
            {
                actions.push(Action {
                    title: format!("Fix: {}", diagnostic.message),
                    kind: Kind::QuickFix,
                    diagnostics: vec![diagnostic],
                    edits,
                });
            }
        }

        combined.sort_by_key(|edit| edit.location.range);
        combined.dedup();

        if kinds.contains(&Kind::FixAll)
            && !combined.is_empty()
            && combined
                .windows(2)
                .all(|pair| pair[0].location.range[1] <= pair[1].location.range[0])
        {
            actions.push(Action {
                title: "Fix all safe Instar findings".to_owned(),
                kind: Kind::FixAll,
                diagnostics,
                edits: combined,
            });
        }

        Ok(actions)
    }
}
