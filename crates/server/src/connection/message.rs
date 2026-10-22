use std::sync::Arc;

use instar_bridge::Frontend;
use tower_lsp_server::{gen_lsp_types::Uri, jsonrpc::Result};

use super::operation::Operation;

use crate::{
    project::{Feedback, Project, Snapshot},
    workspace::{History, State},
};

type Source = Box<dyn FnOnce(Result<&mut Project>) + Send>;
type Control = Box<dyn FnOnce(&mut State) + Send>;
pub(super) type Delivery = Box<dyn FnOnce(Result<()>) + Send>;

pub(super) type Action =
    Box<dyn FnOnce(Result<(&mut Project, &mut Frontend<'_>)>) -> (bool, Delivery) + Send>;

pub(crate) enum Target {
    Uri(Uri),
    Snapshot(Box<Snapshot>),
}

pub(super) struct Work {
    pub(super) identifier: u64,
    pub(super) snapshot: Snapshot,
    pub(super) history: History,
    pub(super) operation: Arc<Operation>,
    pub(super) action: Action,
}

pub(super) struct Completion {
    pub(super) identifier: u64,
    pub(super) feedback: Option<Feedback>,
    pub(super) is_successful: bool,
    pub(super) deliver: Delivery,
}

pub(super) enum Message {
    Source {
        uri: Uri,
        operation: Option<Arc<Operation>>,
        execute: Source,
    },

    Query {
        target: Target,
        operation: Arc<Operation>,
        action: Action,
    },

    Control(Control),
    Complete(Completion),
    Stop,
}
