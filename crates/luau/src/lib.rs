//! Persistent native Luau analysis over host-provided sources and configuration.

#[expect(
    unsafe_code,
    unreachable_pub,
    clippy::absolute_paths,
    clippy::inline_modules,
    reason = "CXX generates public FFI items, unsafe code, absolute paths, and an inline module"
)]
mod bridge;

mod configuration;
mod environment;
mod error;
mod flags;
mod frontend;
mod host;
mod instances;

pub use bridge::bindings::Diagnostic;
pub use configuration::{Configuration, Mode};
pub use environment::{Class, Definition, Environment, Property};
pub use error::{Error, HostError, Operation};
pub use flags::Flag;
pub use frontend::Frontend;
pub use host::{Context, Host, Navigation};
pub use instances::{Instance, Location};
