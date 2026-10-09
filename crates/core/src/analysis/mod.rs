//! Project analysis identities and source anchoring.

mod diagnostics;
mod identity;
mod preparation;

pub(crate) use diagnostics::Report;
pub(crate) use identity::locate;
pub use identity::{Entry, Origin};
pub(crate) use preparation::Preparation;
