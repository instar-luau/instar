use cxx::UniquePtr;

use crate::bridge::bindings::{self, Analysis, Diagnostic};
use crate::error::Error;
use crate::host::Context;

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
        let result = self.native.pin_mut().analyze(module, false);

        self.context
            .complete(result)
            .map(|analysis| analysis.diagnostics)
    }

    /// Lint a module using the same persistent dependency and declaration state.
    ///
    /// # Errors
    /// Returns an operational host error or native analysis failure.
    pub fn lint(&mut self, module: &str) -> Result<Analysis, Error> {
        let result = self.native.pin_mut().analyze(module, true);

        self.context.complete(result)
    }

    /// Invalidate a changed module and its transitive dependents.
    ///
    /// Use [`Self::clear`] when navigation, aliases, shared configuration,
    /// or the module inventory changes.
    ///
    /// # Errors
    /// Returns a native invalidation failure.
    pub fn invalidate(&mut self, module: &str) -> Result<(), Error> {
        let result = self.native.pin_mut().invalidate(module);
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
}

/// Return the canonical names of the bundled Luau warnings.
#[must_use]
pub fn warnings() -> Vec<String> {
    bindings::warnings()
}
