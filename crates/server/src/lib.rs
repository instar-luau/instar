//! Language server for Luau projects.

mod configuration;
mod connection;
mod diagnostics;
mod document;
mod editing;
mod files;
mod native;
mod navigation;
mod presentation;
mod progress;
mod project;
mod runtime;
mod service;
mod suggestions;
mod synchronization;
mod workspace;

pub use self::runtime::run;
