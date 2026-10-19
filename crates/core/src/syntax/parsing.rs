use std::ops::Range;

use vermis::tree::{NodeKind, Tree};

pub(crate) fn parse<Failure>(
    source: &[u8],
    invalid: impl FnOnce(Range<usize>, Option<&'static str>) -> Failure,
) -> Result<Tree<'_>, Failure> {
    let tree = vermis::parse(source);

    if let Some(diagnostic) = tree.diagnostics.first() {
        return Err(invalid(
            diagnostic.span.start..diagnostic.span.end,
            Some(diagnostic.message),
        ));
    }

    if let Some(node) = tree
        .nodes
        .iter()
        .find(|node| matches!(node.kind, NodeKind::Error | NodeKind::Missing { .. }))
    {
        return Err(invalid(node.span.start..node.span.end, None));
    }

    Ok(tree)
}
