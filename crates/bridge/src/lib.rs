//! Persistent native Luau analysis over host-provided sources and configuration.

pub mod types;

#[expect(
    unsafe_code,
    unreachable_pub,
    clippy::absolute_paths,
    clippy::inline_modules,
    reason = "CXX generates public FFI items, unsafe code, absolute paths, and an inline module"
)]
mod bridge;

mod cancellation;
mod configuration;
mod environment;
mod error;
mod flags;
mod frontend;
mod host;
mod instances;
mod semantic;

pub use self::{
    bridge::bindings::{
        CompletionKind, Diagnostic, HintKind, HintSelector, HoverKind, Import, Occurrence,
        OccurrenceRole, Position, Range, SourceLocation,
    },
    cancellation::Cancellation,
    configuration::{Configuration, Mode},
    environment::{Class, Definition, Environment, Property, declarations},
    error::{Error, HostError, Operation},
    flags::Flag,
    frontend::Frontend,
    host::{Context, Host, Navigation, Progress},
    instances::{Instance, Location},
    semantic::{Completion, Hint, Hover, Parameter, Signature},
    types::{Snapshot, Subject},
};
