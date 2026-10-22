use std::ops;

use crate::{
    CompletionKind, HintKind, Parentheses, Range, SourceLocation, bridge::bindings, error::Error,
};

/// Luau-printed type or declaration text.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Presentation {
    /// Single-line editor text.
    pub compact: String,

    /// Expanded editor text.
    pub expanded: String,

    /// Whether the printer produced complete, valid type information.
    pub complete: bool,
}

/// Owned semantic hover information.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Hover {
    /// Exact selected source component.
    pub location: SourceLocation,

    /// Printed declaration or selected type.
    pub presentation: String,

    /// Native documentation catalog symbol.
    pub documentation_symbol: String,

    /// Canonical overload signature used for documentation lookup.
    pub documentation_signature: String,

    /// Whether native resolution selected this overload at the hovered call.
    pub selected: bool,

    /// Whether the presentation names a documented library or constructor namespace.
    pub namespace: bool,

    /// Whether the method signature has an implicit receiver.
    pub receiver: bool,
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

    /// Available printed type.
    pub presentation: Option<String>,

    /// Request-local semantic equivalence group, or zero when unproven.
    pub equivalence: usize,

    /// Native documentation catalog symbol.
    pub documentation_symbol: String,

    /// Exact insertion text, never presentation text.
    pub insert_text: String,

    /// Recommended call parentheses following the insertion text.
    pub parentheses: Parentheses,

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

    /// Exact byte range in the containing signature label.
    pub range: ops::Range<usize>,

    /// Original native argument index for documentation lookup.
    pub source_index: u32,

    /// Whether this is the visible variadic tail.
    pub variadic: bool,
}

/// One native overload with its visible parameter sequence.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Signature {
    /// Printed single-line signature.
    pub label: String,

    /// Expanded signature text.
    pub expanded: String,

    /// Native documentation catalog symbol.
    pub documentation_symbol: String,

    /// Canonical overload signature used for documentation lookup.
    pub documentation_signature: String,

    /// Visible parameters with the colon receiver removed exactly once.
    pub parameters: Vec<Parameter>,

    /// Active visible argument.
    pub active_parameter: u32,

    /// Whether native overload resolution selected this overload.
    pub selected: bool,
}

/// An inferred inlay subject and its printed type.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Hint {
    /// Zero-width insertion location and native identity.
    pub location: SourceLocation,

    /// Selected call parameter name, empty for type and return hints.
    pub name: String,

    /// Printed binding type, return pack, or selected call parameter.
    pub presentation: Presentation,

    /// Inlay subject category.
    pub kind: HintKind,

    /// Whether the source makes the information obvious.
    pub obvious: bool,

    /// Selected subject's documentation symbol.
    pub documentation_symbol: String,

    /// Canonical callable signature for parameter documentation lookup.
    pub documentation_signature: String,

    /// Original callable parameter index for metadata lookup.
    pub parameter_index: Option<u32>,
}

impl TryFrom<bindings::NativePresentation> for Presentation {
    type Error = Error;

    fn try_from(value: bindings::NativePresentation) -> Result<Self, Error> {
        validate_text(&value.compact)?;
        validate_text(&value.expanded)?;

        if value.compact.contains(['\n', '\r']) {
            return Err(Error::Native {
                reason: "Invalid compact presentation.".into(),
            });
        }

        Ok(Self {
            compact: value.compact,
            expanded: value.expanded,
            complete: value.is_complete,
        })
    }
}

impl TryFrom<bindings::NativeHover> for Hover {
    type Error = Error;

    fn try_from(value: bindings::NativeHover) -> Result<Self, Error> {
        validate_text(&value.presentation)?;

        let range = value.location.range;

        if (range.start.line, range.start.column) >= (range.end.line, range.end.column) {
            return Err(Error::Native {
                reason: "Invalid hover component range.".into(),
            });
        }

        Ok(Self {
            location: value.location,
            presentation: value.presentation,
            documentation_symbol: value.documentation_symbol,
            documentation_signature: value.documentation_signature,
            selected: value.is_selected,
            namespace: value.is_namespace,
            receiver: value.has_receiver,
        })
    }
}

