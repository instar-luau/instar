use std::{path::PathBuf, sync::Arc};

use instar_shared::{ContextIdentity, SourceIdentity, SourceVersion, Span};

use crate::{
    configuration::ProjectSettingsPatch,
    inputs::{InputVersion, SourceOrigin},
};

/// Ordered state changes committed as one snapshot transition.
#[derive(Clone, Debug, Default)]
pub struct ChangeBatch {
    /// Changes applied in order; failure leaves the original snapshot intact.
    pub changes: Vec<ProjectChange>,
}

/// One source, configuration, input, or worker-state change.
#[derive(Clone, Debug)]
pub enum ProjectChange {
    /// Changes captured source contents or origin.
    Source(SourceChange),

    /// Adds and removes workspace roots.
    WorkspaceRoots {
        /// Roots to register.
        added: Vec<PathBuf>,
        /// Roots to unregister along with their client configuration.
        removed: Vec<PathBuf>,
    },

    /// Replaces a context's client configuration patch.
    ClientConfiguration {
        /// Project context receiving the patch.
        context: ContextIdentity,
        /// Replacement patch; absent removes the existing client patch.
        patch: Option<Box<ProjectSettingsPatch>>,
    },

    /// Updates a consulted input observation under an expected-value guard.
    Input {
        /// Required previous observation; absent requires no prior observation.
        expected: Option<InputVersion>,
        /// Replacement observation.
        current: InputVersion,
    },

    /// Invalidates worker-backed state after an isolated worker is discarded.
    WorkerDiscarded(WorkerAffinity),
}

/// Revision-guarded changes to the captured source store.
#[derive(Clone, Debug)]
pub enum SourceChange {
    /// Registers source text and its authority or filesystem origin.
    Open {
        /// Identity assigned to the source.
        identity: SourceIdentity,
        /// Authority and context associated with the source.
        origin: SourceOrigin,
        /// Exact initial contents.
        text: Arc<str>,
    },

    /// Replaces source text and advances its revision.
    Replace {
        /// Source revision required before replacement.
        expected: SourceVersion,
        /// Exact replacement contents.
        text: Arc<str>,
    },

    /// Applies sequential text edits and advances the source revision.
    Incremental {
        /// Source revision required before the first edit.
        expected: SourceVersion,
        /// Edits applied in order, each relative to the preceding edited text.
        edits: Vec<SourceEdit>,
    },

    /// Changes a source's origin without modifying its text.
    Move {
        /// Source revision required before relocation.
        expected: SourceVersion,
        /// Replacement authority and context.
        origin: SourceOrigin,
    },

    /// Releases synchronized document contents while retaining its identity.
    Close {
        /// Synchronized document revision required before closing.
        expected: SourceVersion,
    },

    /// Removes a captured source record.
    Remove {
        /// Source revision required before removal.
        expected: SourceVersion,
    },
}

/// One text replacement in a sequential incremental source change.
#[derive(Clone, Debug)]
pub struct SourceEdit {
    /// UTF-8 byte range in the current intermediate text.
    pub span: Span,

    /// Text replacing the range.
    pub replacement: String,
}

/// Identity of an isolated worker lane and its generation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WorkerAffinity {
    /// Isolated worker lane identifier.
    pub lane: u64,

    /// Generation of that lane's worker.
    pub generation: u64,
}
