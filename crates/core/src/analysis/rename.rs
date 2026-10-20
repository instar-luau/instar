use std::ops::Range;

use crate::{
    analysis::{Analysis, BindingKind, Edit, analyze},
    syntax::is_identifier,
};

/// A validated lexical rename target.
#[derive(Clone, Debug)]
pub struct RenameTarget {
    /// Selected identifier range.
    pub range: Range<usize>,

    /// Existing name for a rename prompt.
    pub name: String,
}

/// A rejected lexical rename.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum RenameError {
    /// The position does not identify a renameable local binding.
    #[error("No renameable lexical binding at this position.")]
    Target,

    /// The replacement is not a valid Luau identifier.
    #[error("The replacement is not a valid Luau identifier.")]
    Name,

    /// The document is incomplete and lexical safety cannot be established.
    #[error("Renaming requires a syntactically complete document.")]
    Incomplete,

    /// Renaming would conflict with or capture another binding.
    #[error("The replacement conflicts with or captures another binding.")]
    Conflict,
}

impl Analysis {
    /// Identifies an explicit local value or type name without selecting properties or globals.
    #[must_use]
    pub fn prepare_rename(&self, offset: usize) -> Option<RenameTarget> {
        if !self.is_complete || !self.source.is_char_boundary(offset) {
            return None;
        }

        self.bindings
            .iter()
            .filter(|binding| binding.can_rename)
            .find_map(|binding| {
                binding
                    .occurrences
                    .iter()
                    .find(|occurrence| occurrence.range.contains(&offset))
                    .map(|occurrence| RenameTarget {
                        range: occurrence.range.clone(),
                        name: binding.name.clone(),
                    })
            })
    }

    /// Renames a lexical binding only when all value and type resolutions remain unchanged.
    ///
    /// # Errors
    ///
    /// Rejects incomplete source, invalid names, nonlocal targets, and binding conflicts or captures.
    pub fn rename(&self, offset: usize, name: &str) -> Result<Vec<Edit>, RenameError> {
        if !self.is_complete {
            return Err(RenameError::Incomplete);
        }

        if !is_identifier(name) {
            return Err(RenameError::Name);
        }

        let target = self.prepare_rename(offset).ok_or(RenameError::Target)?;

        let index = self
            .bindings
            .iter()
            .position(|binding| {
                binding.can_rename
                    && binding
                        .occurrences
                        .iter()
                        .any(|occurrence| occurrence.range == target.range)
            })
            .ok_or(RenameError::Target)?;

        let binding = &self.bindings[index];

        if binding.name == name {
            return Ok(Vec::new());
        }

        if self.bindings.iter().enumerate().any(|(other, candidate)| {
            other != index
                && candidate.name == name
                && self.scopes[other] == self.scopes[index]
                && is_type_binding(candidate.kind) == is_type_binding(binding.kind)
        }) {
            return Err(RenameError::Conflict);
        }

        let edits: Vec<_> = binding
            .occurrences
            .iter()
            .map(|occurrence| Edit {
                range: occurrence.range.clone(),
                replacement: name.to_owned(),
            })
            .collect();

        let mut source = self.source.clone();

        for edit in edits.iter().rev() {
            source.replace_range(edit.range.clone(), &edit.replacement);
        }

        let renamed = analyze(&source);

        if !renamed.is_complete
            || renamed.identities != self.identities
            || renamed.bindings.len() != self.bindings.len()
            || renamed.scopes != self.scopes
        {
            return Err(RenameError::Conflict);
        }

        Ok(edits)
    }
}

fn is_type_binding(kind: BindingKind) -> bool {
    matches!(kind, BindingKind::TypeAlias | BindingKind::TypeParameter)
}
