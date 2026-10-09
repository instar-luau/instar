use std::{collections::BTreeMap, io, path::PathBuf, time::Instant};

use instar_analysis::{Edit, Options, error::invalid};
use instar_core::{project::Project, snapshot::Snapshot};

use tower_lsp_server::{
    gen_lsp_types::{
        OptionalVersionedTextDocumentIdentifier, PositionEncodingKind, TextDocumentEdit,
        TextDocumentIdentifier, TextEdit, Uri, WorkspaceEdit,
    },
    jsonrpc::{Error, Result},
};

use crate::{location, position, session::failure, workspace::Workspace};

pub(crate) struct Edits {
    snapshot: Snapshot,
    versions: BTreeMap<PathBuf, i32>,
    locations: BTreeMap<PathBuf, Uri>,
    versioned: bool,
}

impl Edits {
    pub(crate) fn new(workspace: &Workspace) -> Self {
        Self {
            snapshot: workspace.snapshot.clone(),
            versions: workspace.documents(),
            locations: workspace.locations(),
            versioned: workspace.versioned_edits,
        }
    }

    pub(crate) fn render(
        &self,
        workspace: &Workspace,
        edits: BTreeMap<PathBuf, Vec<TextEdit>>,
    ) -> Result<WorkspaceEdit> {
        if workspace.snapshot.revision() != self.snapshot.revision() {
            return Err(Error::content_modified());
        }

        let mut changes = BTreeMap::new();
        let mut documents = Vec::new();

        for (path, edits) in edits {
            let uri = self
                .locations
                .get(&path)
                .cloned()
                .map_or_else(|| location::uri(&path).map_err(|error| failure(&error)), Ok)?;

            workspace.editable(&uri)?;

            if self.versioned {
                documents.push(TextDocumentEdit {
                    text_document: OptionalVersionedTextDocumentIdentifier {
                        text_document_identifier: TextDocumentIdentifier { uri },
                        version: self.versions.get(&path).copied(),
                    },
                    edits: edits.into_iter().map(Into::into).collect(),
                });
            } else {
                changes.insert(uri, edits);
            }
        }

        Ok(if self.versioned {
            WorkspaceEdit {
                document_changes: Some(documents.into_iter().map(Into::into).collect()),
                ..WorkspaceEdit::default()
            }
        } else {
            WorkspaceEdit {
                changes: Some(changes.into_iter().collect()),
                ..WorkspaceEdit::default()
            }
        })
    }
}

pub(crate) fn convert(
    mut edits: Vec<Edit<PathBuf>>,
    project: &mut Project,
    encoding: &PositionEncodingKind,
    options: &Options,
    started: Instant,
) -> io::Result<BTreeMap<PathBuf, Vec<TextEdit>>> {
    edits.sort_by(|left, right| {
        left.location
            .module
            .cmp(&right.location.module)
            .then_with(|| left.location.range.cmp(&right.location.range))
    });

    if edits.windows(2).any(|pair| {
        pair[0].location.module == pair[1].location.module
            && pair[0].location.range[1] > pair[1].location.range[0]
    }) {
        return Err(invalid("workspace edits overlap"));
    }

    let mut rendered = BTreeMap::<PathBuf, Vec<TextEdit>>::new();

    for edit in edits {
        options.check(started)?;
        let source = project.source(&edit.location.module)?;

        if source.revision != edit.location.revision
            || source
                .text
                .get(edit.location.range[0]..edit.location.range[1])
                .is_none()
        {
            return Err(invalid("workspace edit source revision mismatch"));
        }

        rendered
            .entry(edit.location.module)
            .or_default()
            .push(TextEdit {
                range: position::range(&source.text, edit.location.range, encoding)?,
                new_text: edit.text,
            });
    }

    Ok(rendered)
}
