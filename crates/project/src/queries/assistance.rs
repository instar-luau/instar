use instar_luau::{BindingKind, OccurrenceRole, SemanticIdentity};
use instar_shared::{Knowledge, Reason};

use super::{
    index::Index,
    output::{AssistanceOutput, ProjectHint, ProjectInlineValue, ProjectLens},
    request::{PositionQuery, PositionQueryKind},
};

use crate::inputs::ProjectSnapshot;

pub(crate) fn assistance(
    request: &PositionQuery,
    snapshot: &ProjectSnapshot,
    index: &Index,
) -> Result<AssistanceOutput, Reason> {
    match request.kind {
        PositionQueryKind::Hints => {
            let mut hints = Vec::new();

            for binding in index
                .analyses
                .values()
                .flat_map(|facts| &facts.facts.bindings)
            {
                if binding.annotation
                    || matches!(
                        binding.kind,
                        BindingKind::Global | BindingKind::Property | BindingKind::Type
                    )
                {
                    continue;
                }

                let Knowledge::Known(Some(declaration)) = binding.declaration else {
                    continue;
                };

                if declaration.source != request.source {
                    continue;
                }

                let Knowledge::Known(information) = &binding.type_information else {
                    return Err(Reason::UnavailableSemantics);
                };

                hints.push(ProjectHint {
                    location: declaration,
                    label: format!(": {}", information.display),
                    description: Some(format!("{}: {}", binding.name, information.display)),
                    documentation: binding.documentation.clone(),
                });
            }

            hints.sort_by_key(|hint| hint.location.span);

            hints.dedup_by(|left, right| {
                left.location == right.location && left.label == right.label
            });

            Ok(AssistanceOutput::Hints(hints))
        }

        PositionQueryKind::Lenses => {
            let mut lenses = Vec::new();

            for function in index
                .analyses
                .values()
                .flat_map(|facts| &facts.facts.functions)
            {
                if function.selection.source != request.source {
                    continue;
                }

                let references = index.analyses.values().flat_map(|facts| &facts.facts.occurrences).filter(|occurrence| !occurrence.declaration && matches!(&occurrence.targets, Knowledge::Known(targets) if targets.contains(&function.identity))).count();

                lenses.push(ProjectLens {
                    location: function.selection,
                    title: format!("{references} references"),
                    identity: function.identity.clone(),
                });
            }

            Ok(AssistanceOutput::Lenses(lenses))
        }

        PositionQueryKind::InlineValues => {
            let source = snapshot
                .source(request.source.identity)
                .filter(|source| source.version == request.source)
                .ok_or(Reason::StaleQuery)?;

            let mut values = Vec::new();

            for occurrence in index
                .analyses
                .values()
                .flat_map(|facts| &facts.facts.occurrences)
            {
                if occurrence.location.source != request.source
                    || occurrence.role != OccurrenceRole::Variable
                {
                    continue;
                }

                let name = source
                    .text
                    .get(occurrence.location.span.start..occurrence.location.span.end)
                    .ok_or(Reason::FailedPrerequisite)?;

                if matches!(&occurrence.targets, Knowledge::Known(targets) if targets.iter().any(|target| matches!(target, SemanticIdentity::Source { .. })))
                {
                    values.push(ProjectInlineValue {
                        location: occurrence.location,
                        variable: Some(name.to_owned()),
                        expression: None,
                    });
                }
            }

            values.sort_by_key(|value| value.location.span);
            values.dedup_by(|left, right| left.location == right.location);

            Ok(AssistanceOutput::InlineValues(values))
        }

        _ => Err(Reason::UnsupportedFeature),
    }
}
