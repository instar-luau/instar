//! Isolated, validated syntax-only Luau formatting.

mod client;
mod layout;
mod request;
mod result;
pub mod settings;
mod worker;

pub use client::Formatter;
pub use request::{FormatInput, FormatScope};
pub use result::{FormatOutcome, FormatResult, FormatSelection, SelectionOutcome, ValidatedFormat};
pub use settings::{FormatOverrides, FormatSettings};
pub use worker::run_worker;
