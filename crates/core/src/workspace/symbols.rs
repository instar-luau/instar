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
    /// The declaration file that could not be parsed.
    pub path: PathBuf,

    /// Exact declaration bytes used during symbol collection.
    pub text: Arc<str>,

    /// The declaration syntax failure, relative to the retained text.
    pub error: lint::Error,
}

impl Workspace {
    /// Return declared value and type names for a module's lint environment.
    ///
    /// # Errors
    /// Returns invalid flags or inaccessible environment inputs.
    pub fn symbols(&self, module: &str) -> Result<SymbolOutcome, Error> {
        let scope = self.scope(Path::new(module))?;
        let name = environment::identity(self, &scope, Path::new(module))?;

        if let Some(symbols) = scope.symbols.borrow().get(&name) {
            return Ok(symbols.clone());
        }

        let builtins = Context::new(self)
            .builtins(host::flags(&scope.configuration))
            .map_err(Error::Analysis)?;

        let symbols = self.environments.borrow().symbols(
            name.as_deref(),
            &scope.configuration.environment.globals,
            &builtins,
        );

        scope.symbols.borrow_mut().insert(name, symbols.clone());

        Ok(symbols)
    }
}
