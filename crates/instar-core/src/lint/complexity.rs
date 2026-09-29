use vermis::{Kind, Parts, View};

use super::{Context, Finding};

fn score(node: View<'_, '_>, root: bool) -> usize {
    if !root && matches!(node.kind(), Kind::Function | Kind::LocalFunction) {
        return 0;
    }

    let branch = match node.kind() {
        Kind::Branch
        | Kind::While
        | Kind::Repeat
        | Kind::NumericFor
        | Kind::GenericFor
        | Kind::Conditional => 1,

        Kind::Binary => usize::from(
            matches!(node.parts(), Some(Parts::Binary { operator, .. }) if operator.text() == b"and" || operator.text() == b"or"),
        ),

        _ => 0,
    };

    branch
        + node
            .children()
            .map(|child| score(child, false))
            .sum::<usize>()
}

pub(super) fn check(
    node: View<'_, '_>,
    _ancestors: &[View<'_, '_>],
    context: &Context<'_>,
    findings: &mut Vec<Finding>,
) {
    if !context.enabled("high_cyclomatic_complexity")
        || !matches!(node.kind(), Kind::Function | Kind::LocalFunction)
    {
        return;
    }

    let complexity = score(node, true) + 1;

    let maximum = context
        .config
        .options
        .high_cyclomatic_complexity
        .maximum_complexity();

    if complexity > maximum {
        context.emit(
            findings,
            "high_cyclomatic_complexity",
            node.span(),
            format!("function complexity {complexity} exceeds maximum {maximum}"),
        );
    }
}