impl TryFrom<bindings::NativeCompletion> for Completion {
    type Error = Error;

    fn try_from(value: bindings::NativeCompletion) -> Result<Self, Error> {
        if value.presentations.len() > 1 {
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

        if !matches!(
            value.parentheses,
            Parentheses::None | Parentheses::CursorAfter | Parentheses::CursorInside
        ) {
            return Err(Error::Native {
                reason: "Invalid completion parentheses recommendation.".into(),
            });
        }

        let presentation = value.presentations.into_iter().next();

        if value.equivalence != 0 && (presentation.is_none() || !value.is_complete) {
            return Err(Error::Native {
                reason: "Invalid completion type identity.".into(),
            });
        }

        if let Some(presentation) = &presentation {
            validate_text(presentation)?;

            if presentation.contains(['\n', '\r']) {
                return Err(Error::Native {
                    reason: "Invalid compact presentation.".into(),
                });
            }
        }

        Ok(Self {
            label: value.label,
            kind: value.kind,
            is_function: value.is_function,
            presentation,
            equivalence: value.equivalence,
            documentation_symbol: value.documentation_symbol,
            insert_text: value.insert_text,
            parentheses: value.parentheses,
            replacement: value.replacement,
            operation: value.replace_dot.then_some(value.operation),
            deprecated: value.is_deprecated,
        })
    }
}

impl TryFrom<bindings::NativeSignature> for Signature {
    type Error = Error;

    fn try_from(value: bindings::NativeSignature) -> Result<Self, Error> {
        validate_text(&value.label)?;
        validate_text(&value.expanded)?;

        if value.label.contains(['\n', '\r']) {
            return Err(Error::Native {
                reason: "Invalid signature label.".into(),
            });
        }

        let parameters = value
            .parameters
            .into_iter()
            .map(|parameter| {
                let range = parameter.start..parameter.end;

                if range.is_empty() || value.label.get(range.clone()).is_none() {
                    return Err(Error::Native {
                        reason: "Invalid signature parameter span.".into(),
                    });
                }

                Ok(Parameter {
                    name: parameter.name,
                    range,
                    source_index: parameter.source_index,
                    variadic: parameter.is_variadic,
                })
            })
            .collect::<Result<Vec<_>, Error>>()?;

        if parameters
            .first()
            .is_some_and(|parameter| parameter.source_index > 1)
            || parameters.windows(2).any(|pair| {
                pair[0].variadic
                    || pair[0].range.end > pair[1].range.start
                    || pair[0].source_index.checked_add(1) != Some(pair[1].source_index)
            })
        {
            return Err(Error::Native {
                reason: "Invalid signature parameter sequence.".into(),
            });
        }

        Ok(Self {
            label: value.label,
            expanded: value.expanded,
            documentation_symbol: value.documentation_symbol,
            documentation_signature: value.documentation_signature,
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

        if (value.kind == HintKind::Type && value.presentation.is_pack)
            || (value.kind == HintKind::Return && !value.presentation.is_pack)
        {
            return Err(Error::Native {
                reason: "Invalid inlay type subject.".into(),
            });
        }

        Ok(Self {
            location: value.location,
            name: value.name,
            presentation: value.presentation.try_into()?,
            kind: value.kind,
            obvious: value.is_obvious,
            documentation_symbol: value.documentation_symbol,
            documentation_signature: value.documentation_signature,
            parameter_index: value.has_parameter_index.then_some(value.parameter_index),
        })
    }
}

fn validate_text(value: &str) -> Result<(), Error> {
    if value.is_empty() || value.len() > 32 * 1024 || value.lines().count() > 256 {
        return Err(Error::Native {
            reason: "Invalid native presentation bounds.".into(),
        });
    }

    Ok(())
}
