use crate::{
    CompletionKind, HintKind, HoverKind, Range, SourceLocation,
    bridge::bindings,
    error::Error,
    types::{Snapshot, Subject},
};

/// Owned semantic hover information.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Hover {
    /// Exact selected source component.
    pub location: SourceLocation,

    /// Original identifier or string-key bytes when available.
    pub symbol: Vec<u8>,

    /// Declaration category of the selected source component.
    pub kind: HoverKind,

    /// Selected type or pack.
    pub snapshot: Snapshot,

    /// Native documentation catalog symbol.
    pub documentation_symbol: String,

    /// Existing documentation text.
    pub documentation: String,
}

/// An owned native completion candidate.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Completion {
    /// Candidate name.
    pub label: String,

    /// Candidate category.
    pub kind: CompletionKind,

    /// Whether the candidate is callable.
    pub is_function: bool,

    /// Available semantic type.
    pub snapshot: Option<Snapshot>,

    /// Native documentation catalog symbol.
    pub documentation_symbol: String,

    /// Existing documentation text.
    pub documentation: String,

    /// Exact insertion text, never presentation text.
    pub insert_text: String,

    /// Whether insertion uses snippet placeholders.
    pub snippet: bool,

    /// Source replacement range.
    pub replacement: SourceLocation,

    /// Range of a preceding dot to replace with a colon.
    pub operation: Option<Range>,

    /// Whether the candidate is deprecated.
    pub deprecated: bool,
}

/// One visible call parameter.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Parameter {
    /// Argument name when available.
    pub name: String,

    /// Type or pack in the containing signature's snapshot.
    pub subject: Subject,

    /// Original native argument index for documentation lookup.
    pub source_index: u32,

    /// Existing parameter documentation.
    pub documentation: String,

    /// Whether this is the visible variadic tail.
    pub variadic: bool,
}

/// One native overload with an authoritative visible parameter sequence.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Signature {
    /// Native module identity of the call.
    pub identity: String,

    /// Function graph, shared by its parameter subjects.
    pub snapshot: Snapshot,

    /// Native documentation catalog symbol.
    pub documentation_symbol: String,

    /// Existing function documentation.
    pub documentation: String,

    /// Visible parameters with the colon receiver removed exactly once.
    pub parameters: Vec<Parameter>,

    /// Active visible argument.
    pub active_parameter: u32,

    /// Whether native overload resolution selected this overload.
    pub selected: bool,
}

/// A semantic inlay subject, independent of its rendered label.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Hint {
    /// Zero-width insertion location and native identity.
    pub location: SourceLocation,

    /// Selected call parameter name, empty for type and return hints.
    pub name: String,

    /// Binding type, return pack, or selected call parameter.
    pub snapshot: Snapshot,

    /// Inlay subject category.
    pub kind: HintKind,

    /// Whether the source makes the information obvious.
    pub obvious: bool,

    /// Selected subject's documentation symbol.
    pub documentation_symbol: String,

    /// Existing documentation text.
    pub documentation: String,

    /// Original callable parameter index for metadata lookup.
    pub parameter_index: Option<u32>,
}

impl TryFrom<bindings::NativeHover> for Hover {
    type Error = Error;

    fn try_from(value: bindings::NativeHover) -> Result<Self, Error> {
        let range = value.location.range;

        if (range.start.line, range.start.column) >= (range.end.line, range.end.column) {
            return Err(Error::Native {
                reason: "Invalid hover component range.".into(),
            });
        }

        match value.kind {
            HoverKind::Local
            | HoverKind::Global
            | HoverKind::Alias
            | HoverKind::Field
            | HoverKind::LocalFunction
            | HoverKind::Function
            | HoverKind::Parameter
            | HoverKind::Loop
            | HoverKind::Constant
            | HoverKind::ConstantFunction
            | HoverKind::Type => {}

            _ => {
                return Err(Error::Native {
                    reason: "Unknown hover subject kind.".into(),
                });
            }
        }

        Ok(Self {
            location: value.location,
            symbol: value.symbol,
            kind: value.kind,
            snapshot: value.snapshot.try_into()?,
            documentation_symbol: value.documentation_symbol,
            documentation: value.documentation,
        })
    }
}

impl TryFrom<bindings::NativeCompletion> for Completion {
    type Error = Error;

