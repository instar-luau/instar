//! Luau source formatting.

pub mod configuration;

mod formatter;

pub use configuration::Configuration;

/// Formats Luau source using the supplied configuration.
///
/// # Errors
/// Returns an error when the configuration or source syntax is invalid.
pub fn format(source: &[u8], configuration: &Configuration) -> Result<Vec<u8>, String> {
    formatter::format(source, configuration)
}
