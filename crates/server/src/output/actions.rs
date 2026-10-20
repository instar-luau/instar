use tower_lsp_server::gen_lsp_types::{
    CodeAction, CodeActionKind, CodeActionResponse, Diagnostic, WorkspaceEdit,
};

pub(crate) fn quickfix(
    message: &str,
    edit: WorkspaceEdit,
    diagnostics: Vec<Diagnostic>,
) -> CodeActionResponse {
    offer(format!("Fix: {message}"), "quickfix", edit, diagnostics)
}

pub(crate) fn fix_all(edit: WorkspaceEdit) -> CodeActionResponse {
    offer(
        "Apply available Instar fixes".to_owned(),
        "source.fixAll.instar",
        edit,
        Vec::new(),
    )
}

pub(crate) fn organize(edit: WorkspaceEdit) -> CodeActionResponse {
    offer(
        "Organize imports".to_owned(),
        "source.organizeImports",
        edit,
        Vec::new(),
    )
}

fn offer(
    title: String,
    kind: &str,
    edit: WorkspaceEdit,
    diagnostics: Vec<Diagnostic>,
) -> CodeActionResponse {
    CodeActionResponse::CodeAction(CodeAction {
        title,
        kind: Some(CodeActionKind::from(kind.to_owned())),
        diagnostics: (!diagnostics.is_empty()).then_some(diagnostics),
        edit: Some(edit),
        ..CodeAction::default()
    })
}
