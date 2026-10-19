use crate::{analysis::Error, syntax};

/// Validates complete source using the strict syntax policy shared by linting and formatting.
///
/// # Errors
///
/// Returns the first syntax diagnostic or incomplete node with its original source byte range.
pub fn validate(source: &str) -> Result<(), Error> {
    let prefix = usize::from(source.starts_with('\u{feff}')) * 3;

    syntax::parse(&source.as_bytes()[prefix..], |range, message| Error {
        range: range.start + prefix..range.end + prefix,
        message: message.unwrap_or("incomplete syntax").to_owned(),
    })
    .map(|_| ())
}
