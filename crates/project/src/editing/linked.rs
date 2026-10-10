use std::{io, sync::Arc};

use instar_luau::{SemanticCategory, SemanticIdentity, SemanticKind};
use instar_shared::{Knowledge, Options, Reason};
use instar_syntax::identifier;

use super::request::LinkedRequest;

use crate::{
    capture::Capture,
    execution::Runtime,
    queries::{
        ReferenceQuery, navigation,
        service::{QueryEvidence, checked_position},
    },
    report::EditReport,
};

impl Runtime {
    pub(crate) fn linked(
        &mut self,
        request: LinkedRequest,
        report: &mut EditReport,
        options: &Options,
    ) -> io::Result<()> {
        checked_position(&request.snapshot, &request.position)?;
        let categories = [SemanticCategory::Bindings, SemanticCategory::Occurrences];

        let state = self.query_capture(
            Capture::new(Arc::clone(&self.host), request.snapshot),
            Some(request.position.source.identity),
            false,
            QueryEvidence::Inventory(&categories),
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

        let Some(selected) = state
            .index
            .selected(request.position.source, request.position.offset)
        else {
            return Ok(());
        };

        let Knowledge::Known(targets) = &selected.targets else {
            return Err(Reason::UnavailableSemantics.error());
        };

        if !matches!(targets.as_slice(), [SemanticIdentity::Source { location, kind: SemanticKind::Binding | SemanticKind::Parameter | SemanticKind::Function | SemanticKind::Type, .. }] if location.source == request.position.source)
        {
            return Err(invalid(
                "linked editing requires one declaration in the requested source",
            ));
        }

        if state
            .graph
            .modules
            .keys()
            .filter(|module| module.source == request.position.source.identity)
            .count()
            != 1
        {
            return Err(invalid(
                "linked editing source has multiple semantic contexts",
            ));
        }

        let source = state
            .capture
            .snapshot
            .source(request.position.source.identity)
            .ok_or_else(|| invalid("linked editing source is absent"))?;

        selected.location.validate(source)?;
        let name = &source.text[selected.location.span.start..selected.location.span.end];

        if !identifier(name) {
            return Err(invalid("linked editing requires identifier spellings"));
        }

        let references = navigation::references(
            &ReferenceQuery {
                position: request.position,
                include_declaration: true,
                workspace: false,
                highlights: false,
            },
            &state.index,
        )
        .map_err(Reason::error)?;

        let mut ranges = Vec::new();

        for reference in references {
            options.check()?;

            if reference.location.source != source.version {
                return Err(invalid(
                    "linked editing references extend outside the requested source",
                ));
            }

            reference.location.validate(source)?;

            if &source.text[reference.location.span.start..reference.location.span.end] != name {
                return Err(invalid(
                    "linked editing references have different spellings",
                ));
            }

            ranges.push(reference.location);
        }

        ranges.sort_by_key(|location| location.span);
        ranges.dedup();

        if ranges
            .windows(2)
            .any(|pair| pair[0].span.end > pair[1].span.start)
        {
            return Err(invalid("linked editing ranges overlap"));
        }

        report.linked_ranges = ranges;

        Ok(())
    }
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}
