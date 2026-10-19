use vermis::token::TokenKind;

use crate::analysis::{Analysis, BindingKind, Edit, RenameError, RenameTarget, analyze};

impl Analysis {
    /// Identifies an explicit local value or type name without selecting properties or globals.
    #[must_use]
    pub fn prepare_rename(&self, offset: usize) -> Option<RenameTarget> {
        if !self.complete || !self.source.is_char_boundary(offset) {
            return None;
        }

        self.bindings
            .iter()
            .filter(|binding| binding.renameable)
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
        if !self.complete {
            return Err(RenameError::Incomplete);
        }

        if !identifier(name) {
            return Err(RenameError::Name);
        }

        let target = self.prepare_rename(offset).ok_or(RenameError::Target)?;

        let index = self
            .bindings
            .iter()
            .position(|binding| {
                binding.renameable
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
                && type_binding(candidate.kind) == type_binding(binding.kind)
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

        if !renamed.complete
            || renamed.identities != self.identities
            || renamed.bindings.len() != self.bindings.len()
            || renamed.scopes != self.scopes
        {
            return Err(RenameError::Conflict);
        }

        Ok(edits)
    }
}

fn identifier(name: &str) -> bool {
    let mut bytes = name.bytes();

    let Some(first) = bytes.next() else {
        return false;
    };

    if !(first.is_ascii_alphabetic() || first == b'_')
        || !bytes.all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
    {
        return false;
    }

    let tree = vermis::parse(name.as_bytes());

    matches!(tree.tokens.first(), Some(token) if token.kind == TokenKind::Name && token.span.end == name.len())
}

fn type_binding(kind: BindingKind) -> bool {
    matches!(kind, BindingKind::TypeAlias | BindingKind::TypeParameter)
}
