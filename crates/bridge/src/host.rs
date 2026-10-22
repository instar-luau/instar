use std::{
    cell::{Cell, RefCell},
    collections::{BTreeMap, HashMap},
};

use cxx::Exception;

use crate::{
    bridge::{self, bindings},
    configuration::Configuration,
    environment::Environment,
    error::{Error, HostError, Operation},
    flags::Flag,
    instances::Instance,
};

/// Supplies module identity, source snapshots, navigation, and resolved configuration.
///
/// Module identifiers are opaque to Luau. The host supplies filesystem navigation,
/// scoped aliases, and instance hierarchies.
pub trait Host {
    /// Resolve an unprefixed, lowercase alias in the requiring module's scope.
    ///
    /// # Errors
    /// Returns an operational alias-resolution failure.
    fn alias(&self, module: &str, name: &str) -> Result<Navigation, HostError>;

    /// List aliases available in the requiring module's scope.
    ///
    /// # Errors
    /// Returns the host's configuration error.
    fn aliases(&self, module: &str) -> Result<Vec<String>, HostError>;

    /// Find a named child in the given navigation context.
    ///
    /// # Errors
    /// Returns an operational child-lookup failure.
    fn child(&self, module: &str, name: &str) -> Result<Navigation, HostError>;

    /// List child names in the given navigation context.
    ///
    /// # Errors
    /// Returns an operational directory-listing failure.
    fn children(&self, module: &str) -> Result<Vec<String>, HostError>;

    /// Return the module's already-resolved type-checking configuration.
    ///
    /// # Errors
    /// Returns the host's configuration error.
    fn configuration(&self, module: &str) -> Result<Configuration, HostError>;

    /// Load a named declaration environment requested by resolved configuration.
    ///
    /// # Errors
    /// Returns an environment input or loading failure.
    fn environment(&self, name: &str) -> Result<Environment, HostError>;

    /// Return the native flags required by this frontend.
    ///
    /// # Errors
    /// Returns the host's flag configuration failure.
    fn flags(&self) -> Result<BTreeMap<String, Flag>, HostError>;

    /// Load the instances of a named hierarchy, with its root first.
    ///
    /// # Errors
    /// Returns a hierarchy input or loading failure.
    fn hierarchy(&self, name: &str) -> Result<Vec<Instance>, HostError>;

    /// Find the parent navigation context, returning `Missing` at the root.
    ///
    /// # Errors
    /// Returns an operational parent-lookup failure.
    fn parent(&self, module: &str) -> Result<Navigation, HostError>;

    /// Read a module, returning `None` when it does not exist.
    ///
    /// # Errors
    /// Returns the host's source-loading error.
    fn source(&self, module: &str) -> Result<Option<String>, HostError>;
}

/// The outcome of a module-navigation lookup.
#[derive(Clone, Debug)]
pub enum Navigation {
    /// A unique host module identifier was found.
    Found(String),

    /// No matching navigation context exists.
    Missing,

    /// More than one navigation context matches.
    Ambiguous,
}

/// Native analysis progress.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Progress<'module> {
    /// Analysis is beginning for this module.
    Started(&'module str),

    /// Analysis has completed for this module.
    Finished(&'module str),

    /// A batch of dirty native work has completed.
    Advanced {
        /// Completed native work units.
        completed: usize,
        /// Dirty native work units in this batch.
        total: usize,
    },
}

/// Borrows the host callbacks used by native frontends.
pub struct Context<'host> {
    host: &'host dyn Host,
    error: Cell<Option<HostError>>,
    navigation: RefCell<HashMap<(Operation, String, String), Navigation>>,
    progress: Option<&'host dyn Fn(Progress<'_>) -> bool>,
}

impl<'host> Context<'host> {
    /// Borrow a host that supplies current source snapshots.
    pub fn new(host: &'host dyn Host) -> Self {
        let error = Cell::new(None);

        Self {
            host,
            error,
            navigation: RefCell::new(HashMap::new()),
            progress: None,
        }
    }

