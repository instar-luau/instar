use std::collections::HashSet;

use instar_core::{settings::lint::Severity, workspace::Origin};

use tower_lsp_server::gen_lsp_types::{
    Code, Diagnostic, DiagnosticRelatedInformation, DiagnosticSeverity, Location, Message,
    Position, PublishDiagnosticsParams, Range, Uri,
};

pub(crate) fn native(range: Range, code: i32, message: String) -> Diagnostic {
    Diagnostic {
        range,
        severity: Some(DiagnosticSeverity::Error),
        code: Some(Code::Int(code)),
        source: Some("luau".to_owned()),
        message: Message::String(message),
        ..Diagnostic::default()
    }
}

pub(crate) fn finding(
    range: Range,
    rule: String,
    severity: Severity,
    message: String,
) -> Diagnostic {
    let severity = match severity {
        Severity::Error => DiagnosticSeverity::Error,
        Severity::Warning => DiagnosticSeverity::Warning,
        Severity::Information => DiagnosticSeverity::Information,
        Severity::Hint => DiagnosticSeverity::Hint,
    };

    Diagnostic {
        range,
        severity: Some(severity),
        code: Some(Code::String(rule)),
        source: Some("instar".to_owned()),
        message: Message::String(message),
        ..Diagnostic::default()
    }
}

pub(crate) fn error(range: Range, code: &str, message: String) -> Diagnostic {
    Diagnostic {
        range,
        severity: Some(DiagnosticSeverity::Error),
        code: Some(Code::String(code.to_owned())),
        source: Some("instar".to_owned()),
        message: Message::String(message),
        ..Diagnostic::default()
    }
}

pub(crate) fn conflict(target: &str, position: Position, message: &str) -> String {
    format!(
        "Invalid environment at `{target}:{}:{}`. {message}",
        position.line + 1,
        position.character + 1,
    )
}

pub(crate) fn environment(
    origin: &Origin,
    position: Position,
    message: &str,
    related: Option<(Uri, Range)>,
) -> Diagnostic {
    let origin = match origin {
        Origin::File(path) => path.display().to_string(),
        Origin::Builtins => "<builtins>".to_owned(),
    };

    let mut diagnostic = error(
        Range::default(),
        "syntax",
        format!(
            "Invalid lint environment at `{origin}:{}:{}`. {message}",
            position.line + 1,
            position.character + 1,
        ),
    );

    diagnostic.related_information = related.map(|(uri, range)| {
        vec![DiagnosticRelatedInformation {
            location: Location { uri, range },
            message: message.to_owned(),
        }]
    });

    diagnostic
}

pub(crate) fn publication(
    uri: Uri,
    version: Option<i32>,
    mut diagnostics: Vec<Diagnostic>,
) -> PublishDiagnosticsParams {
    let mut unique = HashSet::new();
    diagnostics.retain(|diagnostic| unique.insert(diagnostic.clone()));

    PublishDiagnosticsParams {
        uri,
        version,
        diagnostics,
    }
}
