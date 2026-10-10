use instar_luau::{InspectionResult, OccurrenceAccess, OccurrenceFact, SemanticIdentity};
use instar_shared::{Knowledge, Location, Reason};

use super::{
    index::Index,
    request::{PositionQueryKind, ReferenceQuery},
};

pub(crate) fn locations(
    inspection: &InspectionResult,
    kind: PositionQueryKind,
    index: &Index,
) -> Result<Vec<Location>, Reason> {
    let Some(occurrence) = &inspection.occurrence else {
        return Ok(Vec::new());
    };

    let locations = match kind {
        PositionQueryKind::Declaration => &occurrence.declarations,
        PositionQueryKind::Definition => &occurrence.definitions,
        PositionQueryKind::TypeDefinition => &occurrence.type_definitions,

        PositionQueryKind::Implementation => {
            let Knowledge::Known(targets) = &occurrence.targets else {
                return Err(Reason::UnavailableSemantics);
            };

            let mut locations = Vec::new();

            for facts in index.analyses.values() {
                for function in &facts.facts.functions {
                    if targets.contains(&function.identity) {
                        locations.push(function.body);
                    }
                }

                for nominal in &facts.facts.nominals {
                    if targets.contains(&nominal.identity)
                        || facts.facts.inheritance.iter().any(|edge| {
                            edge.child == nominal.identity && targets.contains(&edge.parent)
                        })
                    {
                        locations.push(nominal.declaration);
                    }
                }
            }

            return Ok(locations);
        }

        _ => return Err(Reason::UnavailableSemantics),
    };

    match locations {
        Knowledge::Known(locations) => Ok(locations.clone()),
        Knowledge::Unavailable(reason) => Err(*reason),
    }
}

pub(crate) fn references(
    request: &ReferenceQuery,
    index: &Index,
) -> Result<Vec<OccurrenceFact>, Reason> {
    let selected = index.selected(request.position.source, request.position.offset);

    let Some(selected) = selected else {
        return Ok(Vec::new());
    };

    let Knowledge::Known(targets) = &selected.targets else {
        return Err(Reason::UnavailableSemantics);
    };

    let mut occurrences = Vec::new();

    for facts in index.analyses.values() {
        for occurrence in &facts.facts.occurrences {
            if request.highlights
                && occurrence.location.source.identity != request.position.source.identity
            {
                continue;
            }

            if occurrence.declaration && !request.include_declaration {
                continue;
            }

            match &occurrence.targets {
                Knowledge::Known(candidate)
                    if candidate.iter().any(|target| targets.contains(target)) =>
                {
                    occurrences.push(occurrence.clone());
                }

                Knowledge::Unavailable(reason) => return Err(*reason),
                Knowledge::Known(_) => {}
            }
        }
    }

    if request.include_declaration {
        for target in targets {
            if let SemanticIdentity::Source { location, .. } = target
                && !occurrences
                    .iter()
                    .any(|occurrence| occurrence.declaration && occurrence.location == *location)
            {
                occurrences.push(OccurrenceFact {
                    location: *location,
                    targets: Knowledge::Known(vec![target.clone()]),
                    declaration: true,
                    access: OccurrenceAccess::Write,
                    role: selected.role,
                    deprecated: selected.deprecated,
                });
            }
        }
    }

    occurrences.sort_by_key(|occurrence| (occurrence.location.source, occurrence.location.span));
    occurrences.dedup();

    Ok(occurrences)
}
