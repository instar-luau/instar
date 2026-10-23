use cxx::UniquePtr;

use crate::{
    bridge::bindings::{
        self, Diagnostic, HintSelector, Import, Occurrence, Position, SourceLocation,
    },
    cancellation::Cancellation,
    error::Error,
    host::Context,
    semantic::{Completion, Hint, Hover, Signature},
};

/// Owns Luau's persistent analysis state while borrowing its host context.
pub struct Frontend<'host> {
    native: UniquePtr<bindings::Frontend<'host>>,
    context: &'host Context<'host>,
}

impl<'host> Frontend<'host> {
    /// Create a frontend, optionally retaining full type graphs for semantic queries.
    ///
    /// # Errors
    /// Returns host/native initialization errors or conflicting live frontend flag settings.
    pub fn new(context: &'host Context<'host>, retain_types: bool) -> Result<Self, Error> {
        let native = context.complete(bindings::create(context, retain_types))?;

        Ok(Self { native, context })
    }

    /// Check a module and its dependencies, reusing unchanged native state.
    ///
    /// # Errors
    /// Returns an operational host error or native analysis failure.
    pub fn check(&mut self, module: &str) -> Result<Vec<Diagnostic>, Error> {
        let result = self.native.pin_mut().check(module);

        self.context.complete(result)
    }

    /// Queue a module and its dependencies for the next check.
    ///
    /// # Errors
    /// Returns an operational host error or native queueing failure.
    pub fn queue(&mut self, module: &str) -> Result<(), Error> {
        let result = self.native.pin_mut().queue(module);
        self.context.complete(result)?;

        Ok(())
    }

    /// Return the number of native type checks since construction or clearing.
    #[must_use]
    pub fn checks(&self) -> usize {
        self.native.checks()
    }

    /// Invalidate a changed module and its transitive dependents.
    ///
    /// # Errors
    /// Returns a native invalidation failure.
    pub fn invalidate(&mut self, module: &str) -> Result<(), Error> {
        let result = self.native.pin_mut().invalidate(module);
        self.context.complete(result)?;

        Ok(())
    }

    /// Re-resolve cached imports after source additions, removals, or renames.
    ///
    /// # Errors
    /// Returns a host navigation error or native invalidation failure.
    pub fn refresh(&mut self) -> Result<(), Error> {
        let result = self.native.pin_mut().refresh();
        self.context.complete(result)?;

        Ok(())
    }

    /// Rebuild builtin state and discard module, configuration, and declaration caches.
    ///
    /// # Errors
    /// Returns a native cache-clearing failure.
    pub fn clear(&mut self) -> Result<(), Error> {
        let result = self.native.pin_mut().clear();
        self.context.complete(result)?;

        Ok(())
    }

    /// Return owned hover data for every instance identity of a physical source.
    ///
    /// # Errors
    /// Returns host, checking, or unavailable type-graph errors.
    pub fn hover(&mut self, module: &str, line: u32, column: u32) -> Result<Vec<Hover>, Error> {
        let result = self.native.pin_mut().hover(module, line, column);

        self.context
            .complete(result)?
            .into_iter()
            .map(Hover::try_from)
            .collect()
    }

    /// Complete incomplete editor input using Luau's native autocomplete engine.
    ///
    /// # Errors
    /// Returns host, checking, or unavailable type-graph errors.
    pub fn completion(
        &mut self,
        module: &str,
        line: u32,
        column: u32,
    ) -> Result<Vec<Completion>, Error> {
        let result = self.native.pin_mut().completion(module, line, column);

        self.context
            .complete(result)?
            .into_iter()
            .map(Completion::try_from)
            .collect()
    }

    /// Expand one completion by its stable context-and-label enumeration index.
    ///
    /// # Errors
    /// Returns native checking, extraction, cancellation, or validation errors.
    pub fn completion_detail(
        &mut self,
        module: &str,
        line: u32,
        column: u32,
        index: usize,
    ) -> Result<Option<Completion>, Error> {
        let result = self
            .native
            .pin_mut()
            .completion_detail(module, line, column, index);

        let mut values = self.context.complete(result)?;

        if values.len() > 1 {
            return Err(Error::Native {
                reason: "Invalid completion selection cardinality.".into(),
            });
        }

        values.pop().map(Completion::try_from).transpose()
    }

    /// Return native overloads and argument metadata at a call position.
    ///
    /// # Errors
    /// Returns host, checking, or unavailable type-graph errors.
    pub fn signatures(
        &mut self,
        module: &str,
        line: u32,
        column: u32,
    ) -> Result<Vec<Signature>, Error> {
        let result = self.native.pin_mut().signatures(module, line, column);

        self.context
            .complete(result)?
            .into_iter()
            .map(Signature::try_from)
            .collect()
    }

    /// Resolve value declarations, or inferred type declarations when `types` is true.
    ///
    /// # Errors
    /// Returns host, checking, or unavailable type-graph errors.
    pub fn definitions(
        &mut self,
        module: &str,
        line: u32,
        column: u32,
        resolve_types: bool,
    ) -> Result<Vec<SourceLocation>, Error> {
        let result = self
            .native
            .pin_mut()
            .definitions(module, line, column, resolve_types);

        self.context.complete(result)
    }

    /// Return declaration-linked occurrences, selecting all when declarations are empty.
    ///
    /// # Errors
    /// Returns host, checking, or unavailable type-graph errors.
    pub fn occurrences(
        &mut self,
        module: &str,
        declarations: &[Occurrence],
    ) -> Result<Vec<Occurrence>, Error> {
        let result = self.native.pin_mut().occurrences(module, declarations);

        self.context.complete(result)
    }

    /// Validate a rename and return affected occurrences across candidate sources.
    ///
    /// # Errors
    /// Returns invalid names, binding conflicts, unsupported targets, or analysis failures.
    pub fn rename(
        &mut self,
        module: &str,
        line: u32,
        column: u32,
        name: &str,
        candidates: &[String],
    ) -> Result<Vec<Occurrence>, Error> {
        let result = self
            .native
            .pin_mut()
            .rename(module, line, column, name, candidates);

        self.context.complete(result)
    }

    /// Return hints in a native byte range, expanding only selected subjects.
    ///
    /// # Errors
    /// Returns native checking, extraction, cancellation, or validation errors.
    pub fn hints_range(
        &mut self,
        module: &str,
        start: Position,
        end: Position,
        is_expanded: bool,
    ) -> Result<Vec<Hint>, Error> {
        let result = self
            .native
            .pin_mut()
            .hints_range(module, start, end, is_expanded);

        self.context
            .complete(result)?
            .into_iter()
            .map(Hint::try_from)
            .collect()
    }

    /// Resolve selected inlay subjects with a shared whole-request extraction budget.
    ///
    /// # Errors
    /// Returns native checking, extraction, cancellation, or validation errors.
    pub fn hint_subjects(&mut self, selectors: &[HintSelector]) -> Result<Vec<Hint>, Error> {
        let result = self.native.pin_mut().hint_subjects(selectors);

        self.context
            .complete(result)?
            .into_iter()
            .map(Hint::try_from)
            .collect()
    }

    /// Resolve require arguments for every native identity.
    ///
    /// # Errors
    /// Returns host, native parsing or resolution, or cancellation errors.
    pub fn imports(&mut self, module: &str) -> Result<Vec<Import>, Error> {
        let result = self.native.pin_mut().imports(module);

        self.context.complete(result)
    }

    /// Obtain a thread-safe cancellation handle without sharing the frontend.
    #[must_use]
    pub fn cancellation(&self) -> Cancellation {
        Cancellation::new(self.native.cancellation())
    }
}
