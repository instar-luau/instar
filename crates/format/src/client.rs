use std::{io, sync::Arc};

use instar_shared::{
    Completion, Options, Reason, SourceSnapshot,
    error::invalid,
    process::{Outcome, Process},
};

use instar_syntax::{PreservationResult, Tree, parse, preserve_mapped};

use crate::{
    FormatInput, FormatOutcome, FormatResult, FormatSelection, SelectionOutcome, ValidatedFormat,
    settings::Order,
    worker::protocol::{Prepared, Request, Response, WireOutcome, prepare, selection_outcomes},
};

/// A lazy isolated syntax-only formatter with host-validated proposals.
#[derive(Default)]
pub struct Formatter {
    process: Process<Request, Response>,
    request: u64,
}

impl Formatter {
    /// Formats exact immutable inputs without exposing partial applicable output.
    ///
    /// # Errors
    /// Returns invalid input, startup, transport, cleanup or contradictory worker responses.
    pub fn format(&mut self, input: &FormatInput, options: &Options) -> io::Result<FormatResult> {
        input.validate()?;

        if let Some(completion) = options.interrupted() {
            self.process.discard()?;

            return FormatResult::interrupted(input, completion);
        }

        self.request = self
            .request
            .checked_add(1)
            .ok_or_else(|| io::Error::other("format request identity exhausted"))?;

        let request = Request {
            identity: self.request,
            input: input.clone(),
            remaining_budget: options.remaining(),
        };

        let response = match self.process.request("format", request, options)? {
            Outcome::Interrupted(completion) => {
                return FormatResult::interrupted(input, completion);
            }

            Outcome::Response(response) => response,
        };

        if let Some(completion) = options.interrupted() {
            self.process.discard()?;

            return FormatResult::interrupted(input, completion);
        }

        let result = self.reconstruct(input, response, options);

        if let Err(error) = result {
            self.process.discard()?;

            return Err(error);
        }

        let result = result?;

        if matches!(
            result.completion,
            Completion::Cancelled | Completion::TimedOut
        ) {
            self.process.discard()?;
        }

        Ok(result)
    }

    /// Stops acceptance and terminates and reaps this formatter's worker.
    ///
    /// # Errors
    /// Returns subprocess cleanup failures.
    pub fn finish(&mut self) -> io::Result<()> {
        self.process.finish()
    }

    fn reconstruct(
        &self,
        input: &FormatInput,
        response: Response,
        options: &Options,
    ) -> io::Result<FormatResult> {
        if response.identity != self.request
            || Some(response.generation) != self.process.generation()
            || &response.input != input
        {
            return Err(invalid(
                "format response does not identify its exact request inputs and generation",
            ));
        }

        for diagnostic in &response.diagnostics {
            diagnostic.primary.validate(&input.source)?;

            if diagnostic.identity.location() != diagnostic.primary
                || diagnostic.identity.code() != diagnostic.code
                || diagnostic.identity.inputs() != [input.source.version]
            {
                return Err(invalid(
                    "format diagnostic identity does not agree with its exact inputs and location",
                ));
            }

            for related in &diagnostic.related {
                related.location.validate(&input.source)?;
            }
        }

        let original = parse(&input.source, &input.syntax, options);
        let prepared = prepare(&original.tree, input, options)?;

        if let Some(completion) = options.interrupted() {
            return FormatResult::interrupted(input, completion);
        }

        match &response.outcome {
            WireOutcome::Validated { .. } => {
                Self::validated(input, &original.tree, &prepared, response, options)
            }

            WireOutcome::Unavailable { .. } => {
                Self::unavailable(input, &original.completion, &prepared, response)
            }
        }
    }

