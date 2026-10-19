use std::cell::OnceCell;

use vermis::tree::{NodeIndex, NodeKind};

use crate::lint::{
    Failure, Finding, Prepared, Symbols, bindings, calls, context::Context, control, directives,
    expression, syntax,
};

pub(super) const RULES: &[&str] = &[
    "almost_swapped",
    "must_use",
    "self_assignment",
    "ignored_pcall_result",
    "unscoped_variables",
    "roblox_incorrect_color3_new_bounds",
    "roblox_manual_fromscale_or_fromoffset",
    "roblox_prefer_get_players",
    "roblox_suspicious_udim2_new",
    "if_expression_assignment",
    "non_const_require",
    "global_usage",
    "type_check_inside_call",
    "loop_invariant_call",
    "manual_table_clone",
    "string_concat_in_loop",
    "unused_variable",
    "prefer_const",
    "restricted_globals",
    "restricted_module_paths",
    "UnknownGlobal",
    "GlobalUsedAsLocal",
    "LocalShadow",
    "SameLineStatement",
    "MultiLineStatement",
    "LocalUnused",
    "FunctionUnused",
    "ImportUnused",
    "BuiltinGlobalWrite",
    "PlaceholderRead",
    "UnreachableCode",
    "UnknownType",
    "ForRange",
    "UnbalancedAssignment",
    "ImplicitReturn",
    "TableLiteral",
    "UninitializedLocal",
    "DuplicateFunction",
    "DuplicateCondition",
    "DuplicateLocal",
    "MisleadingAndOr",
    "IntegerParsing",
    "ComparisonPrecedence",
    "CommentDirective",
    "RedundantNativeAttribute",
];

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

        if settings.syntax {
            expression::visit(&mut context, node);
        }

        if settings.analysis {
            match tree.node(node).kind {
                NodeKind::Block { .. }
                | NodeKind::If { .. }
                | NodeKind::Conditional { .. }
                | NodeKind::NumericFor { .. }
                | NodeKind::Function { .. } => control::analyze(&mut context, node),

                NodeKind::Call { .. }
                | NodeKind::MethodCall { .. }
                | NodeKind::CallStatement { .. } => calls::analyze(&mut context, node),

                _ => expression::analyze(&mut context, node),
            }
        }
    }

    if settings.analysis {
        bindings::finish(&mut context);
    }

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
