use instar_luau::{InspectionResult, SemanticCategory, SemanticIdentity};
use instar_shared::{Availability, Completion, Knowledge, Reason};

use super::{
    index::Index,
    output::{HierarchyCall, HierarchyOutput},
    request::{HierarchyQuery, HierarchyQueryKind},
};

pub(crate) fn hierarchy(
    request: &HierarchyQuery,
    index: &Index,
    inspection: Option<&InspectionResult>,
) -> Result<HierarchyOutput, Reason> {
    let selected = if let Some(identity) = &request.identity {
        vec![identity.clone()]
    } else if request.position.is_some() {
        let inspection = inspection.ok_or(Reason::FailedPrerequisite)?;

        let category = match request.kind {
            HierarchyQueryKind::PrepareTypes
            | HierarchyQueryKind::Supertypes
            | HierarchyQueryKind::Subtypes => SemanticCategory::Nominals,

            HierarchyQueryKind::PrepareCalls
            | HierarchyQueryKind::IncomingCalls
            | HierarchyQueryKind::OutgoingCalls => SemanticCategory::Functions,
        };

        match &inspection.occurrence {
            Some(occurrence) => match &occurrence.hierarchy {
                Knowledge::Known(identities) => identities.clone(),
                Knowledge::Unavailable(reason) => return Err(*reason),
            },

            None if inspection.completion == Completion::Complete
                && inspection.availability.categories.get(&category)
                    == Some(&Availability::Available) =>
            {
                Vec::new()
            }

            None => return Err(Reason::UnavailableSemantics),
        }
    } else {
        return Err(Reason::FailedPrerequisite);
    };

    let mut output = HierarchyOutput {
        selected: if matches!(
            request.kind,
            HierarchyQueryKind::Supertypes | HierarchyQueryKind::Subtypes
        ) {
            Vec::new()
        } else {
            selected.clone()
        },
        ..HierarchyOutput::default()
    };

    for facts in index.analyses.values() {
        for function in &facts.facts.functions {
            if selected.contains(&function.identity) {
                output.functions.push(function.clone());
            }
        }

        for nominal in &facts.facts.nominals {
            if output.selected.contains(&nominal.identity) {
                output.types.push(nominal.clone());
            }
        }

        match request.kind {
            HierarchyQueryKind::IncomingCalls | HierarchyQueryKind::OutgoingCalls => {
                for call in &facts.facts.calls {
                    let Knowledge::Known(targets) = &call.targets else {
                        return Err(Reason::UnavailableSemantics);
                    };

                    for target in targets {
                        if matches!(request.kind, HierarchyQueryKind::IncomingCalls)
                            && selected.contains(target)
                            || matches!(request.kind, HierarchyQueryKind::OutgoingCalls)
                                && selected.contains(&call.caller)
                        {
                            output.calls.push(HierarchyCall {
                                from: call.caller.clone(),
                                to: target.clone(),
                                location: call.callee,
                            });

                            for function in index
                                .analyses
                                .values()
                                .flat_map(|facts| &facts.facts.functions)
                            {
                                if function.identity == call.caller || function.identity == *target
                                {
                                    output.functions.push(function.clone());
                                }
                            }
                        }
                    }
                }
            }

            HierarchyQueryKind::Supertypes | HierarchyQueryKind::Subtypes => {
                let related = facts
                    .facts
                    .inheritance
                    .iter()
                    .filter_map(|edge| {
                        if matches!(request.kind, HierarchyQueryKind::Supertypes)
                            && selected.contains(&edge.child)
                        {
                            Some(&edge.parent)
                        } else if matches!(request.kind, HierarchyQueryKind::Subtypes)
                            && selected.contains(&edge.parent)
                        {
                            Some(&edge.child)
                        } else {
                            None
                        }
                    })
                    .collect::<Vec<&SemanticIdentity>>();

                for nominal in index
                    .analyses
                    .values()
                    .flat_map(|facts| &facts.facts.nominals)
                {
                    if related.contains(&&nominal.identity) {
                        output.types.push(nominal.clone());

                        if !output.selected.contains(&nominal.identity) {
                            output.selected.push(nominal.identity.clone());
                        }
                    }
                }
            }

            HierarchyQueryKind::PrepareCalls | HierarchyQueryKind::PrepareTypes => {}
        }
    }

    order(&mut output);

    Ok(output)
}

fn order(output: &mut HierarchyOutput) {
    output
        .functions
        .sort_by_key(|function| (function.selection.source, function.selection.span));

    output.functions.dedup();

    output
        .types
        .sort_by_key(|nominal| (nominal.selection.source, nominal.selection.span));

    output.types.dedup();

    output
        .calls
        .sort_by_key(|call| (call.location.source, call.location.span));

    output.calls.dedup_by(|left, right| {
        left.from == right.from && left.to == right.to && left.location == right.location
    });
}
