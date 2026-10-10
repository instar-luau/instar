use std::path::PathBuf;

use instar_lint::RuleFix;
use instar_shared::{EditProposal, SourceVersion, Span};
use serde::Serialize;

use crate::{
    inputs::{InputVersion, ProjectSnapshot},
    queries::PositionQuery,
};

/// An editor operation over an exact project snapshot.
#[derive(Clone, Debug)]
pub enum EditRequest {
    /// Rename a source declaration and its references.
    Rename(RenameRequest),

    /// Validate selected catalog fixes together.
    Fixes(FixRequest),

    /// Organize the syntax-recognized require prologue.
    Imports(ImportRequest),

    /// Extract a supported literal into a local binding.
    Extract(ExtractionRequest),

    /// Find same-source identifier occurrences for linked editing.
    Linked(LinkedRequest),

    /// Prepare edits permitted by the effective save policy.
    Save(SaveRequest),

    /// Preflight file operations and related reference edits.
    Files(FileRequest),
}

/// A revision-anchored symbol rename.
#[derive(Clone, Debug)]
pub struct RenameRequest {
    /// Exact original sources and contexts.
    pub snapshot: ProjectSnapshot,

    /// Symbol occurrence to rename.
    pub position: PositionQuery,

    /// One non-keyword Luau identifier.
    pub name: String,

    /// Include references across the workspace.
    pub workspace: bool,
}

/// Catalog proposals selected for one nonconflicting fix batch.
#[derive(Clone, Debug)]
pub struct FixRequest {
    /// Exact original sources and contexts.
    pub snapshot: ProjectSnapshot,

    /// Producer-validated fixes to revalidate together.
    pub fixes: Vec<RuleFix>,
}

/// Syntax-only require organization for one source.
#[derive(Clone, Debug)]
pub struct ImportRequest {
    /// Exact original sources and contexts.
    pub snapshot: ProjectSnapshot,

    /// Source revision to organize.
    pub source: SourceVersion,
}

/// A selected literal extraction.
#[derive(Clone, Debug)]
pub struct ExtractionRequest {
    /// Exact original sources and contexts.
    pub snapshot: ProjectSnapshot,

    /// Source revision containing the selection.
    pub source: SourceVersion,

    /// Original UTF-8 byte selection.
    pub range: Span,
}

/// Same-source linked identifier ranges.
#[derive(Clone, Debug)]
pub struct LinkedRequest {
    /// Exact original sources and contexts.
    pub snapshot: ProjectSnapshot,

    /// Identifier occurrence to inspect.
    pub position: PositionQuery,
}

/// Save preparation using explicit effective fix and format settings.
#[derive(Clone, Debug)]
pub struct SaveRequest {
    /// Exact original sources and contexts.
    pub snapshot: ProjectSnapshot,

    /// Only source allowed to receive save edits.
    pub source: SourceVersion,
}

/// File-operation intent without filesystem mutation authority.
#[derive(Clone, Debug)]
pub struct FileRequest {
    /// Exact original sources and contexts.
    pub snapshot: ProjectSnapshot,

    /// Nonoverlapping operations to preflight together.
    pub changes: Vec<FileChange>,
}

/// A client-owned file or directory operation to preflight.
#[derive(Clone, Debug)]
pub enum FileChange {
    /// Create an absent path; future source contents are not supplied.
    Create(PathBuf),

    /// Move an existing path while retaining its resolved source identity.
    Rename {
        /// Existing source path or directory.
        old: PathBuf,
        /// Absent destination path.
        new: PathBuf,
    },

    /// Delete a path only if retained modules can keep their resolved targets.
    Delete(PathBuf),
}

/// A validated proposal and explicit requested write destinations.
#[derive(Debug)]
pub struct ApplyRequest {
    /// Capability carrying the completed validation evidence.
    pub edit: ValidatedEdit,

    /// Destinations independently authorized by the caller.
    pub destinations: Vec<WriteAuthorization>,
}

/// Caller authorization for one exact source and destination.
#[derive(Clone, Debug)]
pub struct WriteAuthorization {
    /// Original source revision whose agreement is required.
    pub source: SourceVersion,

    /// Requested filesystem destination.
    pub destination: PathBuf,
}

/// An operation-specific guarantee established by project validation.
#[derive(Clone, Debug)]
pub enum EditGuarantee {
    /// Exact revisions, valid UTF-8 boundaries and nonconflicting replacements.
    Ranges,

    /// Applicable syntax validation completed.
    Syntax,

    /// Indexed source binding relationships were preserved.
    Bindings,

    /// Original analysis completed and no new diagnostics were introduced.
    Diagnostics,

    /// Retained resolved loader and module identities were preserved.
    Resolution,

    /// The recognized transformation preserves evaluation effects.
    Effects,

    /// Formatter structural and byte-preservation validation completed.
    Preservation,
}

/// A guarded proposal with private, completed validation evidence.
#[derive(Clone, Debug)]
pub struct ValidatedEdit {
    pub(crate) proposal: EditProposal,
    pub(crate) inputs: Vec<InputVersion>,
    pub(crate) guarantees: Vec<EditGuarantee>,
    pub(crate) proposed: ProjectSnapshot,
    pub(crate) original: ProjectSnapshot,
}

impl ValidatedEdit {
    /// Returns the original-coordinate guarded replacements.
    #[must_use]
    pub fn proposal(&self) -> &EditProposal {
        &self.proposal
    }

    /// Returns every consulted original input required for application.
    #[must_use]
    pub fn inputs(&self) -> &[InputVersion] {
        &self.inputs
    }

    /// Returns the operation-specific guarantees actually established.
    #[must_use]
    pub fn guarantees(&self) -> &[EditGuarantee] {
        &self.guarantees
    }

    /// Returns the immutable validated source overlay.
    #[must_use]
    pub fn proposed(&self) -> &ProjectSnapshot {
        &self.proposed
    }

    /// Returns the complete original capture used for validation.
    #[must_use]
    pub fn original(&self) -> &ProjectSnapshot {
        &self.original
    }
}

/// The actual outcome of a requested source write.
#[derive(Clone, Debug, Serialize)]
pub enum WriteOutcome {
    /// No filesystem write was requested.
    NotRequested,

    /// The authorized source required no content change.
    Unchanged,

    /// A protected commit succeeded and advanced the source revision.
    Written {
        /// Original authorized source.
        source: SourceVersion,
        /// Committed source revision.
        revision: u64,
    },

    /// A write failed without claiming a committed revision.
    Failed {
        /// Original authorized source.
        source: SourceVersion,
        /// Concrete failure detail.
        message: String,
    },

    /// Required commit protection or another prerequisite was unavailable.
    Unapplied {
        /// Original authorized source.
        source: SourceVersion,
        /// Concrete missing guarantee or prerequisite.
        reason: String,
    },
}
