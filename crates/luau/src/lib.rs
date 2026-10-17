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

pub use self::{
    bridge::bindings::Diagnostic,
    configuration::{Configuration, Mode},
    environment::{Class, Definition, Environment, Property},
    error::{Error, HostError, Operation},
    flags::Flag,
    frontend::Frontend,
    host::{Context, Host, Navigation, Progress},
    instances::{Instance, Location},
};
