//! Native checking over the shared project source and dependency graph.

use std::{collections::BTreeSet, io, time::Instant};

use instar_analysis::{self as analysis, Completion, Options, error::invalid, selection};

use crate::{
    analysis::{Entry, Origin, Preparation},
    native,
    project::{Project, absolute},
};

impl Project {
    /// Checks selected entries and reports dependency errors once per exact module.
    ///
    /// Instar patterns select entries only. Native configuration controls language
    /// modes and type error visibility; native lint warnings are never emitted.
    /// Roblox environments use automatically downloaded and verified assets.
    /// Native analysis is isolated in a cancellable worker process.
    /// Missing declaration files use revision zero and an empty source range.
    ///
    /// # Errors
    /// Returns source/configuration failures and invalid host/native source agreement.
    pub fn check(
        &mut self,
        entries: &[Entry],
        options: &Options,
    ) -> io::Result<analysis::Result<Origin>> {
        let started = Instant::now();

        let mut result = analysis::Result {
            modules: Vec::new(),
            diagnostics: Vec::new(),
            completion: Completion::Complete,
        };

        let mut roots = BTreeSet::new();
        let mut identities = BTreeSet::new();

        let preparation = self.during(
            |project| {
                for (completed, entry) in entries.iter().enumerate() {
                    if let Some(reason) = options.interrupted(started) {
                        result.completion = Completion::Incomplete(reason);
                        break;
                    }

                    let source = absolute(&entry.source)?;

                    options.report(
                        "Discovering dependencies",
                        completed,
                        Some(entries.len()),
                        Some(&source.to_string_lossy()),
                    );

                    let directory = source
                        .parent()
                        .ok_or_else(|| invalid("checker entry has no directory"))?;

                    let settings = project.configuration(directory)?;

                    if !selection::matches(
                        &source,
                        settings.configuration.check.include.as_deref(),
                        true,
                    )? || selection::matches(
                        &source,
                        settings.configuration.check.exclude.as_deref(),
                        false,
                    )? {
                        options.report(
                            "Discovering dependencies",
                            completed + 1,
                            Some(entries.len()),
                            Some(&source.to_string_lossy()),
                        );

                        continue;
                    }

                    let module =
                        project.install(&source, entry.context.as_ref(), &mut identities)?;

                    roots.insert(native::name(&module.identity));

                    options.report(
                        "Discovering dependencies",
                        completed + 1,
                        Some(entries.len()),
                        Some(&source.to_string_lossy()),
                    );
                }

                Ok(())
            },
            &options.remaining(started),
        );

        if let Err(error) = preparation {
            if let Some(reason) = options.interrupted(started) {
                result.completion = Completion::Incomplete(reason);
            } else {
                return Err(error);
            }
        }

        if roots.is_empty() {
            if let Some(reason) = options.interrupted(started) {
                result.completion = Completion::Incomplete(reason);
            }

            return Ok(result);
        }

        let mut analysis = Preparation::default();

        self.prepare_modules(identities, &mut analysis, &mut result, options, started)?;

        if result.completion != Completion::Complete {
            result.diagnostics = analysis.report.diagnostics;

            return Ok(result);
        }

        let frontend = self
            .frontend
            .as_mut()
            .ok_or_else(|| invalid("shared native frontend is unavailable"))?;

        result.completion =
            native::definitions(&analysis.environments, frontend, options, started)?;

        if result.completion != Completion::Complete {
            result.diagnostics = analysis.report.diagnostics;

            return Ok(result);
        }

        let remaining = native::progress(&result.modules, options).remaining(started);
        let checked = frontend.check(&roots.into_iter().collect::<Vec<_>>(), &remaining)?;
        result.completion = result.completion.combine(checked.completion);

        for diagnostic in checked.diagnostics {
            analysis.report.native(diagnostic)?;
        }

        result.diagnostics = analysis.report.diagnostics;

        if let Some(reason) = options.interrupted(started) {
            result.completion = Completion::Incomplete(reason);
        }

        Ok(result)
    }
}
