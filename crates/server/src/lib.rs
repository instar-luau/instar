//! Language server for Luau projects.

mod analysis;
mod configuration;
mod connection;
mod document;
mod documentation;
mod editing;
mod error;
mod files;
mod navigation;
mod output;
mod paths;
mod presentation;
mod project;
mod runtime;
mod service;
mod suggestions;
mod synchronization;
mod workspace;

pub use self::runtime::run;
