use std::ops::Range;

use vermis::tree::{NodeKind, Tree};

use crate::{
    format::{
        Edit, Error, Prepared,
        directives::{self, Kind},
        engine,
        imports::Imports,
        statements,
    },
    settings::format::LineEnding,
    syntax::bindings::Bindings,
};

/// An import block that can be extended without changing the rest of a document.
pub struct Insertion<'source> {
    /// The original byte range occupied by the import block.
    pub range: Range<usize>,

    source: &'source str,
    tree: Tree<'source>,
    bindings: Bindings,
}

impl Insertion<'_> {
    /// Find an unchanged module binding in this block by its require argument.
    #[must_use]
    pub fn binding(&self, argument: &Range<usize>) -> Option<String> {
        let prefix = self.source.len() - self.tree.source.len();

        self.bindings.locals.iter().find_map(|binding| {
            let statement = self.tree.node(binding.statement).span;

            if binding.is_assigned
                || statement.start + prefix < self.range.start
                || statement.end + prefix > self.range.end
            {
                return None;
            }

            let NodeKind::Call { callee, arguments } = self.tree.node(binding.value?).kind else {
                return None;
            };

            if !self.bindings.is_global(&self.tree, callee, "require") {
                return None;
            }

            let NodeKind::Arguments { values, .. } = &self.tree.node(arguments).kind else {
                return None;
            };

            let [value] = self.tree.list(values) else {
                return None;
            };

            let span = self.tree.node(value.node).span;

            ((span.start + prefix..span.end + prefix) == *argument).then(|| binding.name.clone())
        })
    }

    /// Whether a value name is declared locally or assigned globally.
    #[must_use]
    pub fn is_bound(&self, name: &str) -> bool {
        self.bindings
            .locals
            .iter()
            .any(|binding| binding.name == name)
            || self
                .bindings
                .globals
                .get(name)
                .is_some_and(|global| !global.writes.is_empty())
    }

    /// Whether a type name is already declared in the document.
    #[must_use]
    pub fn is_type_bound(&self, name: &str) -> bool {
        self.bindings
            .types
            .iter()
            .any(|binding| binding.name == name)
    }

    /// Whether a generated binding name is already present in the document.
    #[must_use]
    pub fn is_used(&self, name: &str) -> bool {
        self.is_bound(name)
            || self
                .tree
                .tokens
                .iter()
                .any(|token| token.bytes(self.tree.source) == name.as_bytes())
    }

    /// Add a declaration and format the resulting import block.
    ///
    /// # Errors
    /// Returns invalid syntax, directives, or failed formatting preservation checks.
    pub fn format(&self, declaration: &str, configuration: &Prepared) -> Result<Edit, Error> {
        let expected = &self.source[self.range.clone()];
        let mut proposed = expected.to_owned();

        if !proposed.is_empty() && !proposed.ends_with('\n') {
            proposed.push('\n');
        }

        proposed.push_str(declaration);
        let mut replacement = engine::format(&proposed, configuration)?;

        let ending = match configuration.settings().line_ending {
            LineEnding::LineFeed => "\n",
            LineEnding::CarriageReturnLineFeed => "\r\n",
        };

        if !replacement.ends_with('\n') {
            replacement.push_str(ending);
        }

        if self.range.end < self.source.len()
            && !self.source[self.range.end..].starts_with(['\r', '\n'])
        {
            replacement.push_str(ending);
        }

        Ok(Edit {
            range: self.range.clone(),
            expected: expected.to_owned(),
            replacement,
        })
    }
}

/// Plan an import insertion before a completion without requiring a complete document.
///
/// # Errors
/// Returns an invalid completion offset.
pub fn insertion(source: &str, before: usize) -> Result<Option<Insertion<'_>>, Error> {
    if !source.is_char_boundary(before) {
        return Err(Error::input(before..before, "Invalid completion offset."));
    }

    let prefix = usize::from(source.starts_with('\u{feff}')) * 3;
    let tree = vermis::parse(&source.as_bytes()[prefix..]);

    let Ok(directives) = directives::collect(&tree) else {
        return Ok(None);
    };

    let NodeKind::Root { block, .. } = tree.node(tree.root).kind else {
        return Ok(None);
    };

    let NodeKind::Block {
        statements: entries,
    } = &tree.node(block).kind
    else {
        return Ok(None);
    };

    let entries = tree.list(entries);
    let imports = Imports::new(&tree);
    let ranges = statements::ranges(&tree, entries);

    let start = prefix
        + ranges
            .first()
            .map_or(tree.source.len(), |range| range.start);

    if start > before {
        return Ok(None);
    }

    let mut end = start;

    for (entry, range) in entries.iter().zip(&ranges) {
        if prefix + range.end > before
            || imports
                .declaration(&tree, entry.node, &directives)
                .is_none()
            || directives.boundary(&tree, entry.node).is_some()
        {
            break;
        }

        end = prefix + range.end;
    }

    if directives.policies.iter().any(|directive| {
        directive.kind != Kind::Global
            && directive.scope.start + prefix <= start
            && start < directive.scope.end + prefix
    }) {
        return Ok(None);
    }

    if engine::parse(&source.as_bytes()[start..end]).is_err() {
        return Ok(None);
    }

    Ok(Some(Insertion {
        range: start..end,
        source,
        tree,
        bindings: imports.bindings,
    }))
}
