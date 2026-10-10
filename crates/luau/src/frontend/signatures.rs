use std::io;

use instar_analysis::Location;
use serde::{Deserialize, Serialize};

use super::result::location;
use crate::boundary;

/// One native callable signature.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct Signature {
    /// Complete native signature label.
    pub label: String,

    /// UTF-8 byte ranges of parameter labels within the signature label.
    pub parameters: Vec<[usize; 2]>,

    /// Whether the final parameter accepts a variadic argument pack.
    pub variadic: bool,
}

/// Signatures for a call in an exact source revision.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct Signatures<Module = String> {
    /// Source call range.
    pub location: Location<Module>,

    /// Callable alternatives, including the selected specialization when available.
    pub entries: Vec<Signature>,

    /// Native selected overload or the sole callable alternative.
    pub active: Option<usize>,

    /// Zero-based explicit argument at the cursor.
    pub argument: usize,

    /// Native documentation identity of the called function.
    pub documentation: Option<String>,
}

impl TryFrom<boundary::NativeSignatures> for Signatures {
    type Error = io::Error;

    fn try_from(value: boundary::NativeSignatures) -> io::Result<Self> {
        let entries = value
            .entries
            .into_iter()
            .map(|entry| {
                let parameters = entry
                    .parameters
                    .into_iter()
                    .map(|parameter| [parameter.start, parameter.end])
                    .collect::<Vec<_>>();

                let mut previous = 0;

                for &[start, end] in &parameters {
                    if start < previous
                        || start >= end
                        || !entry.label.is_char_boundary(start)
                        || !entry.label.is_char_boundary(end)
                    {
                        return Err(io::Error::other("invalid native signature parameter range"));
                    }

                    previous = end;
                }

                Ok(Signature {
                    label: entry.label,
                    parameters,
                    variadic: entry.variadic,
                })
            })
            .collect::<io::Result<Vec<_>>>()?;

        if entries.is_empty() || (value.has_active && value.active >= entries.len()) {
            return Err(io::Error::other("invalid native active signature"));
        }

        Ok(Self {
            location: location(value.location),
            entries,
            active: value.has_active.then_some(value.active),
            argument: value.argument,
            documentation: (!value.documentation.is_empty()).then_some(value.documentation),
        })
    }
}
