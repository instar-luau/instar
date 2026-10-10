use std::{
    collections::{BTreeMap, BTreeSet},
    io,
    sync::Arc,
};

use instar_format::ValidatedFormat;
use instar_luau::SemanticCategory;

use instar_shared::{
    Completion, DiagnosticCategory, EditIdentity, EditIntent, EditProposal, Location, Options,
    Reason, SourceVersion, Span, TextEdit,
};

use super::request::{EditGuarantee, ValidatedEdit};

use crate::{
    capture::Capture,
    change::{ChangeBatch, ProjectChange, SourceChange},
    configuration::{CheckOverrides, DiagnosticPolicy, SelectionOverrides},
    execution::Runtime,
    inputs::{InputIdentity, InputVersion, ProjectSnapshot, SourceOrigin},
    report::CheckReport,
    request::{CheckRequest, Input, Invocation},
};

impl Runtime {
    pub(crate) fn validate_format(
        &mut self,
        format: &ValidatedFormat,
        snapshot: &ProjectSnapshot,
        inputs: &[InputVersion],
        options: &Options,
    ) -> io::Result<ValidatedEdit> {
        options.check()?;
        let source = &format.input().source;

        if snapshot.source(source.version.identity) != Some(source) {
            return Err(invalid(
                "formatter input differs from captured project snapshot",
            ));
        }

        let proposal = EditProposal {
            identity: EditIdentity::new(self.identity()),
            edits: vec![TextEdit {
                location: Location {
                    source: source.version,
                    span: Span {
                        start: 0,
                        end: source.text.len(),
                    },
                },
                replacement: format.output().to_owned(),
            }],
            guards: vec![source.version],
            intent: EditIntent::Formatting,
        };

        let proposed = overlay(&proposal, snapshot, options)?;

        Ok(ValidatedEdit {
            proposal,
            inputs: inputs.to_vec(),
            guarantees: vec![
                EditGuarantee::Ranges,
                EditGuarantee::Syntax,
                EditGuarantee::Preservation,
            ],
            proposed,
            original: snapshot.clone(),
        })
    }

    pub(crate) fn validate_edit(
        &mut self,
        proposal: EditProposal,
        snapshot: &ProjectSnapshot,
        inputs: &[InputVersion],
        guarantees: Vec<EditGuarantee>,
        context: &CheckRequest,
        options: &Options,
    ) -> io::Result<ValidatedEdit> {
        proposal.validate(&snapshot.sources().cloned().collect::<Vec<_>>())?;
        let mut validation_options = options.child();
        validation_options.observer = None;
        let options = &validation_options;

        let workspace = guarantees.iter().any(|guarantee| {
            matches!(
                guarantee,
                EditGuarantee::Bindings | EditGuarantee::Resolution
            )
        });

        let mut original = snapshot.clone();
        let mut guards = inputs.to_vec();

        if workspace {
            let categories = if guarantees
                .iter()
                .any(|guarantee| matches!(guarantee, EditGuarantee::Bindings))
            {
                vec![SemanticCategory::Bindings, SemanticCategory::Occurrences]
            } else {
                Vec::new()
            };

            let state = self.query_capture(
                Capture::new(Arc::clone(&self.host), original),
                proposal.guards.first().map(|source| source.identity),
                true,
                &categories,
                Some(context),
                options,
            )?;

            state.require_complete(options)?;
            original = state.capture.snapshot.clone();

            guards = consulted(
                &original,
                Some(snapshot),
                guards.into_iter().chain(state.capture.guards()),
            )?;
        }

        let invocation = Invocation {
            inputs: proposal
                .guards
                .iter()
                .map(|version| Input::Source(version.identity))
                .collect(),
            ..context.invocation.clone()
        };

        let request = validation_request(invocation, context);
        let mut before = self.check(&request, Some(original.clone()), options)?;

        before.inputs = consulted(
            &before.snapshot,
            Some(&original),
            guards.into_iter().chain(before.inputs.iter().cloned()),
        )?;

        let proposed = overlay(&proposal, &before.snapshot, options)?;

        self.validate_analysis(proposal, before, proposed, guarantees, &request, options)
    }

