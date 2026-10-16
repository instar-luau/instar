#[cxx::bridge(namespace = "instar::luau")]
pub(crate) mod bindings {
    enum Mode {
        NoCheck,
        NonStrict,
        Strict,
    }

    struct Configuration {
        mode: Mode,
        globals: Vec<String>,
        environment: Text,
        flags: Vec<Flag>,
        locations: Vec<Location>,
    }

    enum FlagKind {
        Boolean,
        Integer,
    }

    struct Flag {
        name: String,
        kind: FlagKind,
        boolean: bool,
        integer: i32,
    }

    struct Definition {
        name: String,

        #[cxx_name = "namespace_name"]
        namespace: String,

        source: String,
    }

    struct Property {
        name: String,
        read: bool,
        write: bool,
    }

    struct Class {
        name: String,
        service: bool,
        creatable: bool,
        properties: Vec<Property>,
    }

    struct Environment {
        definitions: Vec<Definition>,
        has_capabilities: bool,
        classes: Vec<Class>,
    }

    struct Instance {
        name: String,
        class_name: String,
        has_parent: bool,
        parent: usize,
        module: Text,
    }

    struct Location {
        hierarchy: String,
        has_script: bool,
        script: usize,
    }

    /// A native parse or type-checking diagnostic.
    #[derive(Clone, Debug, Eq, PartialEq)]
    pub struct Diagnostic {
        /// Host module identifier.
        module: String,

        /// Upstream Luau diagnostic code.
        code: i32,

        /// Zero-based start line.
        start_line: u32,

        /// Zero-based start byte column.
        start_column: u32,

        /// Zero-based exclusive end line.
        end_line: u32,

        /// Zero-based exclusive end byte column.
        end_column: u32,

        /// Upstream diagnostic message.
        message: String,

        /// Whether this error prevents linting or validating source edits.
        blocking: bool,
    }

    /// A native lint finding with its rule-specific subject.
    #[derive(Clone, Debug, Eq, PartialEq)]
    pub struct Warning {
        /// Finding location and message.
        diagnostic: Diagnostic,

        /// Canonical lint rule name.
        rule: String,

        /// Binding name or symbol path used by rule settings.
        subject: String,

        /// Binding or symbol category used by rule settings.
        category: String,

        /// Whether removing this local self-assignment is safe.
        fixable: bool,
    }

    /// Cached analysis diagnostics and lint findings for a selected module.
    #[derive(Clone, Debug)]
    pub struct Analysis {
        /// Type, syntax, and declaration diagnostics.
        diagnostics: Vec<Diagnostic>,

        /// Findings belonging to the selected source.
        warnings: Vec<Warning>,
    }

    struct Text {
        present: bool,
        value: String,
    }

    enum Status {
        Found,
        Missing,
        Ambiguous,
    }

    extern "Rust" {
        type Context<'host>;

        fn source(self: &Context<'_>, module: &str) -> Result<Text>;

        fn parent(self: &Context<'_>, module: &str, destination: &mut String) -> Result<Status>;

        fn child(
            self: &Context<'_>,
            module: &str,
            name: &str,
            destination: &mut String,
        ) -> Result<Status>;

        fn alias(
            self: &Context<'_>,
            module: &str,
            name: &str,
            destination: &mut String,
        ) -> Result<Status>;

        fn configuration(self: &Context<'_>, module: &str) -> Result<Configuration>;

        fn environment(self: &Context<'_>, name: &str) -> Result<Environment>;

        fn flags(self: &Context<'_>) -> Result<Vec<Flag>>;

        fn hierarchy(self: &Context<'_>, name: &str) -> Result<Vec<Instance>>;
    }

    unsafe extern "C++" {
        include!("native/frontend.hpp");

        type Frontend<'host>;

        fn create<'host>(
            host: &'host Context<'host>,
            retain_types: bool,
        ) -> Result<UniquePtr<Frontend<'host>>>;

        fn analyze(self: Pin<&mut Frontend<'_>>, module: &str, lint: bool) -> Result<Analysis>;

        fn warnings() -> Vec<String>;

        fn invalidate(self: Pin<&mut Frontend<'_>>, module: &str) -> Result<()>;

        fn clear(self: Pin<&mut Frontend<'_>>) -> Result<()>;
    }
}

use std::collections::BTreeMap;

use crate::configuration::{Configuration, Mode};
use crate::environment::Environment;
use crate::flags::Flag;
use crate::host::Context;
use crate::instances::Instance;

impl From<Configuration> for bindings::Configuration {
    fn from(configuration: Configuration) -> Self {
        let Configuration {
            mode,
            globals,
            environment,
            flags: overrides,
            locations,
        } = configuration;

        let mode = match mode {
            Mode::NoCheck => bindings::Mode::NoCheck,
            Mode::Nonstrict => bindings::Mode::NonStrict,
            Mode::Strict => bindings::Mode::Strict,
        };

        Self {
            mode,
            globals,
            environment: environment.into(),
            flags: flags(overrides),
            locations: locations
                .into_iter()
                .map(|location| bindings::Location {
                    hierarchy: location.hierarchy,
                    has_script: location.script.is_some(),
                    script: location.script.unwrap_or_default(),
                })
                .collect(),
        }
    }
}

pub(crate) fn flags(values: BTreeMap<String, Flag>) -> Vec<bindings::Flag> {
    values
        .into_iter()
        .map(|(name, value)| match value {
            Flag::Boolean(boolean) => bindings::Flag {
                name,
                kind: bindings::FlagKind::Boolean,
                boolean,
                integer: 0,
            },

            Flag::Integer(integer) => bindings::Flag {
                name,
                kind: bindings::FlagKind::Integer,
                boolean: false,
                integer,
            },
        })
        .collect()
}

impl From<Environment> for bindings::Environment {
    fn from(environment: Environment) -> Self {
        Self {
            definitions: environment
                .definitions
                .into_iter()
                .map(|definition| bindings::Definition {
                    name: definition.name,
                    namespace: definition.namespace,
                    source: definition.source,
                })
                .collect(),
            has_capabilities: environment.classes.is_some(),
            classes: environment
                .classes
                .unwrap_or_default()
                .into_iter()
                .map(|class| bindings::Class {
                    name: class.name,
                    service: class.service,
                    creatable: class.creatable,
                    properties: class
                        .properties
                        .into_iter()
                        .map(|property| bindings::Property {
                            name: property.name,
                            read: property.read,
                            write: property.write,
                        })
                        .collect(),
                })
                .collect(),
        }
    }
}

impl From<Instance> for bindings::Instance {
    fn from(instance: Instance) -> Self {
        Self {
            name: instance.name,
            class_name: instance.class,
            has_parent: instance.parent.is_some(),
            parent: instance.parent.unwrap_or_default(),
            module: instance.module.into(),
        }
    }
}

impl From<Option<String>> for bindings::Text {
    fn from(value: Option<String>) -> Self {
        Self {
            present: value.is_some(),
            value: value.unwrap_or_default(),
        }
    }
}
