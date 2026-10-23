//! Persistent native Luau analysis over host-provided sources and configuration.

#[expect(
    unsafe_code,
    unreachable_pub,
    clippy::absolute_paths,
    clippy::inline_modules,
    reason = "CXX generates FFI items; Luau's cancellation token shares only an atomic boolean across threads"
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
mod language;
mod semantic;

pub use self::{
    bridge::bindings::{
        Builtin, CompletionKind, Diagnostic, Documentation, HintKind, HintSelector, Import,
        Occurrence, OccurrenceRole, Parentheses, Position, Range, SourceLocation,
    },
    cancellation::Cancellation,
    configuration::{Configuration, Mode},
    environment::{Class, Definition, Environment, Property, builtins},
    error::{Error, HostError, Operation},
    flags::Flag,
    frontend::Frontend,
    host::{Context, Host, Navigation, Progress},
    instances::{Instance, Location},
    language::Language,
    semantic::{Completion, Hint, Hover, Parameter, Presentation, Signature},
};