    pub(crate) fn validate_overlay(
        &mut self,
        proposal: EditProposal,
        original: &ProjectSnapshot,
        proposed: ProjectSnapshot,
        inputs: &[InputVersion],
        guarantees: Vec<EditGuarantee>,
        options: &Options,
    ) -> io::Result<ValidatedEdit> {
        proposal.validate(&original.sources().cloned().collect::<Vec<_>>())?;
        let mut validation_options = options.child();
        validation_options.observer = None;

        let invocation = Invocation {
            inputs: proposal
                .guards
                .iter()
                .filter(|source| proposed.source(source.identity).is_some())
                .map(|source| Input::Source(source.identity))
                .collect(),
            ..Invocation::default()
        };

        if invocation.inputs.is_empty() {
            if !proposal.edits.is_empty() {
                return Err(invalid("file proposal has no retained analysis sources"));
            }

            return Ok(ValidatedEdit {
                proposal,
                inputs: inputs.to_vec(),
                guarantees,
                proposed,
                original: original.clone(),
            });
        }

        let request = validation_request(invocation, &CheckRequest::default());

        let mut before = self.check(&request, Some(original.clone()), &validation_options)?;

        before.inputs = consulted(
            &before.snapshot,
            Some(original),
            inputs.iter().cloned().chain(before.inputs.iter().cloned()),
        )?;

        self.validate_analysis(
            proposal,
            before,
            proposed,
            guarantees,
            &request,
            &validation_options,
        )
    }

    fn validate_analysis(
        &mut self,
        proposal: EditProposal,
        before: CheckReport,
        proposed: ProjectSnapshot,
        guarantees: Vec<EditGuarantee>,
        request: &CheckRequest,
        options: &Options,
    ) -> io::Result<ValidatedEdit> {
        let expected = proposed.clone();
        let after = self.check(request, Some(proposed), options)?;

        if before.completion != Completion::Complete || after.completion != Completion::Complete {
            return Err(before.completion.combine(after.completion).error());
        }

        validate_diagnostics(&proposal, &before, &after)?;

        let binding_inputs = if guarantees
            .iter()
            .any(|guarantee| matches!(guarantee, EditGuarantee::Bindings))
        {
            self.validate_bindings(
                &proposal,
                &before.snapshot,
                &after.snapshot,
                true,
                request,
                options,
            )?
        } else {
            Vec::new()
        };

        if guarantees
            .iter()
            .any(|guarantee| matches!(guarantee, EditGuarantee::Resolution))
        {
            for edge in &before.dependencies {
                let (start, end) = relocated(
                    edge.site.argument.source,
                    edge.site.argument.span.start,
                    edge.site.argument.span.end,
                    &proposal.edits,
                )?;

                if !after.dependencies.iter().any(|updated| {
                    updated.importer == edge.importer
                        && updated.target == edge.target
                        && updated.site.argument.span.start == start
                        && updated.site.argument.span.end == end
                }) {
                    return Err(invalid("proposal changes a resolved import identity"));
                }
            }

            if after.dependencies.len() != before.dependencies.len() {
                return Err(invalid("proposal changes the captured import graph"));
            }
        }

        let guards = consulted(
            &before.snapshot,
            Some(&expected),
            before
                .inputs
                .iter()
                .cloned()
                .chain(after.inputs)
                .chain(binding_inputs),
        )?;

        if guarantees
            .iter()
            .any(|guarantee| matches!(guarantee, EditGuarantee::Effects))
        {
            return Err(Reason::UnavailableSemantics.error());
        }

        Ok(ValidatedEdit {
            proposal,
            inputs: guards,
            guarantees,
            proposed: after.snapshot,
            original: before.snapshot,
        })
    }
}

pub(crate) fn overlay(
    proposal: &EditProposal,
    snapshot: &ProjectSnapshot,
    options: &Options,
) -> io::Result<ProjectSnapshot> {
    proposal.validate(&snapshot.sources().cloned().collect::<Vec<_>>())?;
    let mut replacements = BTreeMap::new();
    let mut edits = proposal.edits.iter().collect::<Vec<_>>();
    edits.sort_by_key(|edit| (edit.location.source.identity, edit.location.span.start));

    for edit in edits.into_iter().rev() {
        options.check()?;

        let source = snapshot
            .source(edit.location.source.identity)
            .ok_or_else(|| invalid("proposal source is absent"))?;

        if snapshot.is_declaration(source.version)
            || matches!(
                snapshot.origin(source.version.identity),
                Some(SourceOrigin::Virtual { .. })
            )
        {
            return Err(invalid("proposal edits a read-only declaration"));
        }

        let replacement = replacements
            .entry(source.version.identity)
            .or_insert_with(|| source.text.to_string());

        replacement.replace_range(
            edit.location.span.start..edit.location.span.end,
            &edit.replacement,
        );
    }

    let mut changes = Vec::new();

    for (identity, text) in replacements {
        let expected = snapshot
            .source(identity)
            .ok_or_else(|| invalid("proposal source is absent"))?
            .version;

        changes.push(ProjectChange::Source(SourceChange::Replace {
            expected,
            text: text.into(),
        }));
    }

    snapshot.transition(ChangeBatch { changes })
}

