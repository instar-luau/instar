//! Native Luau configuration and host-adapted analysis representation.

#[expect(
    unsafe_code,
    unreachable_pub,
    reason = "CXX generates public unsafe declarations inside this private native boundary"
)]
mod boundary;

mod configuration;
pub mod frontend;

pub use configuration::{Alias, Configuration, LintPolicy, Mode, Snapshot};
