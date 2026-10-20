use instar_bridge::{Hover, Snapshot, Subject};

use super::rendering::Renderer;
use crate::format::Error;

/// The compact presentation policy for an editor surface.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum Surface {
    /// A binding type, including the colon prefix.
    TypeHint,

    /// A return pack, including the colon prefix.
    ReturnHint,

    /// Completion detail without a colon prefix.
    Completion,
}

/// A type or declaration display.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Section {
    /// The complete section content.
    pub content: String,
}

/// Render a single-line label according to the selected surface's width policy.
///
/// # Errors
/// Returns invalid lexical data or a fragment formatting/preservation failure.
pub fn compact(snapshot: &Snapshot, surface: Surface) -> Result<String, Error> {
    let prefix = if surface == Surface::Completion {
        ""
    } else {
        ": "
    };

    let width = if surface == Surface::Completion {
        80
    } else {
        48 - 2
    };

    let mut renderer = Renderer::new(snapshot, true, width)?;

    let value = match (surface, snapshot.root) {
        (Surface::ReturnHint, Subject::Pack(index)) => renderer.returns(index, 0)?,
        _ => renderer.render(snapshot.root, 0)?,
    };

    let text = if value.grammar.is_some() {
        renderer.validate(&value)?
    } else {
        value.text
    };

    Ok(format!("{prefix}{text}"))
}

/// Render a type display.
///
/// # Errors
/// Returns invalid lexical data or a fragment formatting/preservation failure.
pub fn detail(snapshot: &Snapshot, symbol: impl AsRef<[u8]>) -> Result<Vec<Section>, Error> {
    Renderer::new(snapshot, false, 80)?.finish(symbol.as_ref(), None)
}

/// Render the selected hover subject in its declaration context.
///
/// # Errors
/// Returns invalid lexical data or a formatting/preservation failure.
pub fn hover(value: &Hover) -> Result<Vec<Section>, Error> {
    Renderer::new(&value.snapshot, false, 80)?.finish(&value.symbol, Some(value.kind))
}
