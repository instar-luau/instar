use std::{collections::BTreeSet, io};

use instar_lint::{FixSupport, FixValidation, RuleFix, collect_fixes, rules};
use instar_shared::{Completion, EditIdentity, EditIntent, EditProposal, Options};

use super::request::{EditGuarantee, FixRequest, ValidatedEdit};

use crate::{
    execution::Runtime,
    report::EditReport,
    request::{CheckRequest, Input, Invocation, LintRequest},
};

impl Runtime {
    pub(crate) fn fixes(
        &mut self,
        request: FixRequest,
        report: &mut EditReport,
        options: &Options,
    ) -> io::Result<ValidatedEdit> {
        if request.fixes.is_empty() {
            return Err(invalid("fix request contains no catalog proposals"));
        }

        let selected = request
            .fixes
            .iter()
            .map(|fix| fix.rule)
            .collect::<BTreeSet<_>>();

        let sources = request
            .fixes
            .iter()
            .flat_map(|fix| fix.proposal.guards.iter().map(|source| source.identity))
            .collect::<BTreeSet<_>>();

        let invocation = Invocation {
            inputs: sources.into_iter().map(Input::Source).collect(),
            ..Invocation::default()
        };

        let current = self.lint(
            &LintRequest {
                invocation,
                ..LintRequest::default()
            },
            Some(request.snapshot),
            options,
        )?;

        report.snapshot = current.snapshot.clone();
        report.inputs.clone_from(&current.inputs);
        report.diagnostics.extend(current.findings.clone());

        report.completion = report
            .completion
            .clone()
            .combine(current.completion.clone());

        if current.completion != Completion::Complete {
            return Err(current.completion.error());
        }

        let fixes = collect_fixes(
            &request.fixes,
            &selected.into_iter().collect::<Vec<_>>(),
            &current.snapshot.sources().cloned().collect::<Vec<_>>(),
            options,
        )?;

        for fix in &fixes {
            options.check()?;
            catalog(fix)?;

            if !current.fixes.iter().any(|candidate| {
                candidate.rule == fix.rule
                    && candidate.validation == fix.validation
                    && candidate.proposal.edits == fix.proposal.edits
                    && candidate.proposal.guards == fix.proposal.guards
            }) {
                return Err(invalid(
                    "fix proposal no longer matches its catalog producer",
                ));
            }
        }

        let proposal = EditProposal {
            identity: EditIdentity::new(self.identity()),
            edits: fixes
                .iter()
                .flat_map(|fix| fix.proposal.edits.clone())
                .collect(),
            guards: fixes
                .iter()
                .flat_map(|fix| fix.proposal.guards.iter().copied())
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect(),
            intent: EditIntent::Fix,
        };

        report.proposal = Some(proposal.clone());

        let mut guarantees = vec![
            EditGuarantee::Ranges,
            EditGuarantee::Syntax,
            EditGuarantee::Diagnostics,
            EditGuarantee::Resolution,
        ];

        if fixes
            .iter()
            .any(|fix| matches!(fix.validation, FixValidation::Bindings))
        {
            guarantees.push(EditGuarantee::Bindings);
        }

        self.validate_edit(
            proposal,
            &current.snapshot,
            &current.inputs,
            guarantees,
            &CheckRequest::default(),
            options,
        )
    }
}

fn catalog(fix: &RuleFix) -> io::Result<()> {
    let rule = rules()
        .iter()
        .find(|rule| rule.identity == fix.rule)
        .ok_or_else(|| invalid("fix rule is absent from the catalog"))?;

    if !matches!(
        (rule.fixes, fix.validation),
        (FixSupport::Syntax, FixValidation::Syntax)
            | (FixSupport::Bindings, FixValidation::Bindings)
    ) {
        return Err(invalid("fix validation differs from its catalog contract"));
    }

    Ok(())
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}
