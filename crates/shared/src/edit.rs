use std::{collections::BTreeMap, io};

use serde::{Deserialize, Serialize};

use crate::{Location, SourceSnapshot, SourceVersion};

/// An opaque edit proposal identity allocated by its producing operation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Deserialize, Serialize)]
pub struct EditIdentity(u64);

impl EditIdentity {
    /// Wraps an identity allocated uniquely by the proposal's owner.
    #[must_use]
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    /// Returns the producer's identity value.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

/// A UTF-8 text replacement at an exact immutable source revision.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct TextEdit {
    /// Exact original range to replace.
    pub location: Location,

    /// Replacement UTF-8 bytes.
    pub replacement: String,
}

/// The operation-specific validation policy for an edit proposal.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub enum EditIntent {
    /// Syntax-only layout transformation.
    Formatting,

    /// A diagnostic fix.
    Fix,

    /// A symbol rename.
    Rename,

    /// Import organization or insertion.
    Imports,

    /// Expression or function extraction.
    Extraction,

    /// Project-owned file creation, movement or deletion.
    FileOperations,
}

/// A guarded transformation; validation does not authorize filesystem writes.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct EditProposal {
    /// Stable identity reused by events and terminal reports.
    pub identity: EditIdentity,

    /// Original-coordinate replacements.
    pub edits: Vec<TextEdit>,

    /// Every source revision whose agreement is required for application.
    pub guards: Vec<SourceVersion>,

    /// Operation-specific transformation intent.
    pub intent: EditIntent,
}

impl EditProposal {
    /// Validates guards, exact edit revisions, UTF-8 ranges and overlapping replacements.
    ///
    /// # Errors
    /// Returns invalid input for absent, stale or conflicting sources and edits.
    pub fn validate(&self, sources: &[SourceSnapshot]) -> io::Result<()> {
        let mut snapshots = BTreeMap::new();

        for source in sources {
            source.validate()?;

            if snapshots.insert(source.version.identity, source).is_some() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "edit validation sources contain duplicate identities",
                ));
            }
        }

        let mut guards = BTreeMap::new();

        for guard in &self.guards {
            let source = snapshots
                .get(&guard.identity)
                .filter(|source| source.version == *guard)
                .ok_or_else(|| {
                    io::Error::new(io::ErrorKind::InvalidInput, "edit guard is absent or stale")
                })?;

            if guards.insert(guard.identity, source.version).is_some() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "edit proposal contains duplicate source guards",
                ));
            }
        }

        let mut spans = BTreeMap::new();

        for edit in &self.edits {
            let source = snapshots
                .get(&edit.location.source.identity)
                .ok_or_else(|| {
                    io::Error::new(io::ErrorKind::InvalidInput, "edit source is absent")
                })?;

            edit.location.validate(source)?;

            if guards.get(&source.version.identity) != Some(&source.version) {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "edit source is not guarded",
                ));
            }

            spans
                .entry(source.version)
                .or_insert_with(Vec::new)
                .push(edit.location.span);
        }

        for ranges in spans.values_mut() {
            ranges.sort_unstable();

            for pair in ranges.windows(2) {
                if pair[0].end > pair[1].start || pair[0].start == pair[1].start {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "edit replacements overlap",
                    ));
                }
            }
        }

        Ok(())
    }
}
