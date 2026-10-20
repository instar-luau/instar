//! Language server for Luau projects.

mod analysis;
mod configuration;
mod connection;
mod document;
mod editing;
mod files;
mod native;
mod navigation;
mod output;
mod presentation;
mod project;
mod runtime;
mod service;
mod suggestions;
mod synchronization;
mod workspace;

pub use self::runtime::run;
