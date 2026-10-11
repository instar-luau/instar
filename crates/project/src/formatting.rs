use std::{
    io,
    sync::{Arc, PoisonError},
};

use instar_format::{FormatInput, FormatOutcome, FormatResult};
use instar_shared::{Completion, Event, EventPayload, Options, SourceOutcome, SourceVersion};

use crate::{
    capture::Capture,
    configuration::{OperationOverrides, worker_options},
    editing::{ApplyRequest, ValidatedEdit, WriteAuthorization, WriteOutcome, preview},
    execution::{Runtime, Work, WorkOutput},
    graph::syntax_settings,
    inputs::SourceOrigin,
    report::{Counts, FormatReport, FormattedSource},
    request::{FormatMode, FormatRequest, Input},
    selection::SelectionPurpose,
};

impl Runtime {
    pub(crate) fn format(
        &mut self,
        request: &FormatRequest,
        options: &Options,
    ) -> io::Result<FormatReport> {
        let mut capture = request.snapshot.as_ref().map_or_else(
            || Capture::live(Arc::clone(&self.host)),
            |snapshot| Capture::new(Arc::clone(&self.host), snapshot.clone()),
        );

        let selection = self.select(
            &request.invocation,
            &OperationOverrides::Format(request.overrides.clone()),
            &mut capture,
            SelectionPurpose::Command,
            options,
        )?;

        let excluded_stdin = selection.targets.is_empty()
            && selection.candidates == 1
            && matches!(request.invocation.inputs.as_slice(), [Input::Stdin { .. }]);

        if matches!(request.mode, FormatMode::Source)
            && selection.targets.len() != 1
            && !excluded_stdin
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "source output requires exactly one selected source",
            ));
        }

        let inputs = capture.guards();
        let mut work = Vec::new();
        let mut prepared = Vec::new();

        for target in &selection.targets {
            let source = capture
                .snapshot
                .source(target.identity)
                .cloned()
                .ok_or_else(|| io::Error::other("selected format source is absent"))?;

            let input = FormatInput {
                source,
                settings: target.effective.settings.formatting.clone(),
                syntax: syntax_settings(&target.effective),
                scope: request.scope.clone(),
            };
            let cached = self
                .host
                .formats
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .get(&input.source.version.identity)
                .filter(|(previous, _)| previous == &input)
                .map(|(_, result)| result.clone());
            if cached.is_none() {
                work.push(Work::Format(input.clone()));
            }
            prepared.push((input, cached));
        }

        let mut outcomes = self
            .workers
            .batch(work, &worker_options(options))
            .into_iter();

        let mut report = FormatReport {
            snapshot: capture.snapshot.clone(),
            inputs,
            results: Vec::new(),
            selection: selection.decisions,
            settings: selection
                .targets
                .iter()
                .map(|target| target.effective.clone())
                .collect(),
            counts: Counts {
                selected: selection.targets.len(),
                excluded: selection.candidates.saturating_sub(selection.targets.len()),
                ..Counts::default()
            },
            diagnostics: Vec::new(),
            completion: selection.completion,
            failed: false,
        };

        for (input, cached) in prepared {
            let result = if let Some(result) = cached {
                result
            } else {
                match outcomes
                    .next()
                    .ok_or_else(|| io::Error::other("format work outcome is absent"))?
                {
                    Ok(WorkOutput::Format(result)) => result,
                    Ok(_) => return Err(io::Error::other("format lane returned wrong outcome")),
                    Err(error) => {
                        if let Some(completion) = options.interrupted() {
                            report.completion = report.completion.combine(completion);
                            break;
                        }
                        return Err(error);
                    }
                }
            };
            if result.completion == Completion::Complete {
                self.host
                    .formats
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .insert(input.source.version.identity, (input, result.clone()));
            }
            self.format_result(request, result, &mut report, options)?;
        }

        if let Some(completion) = options.interrupted() {
            report.completion = report.completion.combine(completion);
        }

        Ok(report)
    }

    fn format_result(
        &self,
        request: &FormatRequest,
        result: FormatResult,
        report: &mut FormatReport,
        options: &Options,
    ) -> io::Result<()> {
        report.completion = report.completion.clone().combine(result.completion.clone());
        report.diagnostics.extend(result.diagnostics.clone());
        let mut proposal = None;
        let mut patch = None;
        let mut write = WriteOutcome::NotRequested;

        match &result.outcome {
            FormatOutcome::Validated(format) => {
                let changed = format.input().source.text.as_ref() != format.output();

                if changed {
                    report.counts.changed += 1;
                    report.failed |= matches!(request.mode, FormatMode::Check);

                    let validated =
                        self.validate_format(format, &report.snapshot, &report.inputs, options)?;

                    if matches!(request.mode, FormatMode::Preview) {
                        patch = Some(preview(
                            &format.input().source.text,
                            format.output(),
                            &report
                                .snapshot
                                .origin(result.source.identity)
                                .and_then(SourceOrigin::path)
                                .map_or_else(
                                    || "stdin".to_owned(),
                                    |path| path.display().to_string(),
                                ),
                            options,
                        )?);
                    }

                    if matches!(request.mode, FormatMode::Write) {
                        write = self.apply_format(&validated, result.source, report, options)?;
                    }

                    proposal = Some(validated);
                } else {
                    report.counts.unchanged += 1;
                    write = WriteOutcome::Unchanged;
                }
            }

            FormatOutcome::Unavailable { .. } => {
                report.counts.unsuccessful += 1;
                report.failed = true;
            }
        }

        for diagnostic in &result.diagnostics {
            options.report(Event {
                invocation: 0,
                operation: 0,
                payload: EventPayload::Diagnostic(diagnostic.clone()),
            });
        }

        options.report(Event {
            invocation: 0,
            operation: 0,
            payload: EventPayload::SourceOutcome(SourceOutcome {
                source: result.source,
                completion: result.completion.clone(),
            }),
        });

        report.results.push(FormattedSource {
            result,
            proposal,
            preview: patch,
            write,
        });

        Ok(())
    }

    fn apply_format(
        &self,
        validated: &ValidatedEdit,
        source: SourceVersion,
        report: &mut FormatReport,
        options: &Options,
    ) -> io::Result<WriteOutcome> {
        let destinations = report
            .snapshot
            .origin(source.identity)
            .and_then(|origin| {
                if matches!(
                    origin,
                    SourceOrigin::Stdin { .. } | SourceOrigin::Virtual { .. }
                ) {
                    None
                } else {
                    origin.path()
                }
            })
            .map_or_default(|path| {
                vec![WriteAuthorization {
                    source,
                    destination: path.to_path_buf(),
                }]
            });

        let applied = self.apply(
            ApplyRequest {
                edit: validated.clone(),
                destinations,
            },
            options,
        )?;

        report.completion = report.completion.clone().combine(applied.completion);

        let write = applied
            .writes
            .into_iter()
            .next()
            .unwrap_or(WriteOutcome::NotRequested);

        if matches!(write, WriteOutcome::Written { .. }) {
            report.counts.written += 1;

            for input in &mut report.inputs {
                if let Some(current) = applied.snapshot.input_versions.get(&input.identity) {
                    *input = current.clone();
                }
            }

            report.snapshot = applied.snapshot;
        }

        if matches!(
            write,
            WriteOutcome::Failed { .. } | WriteOutcome::Unapplied { .. }
        ) {
            report.counts.unsuccessful += 1;
        }

        Ok(write)
    }
}
