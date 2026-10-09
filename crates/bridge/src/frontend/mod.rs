//! Native analysis snapshots adapted from host-owned resolution results.

mod client;
mod hierarchy;
mod host;
mod references;
mod result;
mod signatures;
mod source;
mod suggestions;

pub use client::Frontend;
pub use hierarchy::{Call, Hierarchy, HierarchyItem, HierarchyKind, Inheritance};
pub(crate) use host::{Cancellation, Host};
pub use references::{Classification, Namespace, Reference, Role};
pub use result::{Fact, FactKind, Hover, LintResult, Warning};
pub(crate) use result::{checking, completion, location};
pub use signatures::{Signature, Signatures};
pub use source::{Class, Definition, Link, Property, Site, validate_namespace};
pub(crate) use source::{Declaration, Environment, Source};

pub use suggestions::{
    Notation, Parentheses, Suggestion, SuggestionContext, SuggestionKind, Suggestions,
};
