use std::ops::Range;

use instar_bridge::{Signature as NativeSignature, Subject};

use super::{
    rendering::{Rendered, Renderer, bounded, function_text, root_function},
    spelling::identifier,
    surfaces::Section,
};

use crate::format::{Error, fragment::Grammar};

/// A signature constructed from exactly its visible native parameters.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Signature {
    /// A single-line label retaining every visible parameter slot.
    pub label: String,

    /// Final byte ranges into the label, in visible parameter order.
    pub parameters: Vec<Range<usize>>,

    /// Expanded detail, empty when the label already preserves the complete declaration.
    pub detail: Vec<Section>,
}

/// Construct a signature label and parameter spans from the same visible sequence.
///
/// # Errors
/// Returns invalid lexical data or a fragment formatting/preservation failure.
pub fn signature(native: &NativeSignature) -> Result<Signature, Error> {
    let snapshot = &native.snapshot;
    let mut compact = Renderer::new(snapshot, true, 48)?;
    let function = root_function(snapshot)?;

    let (binders, has_complete_binders) = match function {
        Some(function) => compact.binders(function)?,
        None => (String::new(), false),
    };

    let is_checked = function.is_some_and(|function| function.checked);

    let attribute = if is_checked { "@checked " } else { "" };

    let mut label = format!("{attribute}{binders}(");
    let mut parameters = Vec::with_capacity(native.parameters.len());
    let mut prefixes = Vec::with_capacity(native.parameters.len());
    let mut is_abbreviated = !has_complete_binders;

    for parameter in &native.parameters {
        if !parameters.is_empty() {
            label.push_str(", ");
        }

        let child = compact.render(parameter.subject, 0)?;

        let text = if child.grammar.is_some() {
            compact.validate(&child)?
        } else {
            child.text.clone()
        };

        let start = label.len();

        if parameter.variadic {
            if matches!(parameter.subject, Subject::Type(_)) {
                label.push_str("...");
            }
        } else if !parameter.name.is_empty() {
            label.push_str(identifier(&parameter.name)?);
            label.push_str(": ");
        }

        prefixes.push(label[start..].to_owned());

        label.push_str(&text);
        parameters.push(start..label.len());
        is_abbreviated |= child.is_abbreviated;
    }

    let returns = match function {
        Some(function) => compact.returns(function.returns, 0)?,
        None => Rendered::display("<omitted>", true),
    };

    let return_label = if returns.grammar.is_some() {
        compact.validate(&returns)?
    } else {
        returns.text.clone()
    };

    label.push_str(") -> ");
    label.push_str(&return_label);
    is_abbreviated |= returns.is_abbreviated;
    let expanded = signature_detail(native, &prefixes)?;

    let is_redundant = !is_abbreviated
        && expanded.len() == 1
        && !expanded[0].content.contains('\n')
        && expanded[0].content == label;

    Ok(Signature {
        label,
        parameters,
        detail: if is_redundant { Vec::new() } else { expanded },
    })
}

fn signature_detail(native: &NativeSignature, prefixes: &[String]) -> Result<Vec<Section>, Error> {
    let snapshot = &native.snapshot;
    let function = root_function(snapshot)?;
    let mut renderer = Renderer::new(snapshot, false, 80)?;
    let mut children = Vec::with_capacity(native.parameters.len() + 1);

    for parameter in &native.parameters {
        children.push(renderer.render(parameter.subject, 0)?);
    }

    let (binders, has_complete_binders) = match function {
        Some(function) => renderer.binders(function)?,
        None => (String::new(), false),
    };

    let returns = match function {
        Some(function) => renderer.returns(function.returns, 0)?,
        None => Rendered::display("<omitted>", true),
    };

    let is_checked = function.is_some_and(|function| function.checked);
    let header = format!("{}{binders}", if is_checked { "@checked " } else { "" });
    children.push(returns);
    let is_source = has_complete_binders && children.iter().all(|child| !child.is_declaration);

    let value = renderer.combine(
        if is_checked {
            Grammar::Declaration
        } else {
            Grammar::Annotation
        },
        is_source,
        &children,
        |children| function_text(&header, prefixes, children, " -> "),
    )?;

    let primary = renderer.section(&value)?;

    Ok(bounded(primary))
}
