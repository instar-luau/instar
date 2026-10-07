//! Native Luau configuration and host-adapted analysis representation.

#[expect(
    clippy::absolute_paths,
    clippy::arbitrary_source_item_ordering,
    reason = "CXX generates fully qualified paths and pointer trait implementations after factory functions"
)]
#[expect(
    unsafe_code,
    unreachable_pub,
    reason = "CXX generates public unsafe declarations inside this private native boundary"
)]
mod boundary;

mod configuration;
pub mod flags;
pub mod frontend;
mod protocol;

/// Native worker process entry point.
pub mod worker;

pub use configuration::{Alias, Configuration, LintPolicy, Mode, Snapshot};