    fn coverage(
        input: &FormatInput,
        prepared: &Prepared,
        completion: &Completion,
        selections: &[FormatSelection],
        preservation: &PreservationResult,
    ) -> io::Result<()> {
        if completion != &Completion::Complete
            || prepared.reason.is_some()
            || prepared.completion != Completion::Complete
            || !preservation.preserved()
        {
            return Err(invalid(
                "applicable format response lacks complete required coverage",
            ));
        }

        if selections.len() != prepared.selections.len()
            || selections
                .iter()
                .zip(&prepared.selections)
                .any(|(actual, expected)| {
                    actual.requested != expected.requested
                        || actual.expanded != expected.expanded
                        || matches!(actual.outcome, SelectionOutcome::Unavailable(_))
                })
        {
            return Err(invalid(
                "format response selections do not cover the exact request",
            ));
        }

        if preservation.selections != prepared.regions
            || preservation.output_ranges.len() != prepared.regions.len()
            || preservation.import_ordering != (input.settings.requires.order != Order::Preserve)
        {
            return Err(invalid(
                "format response preservation ranges or settings disagree",
            ));
        }

        Ok(())
    }

    fn validated(
        input: &FormatInput,
        original: &Tree<'_>,
        prepared: &Prepared,
        response: Response,
        options: &Options,
    ) -> io::Result<FormatResult> {
        let WireOutcome::Validated {
            output,
            selections,
            preservation,
        } = response.outcome
        else {
            return Err(invalid("expected applicable format response"));
        };

        Self::coverage(
            input,
            prepared,
            &response.completion,
            &selections,
            &preservation,
        )?;

        let proposed = SourceSnapshot::new(input.source.version, Arc::clone(&output))?;

        if proposed.encoding != input.source.encoding {
            return Err(invalid(
                "format response changed the source encoding prefix",
            ));
        }

        for span in &preservation.output_ranges {
            span.validate(&proposed)?;
        }

        let expected = selection_outcomes(
            &prepared.selections,
            &prepared.regions,
            &preservation.output_ranges,
            &input.source.text,
            &output,
        );

        if expected != selections {
            return Err(invalid(
                "format response selection outcomes contradict its replacement bytes",
            ));
        }

        let parsed = parse(&proposed, &input.syntax, options);

        let checked = preserve_mapped(
            original,
            &parsed.tree,
            &prepared.regions,
            &preservation.output_ranges,
            preservation.import_ordering,
            options,
        );

        if let Some(completion) = options.interrupted() {
            return FormatResult::interrupted(input, completion);
        }

        if checked != preservation || !checked.preserved() {
            return Err(invalid(
                "format response preservation evidence could not be reconstructed",
            ));
        }

        Ok(FormatResult {
            source: input.source.version,
            outcome: FormatOutcome::Validated(ValidatedFormat::new(
                input.clone(),
                output,
                selections,
                checked,
            )),
            diagnostics: response.diagnostics,
            completion: Completion::Complete,
        })
    }

    fn unavailable(
        input: &FormatInput,
        original: &Completion,
        prepared: &Prepared,
        response: Response,
    ) -> io::Result<FormatResult> {
        let WireOutcome::Unavailable {
            reason,
            selections,
            preservation,
        } = response.outcome
        else {
            return Err(invalid("expected unavailable format response"));
        };

        if response.completion == Completion::Complete
            && selections.len() != prepared.selections.len()
        {
            return Err(invalid(
                "unavailable format response omits requested selections",
            ));
        }

        for (actual, expected) in selections.iter().zip(&prepared.selections) {
            if actual.requested != expected.requested
                || actual.expanded != expected.expanded
                || actual.outcome != SelectionOutcome::Unavailable(reason)
            {
                return Err(invalid(
                    "unavailable format response selections disagree with the input or failure reason",
                ));
            }
        }

        if selections.len() > prepared.selections.len()
            || preservation.as_ref().is_some_and(|preservation| {
                preservation.preserved() || preservation.selections != prepared.regions
            })
        {
            return Err(invalid(
                "unavailable format response contains contradictory applicable evidence",
            ));
        }

        if let Completion::Incomplete(reasons) = &response.completion
            && reasons.is_empty()
        {
            return Err(invalid(
                "format response has incomplete work without a reason",
            ));
        }

        if original != &Completion::Complete
            && response.completion == Completion::Complete
            && reason != Reason::UnsupportedFeature
        {
            return Err(invalid(
                "format response completed an unsupported syntax profile",
            ));
        }

        Ok(FormatResult::unavailable(
            input,
            reason,
            selections,
            preservation,
            response.diagnostics,
            response.completion,
        ))
    }
}
