use std::cell::OnceCell;

use vermis::tree::{NodeIndex, NodeKind};

use crate::lint::{
    Failure, Finding, Prepared, Symbols, bindings, calls, context::Context, control, directives,
    expression, syntax,
};

/// Run configured rules and validate suppression directives against one strict parse.
///
/// # Errors
/// Returns syntax or directive failures without findings or corrections.
pub fn lint(source: &str, settings: &Prepared, symbols: &Symbols) -> Result<Vec<Finding>, Failure> {
    let prefix = usize::from(source.starts_with('\u{feff}')) * 3;
    let text = &source[prefix..];
    let tree = syntax::parse(source).map_err(Failure::Syntax)?;
    let bindings = OnceCell::new();
    let lines = OnceCell::new();

    let mut context = Context {
        tree: &tree,
        source: text,
        settings,
        symbols,
        bindings: &bindings,
        findings: Vec::new(),
        lines: &lines,
    };

    for index in 0..tree.nodes.len() {
        let node = NodeIndex::new(index);

        match tree.node(node).kind {
            NodeKind::Block { .. }
            | NodeKind::Branch { .. }
            | NodeKind::If { .. }
            | NodeKind::Conditional { .. }
            | NodeKind::While { .. }
            | NodeKind::Repeat { .. }
            | NodeKind::NumericFor { .. }
            | NodeKind::GenericFor { .. }
            | NodeKind::Function { .. } => control::check(&mut context, node),

            NodeKind::Call { .. }
            | NodeKind::MethodCall { .. }
            | NodeKind::CallStatement { .. } => calls::check(&mut context, node),

            _ => expression::check(&mut context, node),
        }
    }

    bindings::finish(&mut context);

    directives::suppress(text, &tree, &mut context.findings).map_err(|mut error| {
        error.range = error.range.start + prefix..error.range.end + prefix;

        Failure::Directive(error)
    })?;

    for finding in &mut context.findings {
        finding.range.start += prefix;
        finding.range.end += prefix;

        for edit in &mut finding.edits {
            edit.range.start += prefix;
            edit.range.end += prefix;
        }
    }

    Ok(context.findings)
}
