//! Project analysis identities and source anchoring.

mod diagnostics;
mod identity;

pub(crate) use diagnostics::Report;
pub(crate) use identity::locate;
pub use identity::{Entry, Origin};
