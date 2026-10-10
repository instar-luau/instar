use std::{io, sync::Arc};

use instar_luau::{EnvironmentIdentity, SemanticCategory, SemanticIdentity};
use instar_shared::{EditProposal, Knowledge, Location, Options, Reason, Span};

use super::validation::relocated;

use crate::{
    capture::Capture,
    execution::Runtime,
    inputs::{InputVersion, ProjectSnapshot},
    queries::service::QueryEvidence,
    request::CheckRequest,
};

impl Runtime {
    pub(crate) fn validate_bindings(
        &mut self,
        proposal: &EditProposal,
        original: &ProjectSnapshot,
        proposed: &ProjectSnapshot,
        workspace: bool,
        context: &CheckRequest,
        options: &Options,
    ) -> io::Result<Vec<InputVersion>> {
        let source = proposal.guards.first().map(|version| version.identity);
        let categories = [SemanticCategory::Bindings, SemanticCategory::Occurrences];

        let before = self.query_capture(
            Capture::new(Arc::clone(&self.host), original.clone()),
            source,
            workspace,
            QueryEvidence::Inventory(&categories),
            Some(context),
            options,
        )?;

        let after = self.query_capture(
            Capture::new(Arc::clone(&self.host), proposed.clone()),
            source,
            workspace,
            QueryEvidence::Inventory(&categories),
            Some(context),
            options,
        )?;

        before.require_complete(options)?;
        after.require_complete(options)?;

        if !before.index.available(&categories) || !after.index.available(&categories) {
            return Err(Reason::UnavailableSemantics.error());
        }

        for occurrence in before
            .index
            .analyses
            .values()
            .flat_map(|facts| &facts.facts.occurrences)
        {
            options.check()?;
            let span = occurrence.location.span;

            if proposal.edits.iter().any(|edit| {
                edit.location.source == occurrence.location.source
                    && edit.location.span.start <= span.start
                    && span.end <= edit.location.span.end
                    && edit.replacement.is_empty()
            }) {
                continue;
            }

            let location = location(occurrence.location, proposed, proposal)?;

            let updated = after
                .index
                .analyses
                .values()
                .flat_map(|facts| &facts.facts.occurrences)
                .find(|updated| updated.location == location)
                .ok_or_else(|| invalid("proposal loses an indexed symbol occurrence"))?;

            let Knowledge::Known(before_targets) = &occurrence.targets else {
                return Err(Reason::UnavailableSemantics.error());
            };

            let Knowledge::Known(after_targets) = &updated.targets else {
                return Err(Reason::UnavailableSemantics.error());
            };

            let before_targets = before_targets
                .iter()
                .map(|target| identity(target, proposed, proposal))
                .collect::<io::Result<Vec<_>>>()?;

            let after_targets = after_targets
                .iter()
                .map(|target| canonical(target.clone()))
                .collect::<Vec<_>>();

            if before_targets.len() != after_targets.len()
                || before_targets
                    .iter()
                    .any(|target| !after_targets.contains(target))
                || occurrence.declaration != updated.declaration
                || occurrence.access != updated.access
                || occurrence.role != updated.role
            {
                return Err(invalid("proposal changes indexed binding relationships"));
            }
        }

        Ok(before
            .capture
            .guards()
            .into_iter()
            .chain(after.capture.guards())
            .collect())
    }
}

fn identity(
    identity: &SemanticIdentity,
    proposed: &ProjectSnapshot,
    proposal: &EditProposal,
) -> io::Result<SemanticIdentity> {
    Ok(canonical(match identity {
        SemanticIdentity::Source {
            location: original,
            kind,
            ordinal,
        } => SemanticIdentity::Source {
            location: location(*original, proposed, proposal)?,
            kind: *kind,
            ordinal: *ordinal,
        },

        other @ SemanticIdentity::Environment { .. } => other.clone(),
    }))
}

fn canonical(identity: SemanticIdentity) -> SemanticIdentity {
    match identity {
        SemanticIdentity::Source { location, kind, .. } => SemanticIdentity::Source {
            location,
            kind,
            ordinal: 0,
        },

        SemanticIdentity::Environment {
            namespace,
            name,
            kind,
            ..
        } => SemanticIdentity::Environment {
            environment: EnvironmentIdentity::new(0),
            namespace,
            name,
            kind,
        },
    }
}

fn location(
    original: Location,
    proposed: &ProjectSnapshot,
    proposal: &EditProposal,
) -> io::Result<Location> {
    let source = proposed
        .source(original.source.identity)
        .ok_or_else(|| invalid("binding source is absent from the proposed capture"))?;

    let (start, end) = relocated(
        original.source,
        original.span.start,
        original.span.end,
        &proposal.edits,
    )?;

    Location::new(source, Span { start, end })
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}
