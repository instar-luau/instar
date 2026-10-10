use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
};

use tower_lsp_server::gen_lsp_types::{
    ApplyWorkspaceEditResult, PositionEncodingKind, Registration, Uri,
};

use super::{capabilities::Capabilities, documents::Document, identities::Identities};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Lifecycle {
    Uninitialized,
    Initialized,
    Shutdown,
    Exited,
    Disconnected,
}

pub(crate) struct State {
    pub(crate) lifecycle: Lifecycle,
    pub(crate) capabilities: Capabilities,
    pub(crate) encoding: PositionEncodingKind,
    pub(crate) workspace_folders: BTreeSet<PathBuf>,
    pub(crate) documents: BTreeMap<Uri, Document>,
    pub(crate) registrations: BTreeMap<String, Registration>,
    pub(crate) published: BTreeSet<Uri>,
    pub(crate) identities: Identities,
    pub(crate) edit_outcomes: Vec<ApplyWorkspaceEditResult>,
    pub(crate) shutdown_requested: bool,
    pub(crate) exit_received: bool,
}

impl State {
    pub(crate) fn new(session: u64) -> Self {
        Self {
            lifecycle: Lifecycle::Uninitialized,
            capabilities: Capabilities::default(),
            encoding: PositionEncodingKind::UTF16,
            workspace_folders: BTreeSet::new(),
            documents: BTreeMap::new(),
            registrations: BTreeMap::new(),
            published: BTreeSet::new(),
            identities: Identities::new(session),
            edit_outcomes: Vec::new(),
            shutdown_requested: false,
            exit_received: false,
        }
    }
}
