use std::io;

use instar_analysis::Location;
use serde::{Deserialize, Serialize};

use super::result::location;
use crate::boundary;

/// Namespace distinguishing declarations with different language roles.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub enum Namespace {
    /// Value binding.
    Binding,

    /// Type declaration.
    Type,

    /// Named member.
    Property,

    /// Implicit method receiver.
    Receiver,
}

/// Native semantic role of an occurrence.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub enum Role {
    /// Value binding.
    Variable,

    /// Function parameter or implicit receiver.
    Parameter,

    /// Function-valued binding.
    Function,

    /// Named member.
    Property,

    /// Function-valued member.
    Method,

    /// Type declaration or reference.
    Type,
}

/// Native presentation metadata independent of declaration identity.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct Classification {
    /// Semantic role at this occurrence.
    pub role: Role,

    /// Whether native metadata marks the occurrence as deprecated.
    pub deprecated: bool,
}

/// A source occurrence and its native declaration identities.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct Reference<Module = String> {
    /// Exact occurrence range.
    pub location: Location<Module>,

    /// Native declarations identifying this occurrence.
    pub targets: Vec<Location<Module>>,

    /// Native builtin identity when no source declaration is available.
    pub symbol: Option<String>,

    /// Decoded identifier or member name.
    pub name: String,

    /// Language namespace containing the referenced declaration.
    pub namespace: Namespace,

    /// Whether this occurrence declares the name.
    pub declaration: bool,

    /// Whether this occurrence writes the binding or member.
    pub write: bool,

    /// Whether the occurrence is represented by a string literal.
    pub literal: bool,

    /// Native role and presentation metadata.
    pub classification: Classification,
}

impl TryFrom<boundary::NativeReference> for Reference {
    type Error = io::Error;

    fn try_from(value: boundary::NativeReference) -> io::Result<Self> {
        let namespace = match value.kind {
            0 => Namespace::Binding,
            1 => Namespace::Type,
            2 => Namespace::Property,
            3 => Namespace::Receiver,
            _ => return Err(io::Error::other("unknown native reference namespace")),
        };

        if value.name.is_empty() || (value.targets.is_empty() && value.documentation.is_empty()) {
            return Err(io::Error::other("native reference identity is missing"));
        }

        let role = match namespace {
            Namespace::Type => Role::Type,
            Namespace::Receiver => Role::Parameter,
            Namespace::Property if value.function => Role::Method,
            Namespace::Property => Role::Property,
            Namespace::Binding if value.parameter => Role::Parameter,
            Namespace::Binding if value.function => Role::Function,
            Namespace::Binding => Role::Variable,
        };

        Ok(Self {
            location: location(value.location),
            targets: value.targets.into_iter().map(location).collect(),
            symbol: (!value.documentation.is_empty()).then_some(value.documentation),
            name: value.name,
            namespace,
            declaration: value.declaration,
            write: value.write,
            literal: value.literal,
            classification: Classification {
                role,
                deprecated: value.deprecated,
            },
        })
    }
}
