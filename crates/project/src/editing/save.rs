use std::{io, sync::Arc, time::Duration};

use instar_format::{FormatInput, FormatOutcome, FormatScope, ValidatedFormat};

use instar_shared::{
    Completion, EditIdentity, EditIntent, EditProposal, Location, Options, SourceSnapshot,
    SourceVersion, Span, TextEdit,
};

use similar::{ChangeTag, TextDiff};

use super::request::{EditGuarantee, FixRequest, SaveRequest, ValidatedEdit};

use crate::{
    capture::Capture,
    configuration::worker_options,
    execution::{Runtime, Work, WorkOutput},
    graph::syntax_settings,
    inputs::ProjectSnapshot,
    queries::service::checked_source,
    report::EditReport,
    request::{CheckRequest, Input, Invocation, LintRequest},
};

impl Runtime {
    pub(crate) fn save(
        &mut self,
        request: SaveRequest,
        report: &mut EditReport,
        options: &Options,
    ) -> io::Result<Option<ValidatedEdit>> {
        let source = checked_source(&request.snapshot, request.source)?.clone();
        let mut capture = Capture::new(Arc::clone(&self.host), request.snapshot);

        let target = self
            .editor_targets(
                Some(source.version.identity),
                false,
                &mut capture,
                None,
                options,
            )?
            .pop()
            .ok_or_else(|| invalid("saving source is absent"))?;

        report.snapshot = capture.snapshot.clone();
        report.inputs = capture.guards();
        let policy = &target.effective.settings.editing;

        if !policy.fix_on_save && !policy.format_on_save {
            return Ok(None);
        }

        let fixed = if policy.fix_on_save {
            self.save_fixes(source.version, capture.snapshot, report, options)?
        } else {
            None
        };

        if !policy.format_on_save {
            return Ok(fixed);
        }

        let snapshot = fixed
            .as_ref()
            .map_or_else(|| report.snapshot.clone(), |fixed| fixed.proposed.clone());

        report.snapshots.push(snapshot.clone());

        let intermediate = snapshot
            .source(source.version.identity)
            .cloned()
            .ok_or_else(|| invalid("saving source is absent from the fix overlay"))?;

        let (format, formatting_capture) =
            self.save_format(&intermediate, snapshot, report, options)?;

        if format.output() == intermediate.text.as_ref() {
            return Ok(fixed);
        }

        let Some(fixed) = fixed else {
            return self
                .validate_format(
                    &format,
                    &formatting_capture.snapshot,
                    &formatting_capture.guards(),
                    options,
                )
                .map(Some);
        };

        let edits = difference(&source, format.output(), options)?;

        let proposal = EditProposal {
            identity: EditIdentity::new(self.identity()),
            edits,
            guards: vec![source.version],
            intent: EditIntent::Fix,
        };

        report.proposal = Some(proposal.clone());

        let mut guarantees = vec![
            EditGuarantee::Ranges,
            EditGuarantee::Syntax,
            EditGuarantee::Diagnostics,
            EditGuarantee::Resolution,
        ];

        if fixed
            .guarantees
            .iter()
            .any(|guarantee| matches!(guarantee, EditGuarantee::Bindings))
        {
            guarantees.push(EditGuarantee::Bindings);
        }

        let mut validated = self.validate_edit(
            proposal,
            &fixed.original,
            &fixed.inputs,
            guarantees,
            &CheckRequest::default(),
            options,
        )?;

        validated.guarantees.push(EditGuarantee::Preservation);

        Ok(Some(validated))
    }

    fn save_fixes(
        &mut self,
        source: SourceVersion,
        snapshot: ProjectSnapshot,
        report: &mut EditReport,
        options: &Options,
    ) -> io::Result<Option<ValidatedEdit>> {
        let invocation = Invocation {
            inputs: vec![Input::Source(source.identity)],
            ..Invocation::default()
        };

        let lint = self.lint(
            &LintRequest {
                invocation,
                ..LintRequest::default()
            },
            Some(snapshot),
            options,
        )?;

        report.snapshots.push(lint.snapshot.clone());
        report.snapshot = lint.snapshot.clone();
        report.inputs.clone_from(&lint.inputs);
        report.diagnostics.extend(lint.findings);
        report.completion = report.completion.clone().combine(lint.completion.clone());

        if lint.completion != Completion::Complete {
            return Err(lint.completion.error());
        }

        let fixes = lint
            .fixes
            .into_iter()
            .filter(|fix| {
                fix.proposal
                    .edits
                    .iter()
                    .all(|edit| edit.location.source == source)
            })
            .collect::<Vec<_>>();

        if fixes.is_empty() {
            return Ok(None);
        }

        self.fixes(
            FixRequest {
                snapshot: lint.snapshot,
                fixes,
            },
            report,
            options,
        )
        .map(Some)
    }

    fn save_format(
        &mut self,
        source: &SourceSnapshot,
        snapshot: ProjectSnapshot,
        report: &mut EditReport,
        options: &Options,
    ) -> io::Result<(ValidatedFormat, Capture)> {
        let mut capture = Capture::new(Arc::clone(&self.host), snapshot);

        let target = self
            .editor_targets(
                Some(source.version.identity),
                false,
                &mut capture,
                None,
                options,
            )?
            .pop()
            .ok_or_else(|| invalid("saving format source is absent"))?;

        report.snapshots.push(capture.snapshot.clone());

        let WorkOutput::Format(result) = self.workers.execute(
            Work::Format(FormatInput {
                source: source.clone(),
                settings: target.effective.settings.formatting.clone(),
                syntax: syntax_settings(&target.effective),
                scope: FormatScope::Whole,
            }),
            &worker_options(options),
        )?
        else {
            return Err(io::Error::other(
                "save formatting lane returned wrong outcome",
            ));
        };

        report.diagnostics.extend(result.diagnostics);
        report.completion = report.completion.clone().combine(result.completion.clone());

        if result.completion != Completion::Complete {
            return Err(result.completion.error());
        }

        let format = match result.outcome {
            FormatOutcome::Validated(format) => format,
            FormatOutcome::Unavailable { reason, .. } => return Err(reason.error()),
        };

        Ok((format, capture))
    }
}

fn difference(
    source: &SourceSnapshot,
    proposed: &str,
    options: &Options,
) -> io::Result<Vec<TextEdit>> {
    options.check()?;
    let mut configuration = TextDiff::configure();

    configuration.timeout(
        options
            .remaining()
            .unwrap_or(Duration::from_millis(50))
            .min(Duration::from_millis(50)),
    );

    let diff = configuration.diff_chars(source.text.as_ref(), proposed);
    let mut edits = Vec::new();
    let mut offset = 0;
    let mut pending: Option<(usize, String)> = None;

    for change in diff.iter_all_changes() {
        options.check()?;

        match change.tag() {
            ChangeTag::Equal => {
                if let Some((start, replacement)) = pending.take() {
                    edits.push(TextEdit {
                        location: Location::new(source, Span { start, end: offset })?,
                        replacement,
                    });
                }

                offset += change.value().len();
            }

            ChangeTag::Delete => {
                pending.get_or_insert_with(|| (offset, String::new()));
                offset += change.value().len();
            }

            ChangeTag::Insert => {
                pending
                    .get_or_insert_with(|| (offset, String::new()))
                    .1
                    .push_str(change.value());
            }
        }
    }

    if let Some((start, replacement)) = pending {
        edits.push(TextEdit {
            location: Location::new(source, Span { start, end: offset })?,
            replacement,
        });
    }

    Ok(edits)
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}
