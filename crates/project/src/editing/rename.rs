use std::{collections::BTreeSet, io, sync::Arc};

use instar_luau::{SemanticCategory, SemanticIdentity};
use instar_shared::{EditIdentity, EditIntent, EditProposal, Knowledge, Options, Reason, TextEdit};
use instar_syntax::{identifier, literal};

use super::request::{EditGuarantee, RenameRequest, ValidatedEdit};

use crate::{
    capture::Capture,
    execution::Runtime,
    inputs::SourceOrigin,
    queries::{ReferenceQuery, navigation, service::checked_position},
    report::EditReport,
    request::CheckRequest,
};

impl Runtime {
    pub(crate) fn rename(
        &mut self,
        request: RenameRequest,
        report: &mut EditReport,
        options: &Options,
    ) -> io::Result<ValidatedEdit> {
        checked_position(&request.snapshot, &request.position)?;
        let categories = [SemanticCategory::Bindings, SemanticCategory::Occurrences];

        let state = self.query_capture(
            Capture::new(Arc::clone(&self.host), request.snapshot),
            Some(request.position.source.identity),
            request.workspace,
            &categories,
            None,
            options,
        )?;

        report.snapshot = state.capture.snapshot.clone();
        report.inputs = state.capture.guards();
        report.diagnostics.extend(state.diagnostics.clone());
        report.completion = report.completion.clone().combine(state.completion.clone());
        state.require_complete(options)?;

        if !state.index.available(&categories) {
            return Err(Reason::UnavailableSemantics.error());
        }

        let selected = state
            .index
            .selected(request.position.source, request.position.offset)
            .ok_or_else(|| invalid("symbol cannot be renamed"))?;

        let Knowledge::Known(targets) = &selected.targets else {
            return Err(Reason::UnavailableSemantics.error());
        };

        if targets.len() != 1 || !matches!(targets[0], SemanticIdentity::Source { .. }) {
            return Err(invalid(
                "rename includes ambiguous or read-only declarations",
            ));
        }

        let references = navigation::references(
            &ReferenceQuery {
                position: request.position,
                include_declaration: true,
                workspace: request.workspace,
                highlights: false,
            },
            &state.index,
        )
        .map_err(Reason::error)?;

        let mut edits = Vec::new();
        let mut guards = BTreeSet::new();

        for reference in references {
            options.check()?;

            let source = state
                .capture
                .snapshot
                .source(reference.location.source.identity)
                .ok_or_else(|| invalid("rename source is absent"))?;

            reference.location.validate(source)?;

            if state.capture.snapshot.is_declaration(source.version)
                || matches!(
                    state.capture.snapshot.origin(source.version.identity),
                    Some(SourceOrigin::Virtual { .. })
                )
                || state
                    .graph
                    .modules
                    .keys()
                    .filter(|module| module.source == source.version.identity)
                    .count()
                    != 1
            {
                return Err(invalid(
                    "rename includes a read-only or multiply mapped source",
                ));
            }

            let spelling = &source.text[reference.location.span.start..reference.location.span.end];

            let replacement = replacement(spelling, &request.name)?;

            edits.push(TextEdit {
                location: reference.location,
                replacement,
            });

            guards.insert(source.version);
        }

        let proposal = EditProposal {
            identity: EditIdentity::new(self.identity()),
            edits,
            guards: guards.into_iter().collect(),
            intent: EditIntent::Rename,
        };

        report.proposal = Some(proposal.clone());

        self.validate_edit(
            proposal,
            &state.capture.snapshot,
            &state.capture.guards(),
            vec![
                EditGuarantee::Ranges,
                EditGuarantee::Syntax,
                EditGuarantee::Bindings,
                EditGuarantee::Diagnostics,
                EditGuarantee::Resolution,
            ],
            &CheckRequest::default(),
            options,
        )
    }
}

fn replacement(spelling: &str, name: &str) -> io::Result<String> {
    if identifier(spelling) {
        Ok(name.to_owned())
    } else if literal::decode(spelling.as_bytes()).is_some() {
        Ok(literal::quote(name))
    } else {
        Err(invalid("rename includes unsupported reference spellings"))
    }
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}
