use std::{io, sync::Arc};

use instar_shared::{
    Completion, EditIdentity, EditIntent, EditProposal, Options, Reason, SourceVersion,
};

use instar_syntax::identifier;

use super::request::{EditGuarantee, EditRequest, ValidatedEdit};

use crate::{
    capture::Capture, execution::Runtime, inputs::ProjectSnapshot,
    queries::service::checked_source, report::EditReport,
};

impl Runtime {
    pub(crate) fn edit(
        &mut self,
        request: EditRequest,
        options: &Options,
    ) -> io::Result<EditReport> {
        if let EditRequest::Rename(request) = &request
            && !identifier(&request.name)
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "rename requires one non-keyword Luau identifier",
            ));
        }

        let snapshot = match &request {
            EditRequest::Rename(request) => &request.snapshot,
            EditRequest::Fixes(request) => &request.snapshot,
            EditRequest::Imports(request) => &request.snapshot,
            EditRequest::Extract(request) => &request.snapshot,
            EditRequest::Linked(request) => &request.snapshot,
            EditRequest::Save(request) => &request.snapshot,
            EditRequest::Files(request) => &request.snapshot,
        }
        .clone();

        let mut report = EditReport {
            snapshots: vec![snapshot.clone()],
            inputs: snapshot.input_versions.values().cloned().collect(),
            snapshot,
            proposal: None,
            validated: None,
            linked_ranges: Vec::new(),
            unavailable_guarantees: Vec::new(),
            diagnostics: Vec::new(),
            completion: Completion::Complete,
        };

        let prepared = options.check().and_then(|()| match request {
            EditRequest::Rename(request) => self.rename(request, &mut report, options).map(Some),
            EditRequest::Fixes(request) => self.fixes(request, &mut report, options).map(Some),

            EditRequest::Imports(request) => {
                let snapshot = request.snapshot.clone();
                let source = request.source;

                self.organize(request, &mut report, options)
                    .and_then(|validated| match validated {
                        Some(validated) => Ok(Some(validated)),

                        None => self
                            .unchanged(snapshot, source, EditIntent::Imports, options)
                            .map(Some),
                    })
            }

            EditRequest::Extract(request) => self.extract(request, &mut report, options),

            EditRequest::Linked(request) => {
                self.linked(request, &mut report, options).map(|()| None)
            }

            EditRequest::Save(request) => self.save(request, &mut report, options),
            EditRequest::Files(request) => self.files(request, &mut report, options).map(Some),
        });

        match prepared {
            Ok(Some(validated)) => {
                report.snapshots.push(validated.original.clone());
                report.snapshots.push(validated.proposed.clone());
                report.snapshot = validated.original.clone();
                report.inputs.clone_from(&validated.inputs);
                report.proposal = Some(validated.proposal.clone());
                report.validated = Some(validated);
            }

            Ok(None) => {}

            Err(error) => {
                let completion = options
                    .interrupted()
                    .or_else(|| {
                        error
                            .get_ref()
                            .and_then(|cause| cause.downcast_ref::<Completion>())
                            .cloned()
                    })
                    .unwrap_or_else(|| {
                        let reason = error
                            .get_ref()
                            .and_then(|cause| cause.downcast_ref::<Reason>())
                            .copied()
                            .unwrap_or(if error.kind() == io::ErrorKind::Unsupported {
                                Reason::UnsupportedFeature
                            } else {
                                Reason::FailedPrerequisite
                            });

                        Completion::Incomplete(vec![reason])
                    });

                report.completion = report.completion.clone().combine(completion);
                report.unavailable_guarantees.push(error.to_string());
                report.validated = None;
                report.linked_ranges.clear();
            }
        }

        if let Some(completion) = options.interrupted() {
            report.completion = report.completion.clone().combine(completion);
            report.validated = None;
            report.linked_ranges.clear();
        }

        report.diagnostics.sort_by_key(|finding| {
            (
                finding.primary.source,
                finding.primary.span,
                finding.identity.clone(),
            )
        });

        report.diagnostics.dedup();
        report.snapshots.push(report.snapshot.clone());

        Ok(report)
    }

    fn unchanged(
        &mut self,
        snapshot: ProjectSnapshot,
        version: SourceVersion,
        intent: EditIntent,
        options: &Options,
    ) -> io::Result<ValidatedEdit> {
        let source = checked_source(&snapshot, version)?.clone();
        let mut capture = Capture::new(Arc::clone(&self.host), snapshot);
        self.editor_targets(Some(version.identity), false, &mut capture, None, options)?;
        capture.consult_source(&source);

        let proposal = EditProposal {
            identity: EditIdentity::new(self.identity()),
            edits: Vec::new(),
            guards: vec![version],
            intent,
        };

        proposal.validate(&capture.snapshot.sources().cloned().collect::<Vec<_>>())?;

        Ok(ValidatedEdit {
            proposal,
            inputs: capture.guards(),
            guarantees: vec![EditGuarantee::Ranges],
            proposed: capture.snapshot.clone(),
            original: capture.snapshot,
        })
    }
}