fn validate_diagnostics(
    proposal: &EditProposal,
    before: &CheckReport,
    after: &CheckReport,
) -> io::Result<()> {
    if matches!(proposal.intent, EditIntent::Rename | EditIntent::Extraction)
        && !before.findings.is_empty()
    {
        return Err(invalid(
            "transformation requires error-free original analysis",
        ));
    }

    let allowed = before
        .findings
        .iter()
        .filter(|finding| finding.category != DiagnosticCategory::Lint)
        .filter_map(|finding| {
            let source = after.snapshot.source(finding.primary.source.identity)?;

            let (start, end) = relocated(
                finding.primary.source,
                finding.primary.span.start,
                finding.primary.span.end,
                &proposal.edits,
            )
            .ok()?;

            Some((
                &finding.code,
                &finding.message,
                finding.category,
                finding.severity,
                Location {
                    source: source.version,
                    span: Span { start, end },
                },
            ))
        })
        .collect::<BTreeSet<_>>();

    if after.findings.len() > before.findings.len()
        || after.findings.iter().any(|finding| {
            !allowed.contains(&(
                &finding.code,
                &finding.message,
                finding.category,
                finding.severity,
                finding.primary,
            ))
        })
    {
        return Err(invalid("proposal introduces a new analysis diagnostic"));
    }

    Ok(())
}

fn validation_request(invocation: Invocation, context: &CheckRequest) -> CheckRequest {
    CheckRequest {
        invocation,
        overrides: CheckOverrides {
            selection: SelectionOverrides {
                include: Some(Vec::new()),
                exclude: Some(Vec::new()),
                override_exclusions: true,
            },
            diagnostics: DiagnosticPolicy::default(),
            ..context.overrides.clone()
        },
    }
}

pub(crate) fn protected(
    original: &ProjectSnapshot,
    proposed: &ProjectSnapshot,
) -> BTreeSet<InputIdentity> {
    proposed
        .input_versions
        .iter()
        .filter(|(identity, input)| {
            matches!(
                identity,
                InputIdentity::Source(_)
                    | InputIdentity::Path(_)
                    | InputIdentity::Directory(_)
                    | InputIdentity::Sources
            ) && original.input_versions.get(*identity) != Some(*input)
        })
        .map(|(identity, _)| identity.clone())
        .collect()
}

pub(crate) fn consulted(
    snapshot: &ProjectSnapshot,
    proposed: Option<&ProjectSnapshot>,
    inputs: impl IntoIterator<Item = InputVersion>,
) -> io::Result<Vec<InputVersion>> {
    let mut guards = BTreeMap::new();

    for input in inputs {
        let expected = proposed.and_then(|proposed| proposed.input_versions.get(&input.identity));

        let Some(original) = snapshot.input_versions.get(&input.identity) else {
            if expected == Some(&input)
                && matches!(
                    input.identity,
                    InputIdentity::Source(_) | InputIdentity::Path(_) | InputIdentity::Directory(_)
                )
            {
                continue;
            }

            return Err(invalid(
                "validation consulted an input absent from the original capture",
            ));
        };

        if original != &input && expected != Some(&input) {
            return Err(invalid("validation changed an unguarded consulted input"));
        }

        guards.insert(original.identity.clone(), original.clone());
    }

    Ok(guards.into_values().collect())
}

pub(crate) fn relocated(
    version: SourceVersion,
    start: usize,
    end: usize,
    edits: &[TextEdit],
) -> io::Result<(usize, usize)> {
    let mut result = (start, end);

    for edit in edits.iter().filter(|edit| edit.location.source == version) {
        for (original, offset) in [(start, &mut result.0), (end, &mut result.1)] {
            if edit.location.span.start < original && original < edit.location.span.end {
                return Err(invalid("edit crosses a semantic boundary"));
            }

            if edit.location.span.end <= original {
                *offset = offset
                    .checked_add(edit.replacement.len())
                    .and_then(|offset| {
                        offset.checked_sub(edit.location.span.end - edit.location.span.start)
                    })
                    .ok_or_else(|| invalid("edit offset overflow"))?;
            }
        }
    }

    Ok(result)
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}