    fn try_from(value: bindings::NativeCompletion) -> Result<Self, Error> {
        if value.snapshots.len() > 1 {
            return Err(Error::Native {
                reason: "Invalid completion presentation cardinality.".into(),
            });
        }

        if !matches!(
            value.kind,
            CompletionKind::Property
                | CompletionKind::Binding
                | CompletionKind::Keyword
                | CompletionKind::String
                | CompletionKind::Type
                | CompletionKind::Module
                | CompletionKind::GeneratedFunction
                | CompletionKind::RequirePath
                | CompletionKind::HotComment
        ) {
            return Err(Error::Native {
                reason: "Invalid completion category.".into(),
            });
        }

        let snapshot = value
            .snapshots
            .into_iter()
            .next()
            .map(Snapshot::try_from)
            .transpose()?;

        if snapshot
            .as_ref()
            .is_some_and(|snapshot| !matches!(snapshot.root, Subject::Type(_)))
        {
            return Err(Error::Native {
                reason: "Invalid completion type subject.".into(),
            });
        }

        Ok(Self {
            label: value.label,
            kind: value.kind,
            is_function: value.is_function,
            snapshot,
            documentation_symbol: value.documentation_symbol,
            documentation: value.documentation,
            insert_text: value.insert_text,
            snippet: value.is_snippet,
            replacement: value.replacement,
            operation: value.replace_dot.then_some(value.operation),
            deprecated: value.is_deprecated,
        })
    }
}

impl TryFrom<bindings::NativeSignature> for Signature {
    type Error = Error;

    fn try_from(value: bindings::NativeSignature) -> Result<Self, Error> {
        let snapshot = Snapshot::try_from(value.snapshot)?;

        if !matches!(snapshot.root, Subject::Type(_)) {
            return Err(Error::Native {
                reason: "Invalid signature type subject.".into(),
            });
        }

        let parameters = value
            .parameters
            .into_iter()
            .map(|parameter| {
                let subject = if parameter.is_pack {
                    Subject::Pack(parameter.subject)
                } else {
                    Subject::Type(parameter.subject)
                };

                let is_valid = match subject {
                    Subject::Type(index) => index < snapshot.types.len(),
                    Subject::Pack(index) => index < snapshot.packs.len(),
                };

                if !is_valid || parameter.is_pack != parameter.is_variadic {
                    return Err(Error::Native {
                        reason: "Invalid signature parameter subject.".into(),
                    });
                }

                Ok(Parameter {
                    name: parameter.name,
                    subject,
                    source_index: parameter.source_index,
                    documentation: parameter.documentation,
                    variadic: parameter.is_variadic,
                })
            })
            .collect::<Result<Vec<_>, Error>>()?;

        if parameters
            .first()
            .is_some_and(|parameter| parameter.source_index > 1)
            || parameters.windows(2).any(|pair| {
                pair[0].variadic
                    || pair[0].source_index.checked_add(1) != Some(pair[1].source_index)
            })
        {
            return Err(Error::Native {
                reason: "Invalid signature parameter sequence.".into(),
            });
        }

        Ok(Self {
            identity: value.identity,
            snapshot,
            documentation_symbol: value.documentation_symbol,
            documentation: value.documentation,
            parameters,
            active_parameter: value.active_parameter,
            selected: value.is_selected,
        })
    }
}

impl TryFrom<bindings::NativeHint> for Hint {
    type Error = Error;

    fn try_from(value: bindings::NativeHint) -> Result<Self, Error> {
        if !matches!(
            value.kind,
            HintKind::Type | HintKind::Return | HintKind::Parameter
        ) {
            return Err(Error::Native {
                reason: "Invalid inlay subject kind.".into(),
            });
        }

        if value.has_parameter_index != (value.kind == HintKind::Parameter)
            || (!value.has_parameter_index && value.parameter_index != 0)
            || value.location.range.start != value.location.range.end
        {
            return Err(Error::Native {
                reason: "Invalid inlay parameter metadata.".into(),
            });
        }

        let snapshot = Snapshot::try_from(value.snapshot)?;

        if (value.kind == HintKind::Type && !matches!(snapshot.root, Subject::Type(_)))
            || (value.kind == HintKind::Return && !matches!(snapshot.root, Subject::Pack(_)))
        {
            return Err(Error::Native {
                reason: "Invalid inlay type subject.".into(),
            });
        }

        Ok(Self {
            location: value.location,
            name: value.name,
            snapshot,
            kind: value.kind,
            obvious: value.is_obvious,
            documentation_symbol: value.documentation_symbol,
            documentation: value.documentation,
            parameter_index: value.has_parameter_index.then_some(value.parameter_index),
        })
    }
}
