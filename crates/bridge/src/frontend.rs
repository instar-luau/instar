use cxx::UniquePtr;

use crate::{
    bridge::bindings::{
        self, Completion, Diagnostic, Hint, Hover, Import, Occurrence, Signature, SourceLocation,
    },
    cancellation::Cancellation,
    error::Error,
    host::Context,
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

    /// Queue a module and its dependencies for the next check.
    ///
    /// # Errors
    /// Returns an operational host error or native queueing failure.
    pub fn queue(&mut self, module: &str) -> Result<(), Error> {
        let result = self.native.pin_mut().queue(module);
        self.context.complete(result)?;

        Ok(())
    }

    /// Check a module and its dependencies, reusing unchanged native state.
    ///
    /// # Errors
    /// Returns an operational host error or native analysis failure.
    pub fn check(&mut self, module: &str) -> Result<Vec<Diagnostic>, Error> {
        let result = self.native.pin_mut().analyze(module);

        self.context.complete(result)
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

        self.context.complete(result)
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

        self.context.complete(result)
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

        self.context.complete(result)
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
        types: bool,
    ) -> Result<Vec<SourceLocation>, Error> {
        let result = self
            .native
            .pin_mut()
            .definitions(module, line, column, types);

        self.context.complete(result)
    }

    /// Return all declaration-linked occurrences in every instance of a source.
    ///
    /// # Errors
    /// Returns host, checking, or unavailable type-graph errors.
    pub fn occurrences(&mut self, module: &str) -> Result<Vec<Occurrence>, Error> {
        let result = self.native.pin_mut().occurrences(module);

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

    /// Return inferred binding, argument-name, and return-type inlay hints.
    ///
    /// # Errors
    /// Returns host, checking, or unavailable type-graph errors.
    pub fn hints(&mut self, module: &str) -> Result<Vec<Hint>, Error> {
        let result = self.native.pin_mut().hints(module);

        self.context.complete(result)
    }

    /// Parse the source and resolve require arguments for every native identity.
    ///
    /// # Errors
    /// Returns host, native parsing or resolution, or cancellation errors.
    pub fn imports(&mut self, module: &str) -> Result<Vec<Import>, Error> {
        let result = self.native.pin_mut().imports(module);

        self.context.complete(result)
    }

    /// Obtain a thread-safe cancellation handle without sharing the frontend.
    ///
    /// # Errors
    /// Returns native handle allocation failures.
    pub fn cancellation(&self) -> Result<Cancellation, Error> {
        let identifier = self.context.complete(self.native.cancellation())?;

        Ok(Cancellation::new(identifier))
    }
}