    /// Report native analysis progress, returning false to interrupt analysis.
    #[must_use]
    pub fn with_progress(mut self, progress: &'host dyn Fn(Progress<'_>) -> bool) -> Self {
        self.progress = Some(progress);

        self
    }

    /// Return built-in declaration syntax using validated host and scope flags.
    ///
    /// # Errors
    /// Returns host configuration failures or invalid native flag settings.
    pub fn builtins(&self, flags: BTreeMap<String, Flag>) -> Result<String, Error> {
        let settings = bridge::flags(self.host.flags().map_err(Error::Host)?);
        let scoped = bridge::flags(flags);

        let mut source = String::new();

        let available = self.complete(bindings::declarations(
            &bindings::DeclarationFlags {
                settings: &settings,
                scoped: &scoped,
            },
            &mut source,
        ))?;

        if available {
            Ok(source)
        } else {
            Err(Error::FlagsBusy)
        }
    }

    pub(crate) fn alias(
        &self,
        module: &str,
        name: &str,
        destination: &mut String,
    ) -> Result<bindings::Status, &'static str> {
        self.navigate(
            (Operation::Alias, module, name),
            || self.host.alias(module, name),
            destination,
        )
    }

    pub(crate) fn aliases(&self, module: &str) -> Result<Vec<String>, &'static str> {
        self.capture(self.host.aliases(module))
    }

    pub(crate) fn child(
        &self,
        module: &str,
        name: &str,
        destination: &mut String,
    ) -> Result<bindings::Status, &'static str> {
        self.navigate(
            (Operation::Child, module, name),
            || self.host.child(module, name),
            destination,
        )
    }

    pub(crate) fn children(&self, module: &str) -> Result<Vec<String>, &'static str> {
        self.capture(self.host.children(module))
    }

    pub(crate) fn configuration(
        &self,
        module: &str,
    ) -> Result<bindings::Configuration, &'static str> {
        self.capture(self.host.configuration(module))
            .map(Into::into)
    }

    pub(crate) fn environment(&self, name: &str) -> Result<bindings::Environment, &'static str> {
        self.capture(self.host.environment(name)).map(Into::into)
    }

    pub(crate) fn flags(&self) -> Result<Vec<bindings::Flag>, &'static str> {
        self.capture(self.host.flags()).map(bridge::flags)
    }

    pub(crate) fn hierarchy(&self, name: &str) -> Result<Vec<bindings::Instance>, &'static str> {
        self.capture(self.host.hierarchy(name))
            .map(|instances| instances.into_iter().map(Into::into).collect())
    }

    pub(crate) fn parent(
        &self,
        module: &str,
        destination: &mut String,
    ) -> Result<bindings::Status, &'static str> {
        self.navigate(
            (Operation::Parent, module, ""),
            || self.host.parent(module),
            destination,
        )
    }

    pub(crate) fn source(&self, module: &str) -> Result<bindings::Text, &'static str> {
        self.capture(self.host.source(module)).map(Into::into)
    }

    pub(crate) fn progress(&self, module: &str, is_completed: bool) -> bool {
        self.progress.is_none_or(|progress| {
            progress(if is_completed {
                Progress::Finished(module)
            } else {
                Progress::Started(module)
            })
        })
    }

    pub(crate) fn advance(&self, completed: usize, total: usize) -> bool {
        self.progress
            .is_none_or(|progress| progress(Progress::Advanced { completed, total }))
    }

    pub(crate) fn complete<Value>(&self, result: Result<Value, Exception>) -> Result<Value, Error> {
        self.navigation.borrow_mut().clear();

        match self.error.take() {
            Some(error) => Err(Error::Host(error)),

            None => result.map_err(|exception| Error::Native {
                reason: exception.what().to_owned(),
            }),
        }
    }

    fn capture<Value>(&self, result: Result<Value, HostError>) -> Result<Value, &'static str> {
        result.map_err(|error| {
            self.error.set(Some(error));

            "Host callback failed."
        })
    }

    fn navigate(
        &self,
        identity: (Operation, &str, &str),
        lookup: impl FnOnce() -> Result<Navigation, HostError>,
        destination: &mut String,
    ) -> Result<bindings::Status, &'static str> {
        let key = (identity.0, identity.1.to_owned(), identity.2.to_owned());
        let cached = self.navigation.borrow().get(&key).cloned();

        let navigation = if let Some(navigation) = cached {
            navigation
        } else {
            let navigation = self.capture(lookup())?;
            self.navigation.borrow_mut().insert(key, navigation.clone());

            navigation
        };

        let status = match navigation {
            Navigation::Found(module) => {
                *destination = module;

                bindings::Status::Found
            }

            Navigation::Missing => bindings::Status::Missing,
            Navigation::Ambiguous => bindings::Status::Ambiguous,
        };

        Ok(status)
    }
}
