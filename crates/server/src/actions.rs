use std::{collections::BTreeMap, io, path::PathBuf};

use instar_core::actions::Kind;

use tower_lsp_server::{
    gen_lsp_types::{CodeAction, CodeActionKind, CodeActionParams, TextEdit},
    jsonrpc::Result,
};

use crate::{
    diagnostics,
    edits::{self, Edits},
    position,
    session::Session,
};

struct Planned {
    action: CodeAction,
    edits: BTreeMap<PathBuf, Vec<TextEdit>>,
}

impl Session {
    pub(crate) async fn actions(&self, parameters: CodeActionParams) -> Result<Vec<CodeAction>> {
        let (rendering, locations) = {
            let workspace = self.workspace.lock().expect("workspace lock");
            workspace.editable(&parameters.text_document.uri)?;

            (Edits::new(&workspace), workspace.locations())
        };

        let kinds = [
            Kind::QuickFix,
            Kind::FixAll,
            Kind::OrganizeImports,
            Kind::Extract,
        ]
        .into_iter()
        .filter(|candidate| {
            parameters.context.only.as_ref().is_none_or(|only| {
                only.iter().any(|requested| {
                    let kind = kind(*candidate);

                    kind.as_str() == requested.as_str()
                        || kind
                            .as_str()
                            .starts_with(&format!("{}.", requested.as_str()))
                })
            })
        })
        .collect::<Vec<_>>();

        let workers = if kinds.iter().all(|kind| *kind == Kind::OrganizeImports) {
            &self.workers
        } else {
            &self.queries
        };

        let planned = self
            .query(
                &parameters.text_document.uri,
                workers,
                move |path, encoding, project, options, started| {
                    let source = project.source(path)?;

                    let offset = |position_value| {
                        position::offset(&source.text, position_value, encoding).map_err(|error| {
                            io::Error::new(io::ErrorKind::InvalidInput, error.to_string())
                        })
                    };

                    let range = [
                        offset(parameters.range.start)?,
                        offset(parameters.range.end)?,
                    ];

                    let entries =
                        project.entries(&[path.to_path_buf()], &options.remaining(started))?;

                    let [entry] = entries.as_slice() else {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidInput,
                            "code actions require one module context",
                        ));
                    };

                    let mut planned = Vec::new();

                    for action in
                        project.actions(entry, range, &kinds, &options.remaining(started))?
                    {
                        options.check(started)?;
                        let kind = kind(action.kind);

                        let mut findings = action
                            .diagnostics
                            .into_iter()
                            .map(|finding| diagnostics::convert(finding, project, encoding))
                            .collect::<io::Result<Vec<_>>>()?;

                        diagnostics::remap(&locations, &mut findings);

                        planned.push(Planned {
                            action: CodeAction {
                                title: action.title,
                                kind: Some(kind),
                                diagnostics: (!findings.is_empty()).then_some(findings),
                                ..CodeAction::default()
                            },
                            edits: edits::convert(
                                action.edits,
                                project,
                                encoding,
                                options,
                                started,
                            )?,
                        });
                    }

                    Ok(planned)
                },
            )
            .await?;

        let workspace = self.workspace.lock().expect("workspace lock");

        planned
            .into_iter()
            .map(|mut planned| {
                planned.action.edit = Some(rendering.render(&workspace, planned.edits)?);

                Ok(planned.action)
            })
            .collect()
    }
}

fn kind(kind: Kind) -> CodeActionKind {
    match kind {
        Kind::QuickFix => "quickfix",
        Kind::FixAll => "source.fixAll.instar",
        Kind::OrganizeImports => "source.organizeImports",
        Kind::Extract => "refactor.extract",
    }
    .to_owned()
    .into()
}
