//! Native analysis snapshots adapted from host-owned resolution results.

mod client;
mod host;
mod result;
mod source;

pub use client::Frontend;
pub(crate) use host::{Cancellation, Host};
pub use result::{Fact, FactKind, LintResult, Warning};
pub(crate) use source::Source;
pub use source::{Class, Definition, Link, Property, Site, validate_namespace};
