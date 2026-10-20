use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use instar_bridge::Context;

use super::{Error, Workspace, environment, host};
use crate::lint::{self, Symbols};

/// Declaration names or the exact input that prevented their construction.
#[derive(Clone, Debug)]
pub enum SymbolOutcome {
    /// The selected declaration environment is valid.
    Ready(Arc<Symbols>),

    /// A declaration source contains invalid syntax.
    Invalid(InvalidSource),
}

/// A declaration failure with its original source snapshot.
#[derive(Clone, Debug)]
pub struct InvalidSource {
    /// The source that could not be parsed.
    pub origin: Origin,

    /// Exact declaration bytes used during symbol collection.
    pub text: Arc<str>,

    /// The declaration syntax failure, relative to the retained text.
    pub error: lint::Error,
}

/// The origin of a declaration source.
#[derive(Clone, Debug)]
pub enum Origin {
    /// A declaration file.
    File(PathBuf),

    /// Native in-memory builtin declarations.
    Builtins,
}

impl Workspace {
    /// Return declared value and type names for a module's lint environment.
    ///
    /// # Errors
    /// Returns invalid flags or inaccessible environment inputs.
    pub fn symbols(&self, module: &str) -> Result<SymbolOutcome, Error> {
        let scope = self.scope(Path::new(module))?;
        let cached = scope.builtins.borrow().as_ref().map(Arc::clone);

        let builtins = if let Some(builtins) = cached {
            builtins
        } else {
            let builtins: Arc<str> = Context::new(self)
                .builtins(host::flags(&scope.configuration))
                .map_err(Error::Analysis)?
                .into();

            *scope.builtins.borrow_mut() = Some(Arc::clone(&builtins));

            builtins
        };

        let name = environment::identity(self, &scope, Path::new(module))?;

        Ok(self.environments.borrow_mut().symbols(
            name.as_deref(),
            &scope.configuration.environment.globals,
            &builtins,
        ))
    }
}
