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

    struct Enumeration {
        name: String,
        items: Vec<String>,
    }

    struct Environment {
        definitions: Vec<Definition>,
        has_enumerations: bool,
        enumerations: Vec<Enumeration>,
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
    }

    /// A zero-based native byte position.
    #[derive(Copy, Clone, Debug, Eq, PartialEq)]
    pub struct Position {
        /// Zero-based line.
        line: u32,

        /// Zero-based byte column.
        column: u32,
    }

    /// A half-open native byte range.
    #[derive(Copy, Clone, Debug, Eq, PartialEq)]
    pub struct Range {
        /// Inclusive start.
        start: Position,

        /// Exclusive end.
        end: Position,
    }

    /// A source range retaining its physical and instantiated module identities.
    #[derive(Clone, Debug, Eq, PartialEq)]
    pub struct SourceLocation {
        /// Physical host source identifier.
        module: String,

        /// Native module identity, including instance context.
        identity: String,

        /// Native byte range.
        range: Range,
    }

    /// Owned semantic hover information.
    #[derive(Clone, Debug, Eq, PartialEq)]
    pub struct Hover {
        /// Hovered source range.
        location: SourceLocation,

        /// Identifier, when available.
        symbol: String,

        /// Inferred Luau type.
        type_name: String,

        /// Native documentation catalog symbol, empty when unavailable.
        documentation_symbol: String,

        /// Documentation text, empty when unavailable.
        documentation: String,
    }

    /// Native completion category.
    #[derive(Copy, Clone, Debug, Eq, PartialEq)]
    pub enum CompletionKind {
        /// A table or external property.
        Property,

        /// A value binding.
        Binding,

        /// A language keyword.
        Keyword,

        /// A string value.
        String,

        /// A type binding.
        Type,

        /// A module.
        Module,

        /// An inferred anonymous function.
        GeneratedFunction,

        /// An import path.
        RequirePath,

        /// A hot comment.
        HotComment,
    }

    /// An owned native completion candidate.
    #[derive(Clone, Debug, Eq, PartialEq)]
    pub struct Completion {
        /// Displayed candidate name.
        label: String,

        /// Candidate category.
        kind: CompletionKind,

        /// Whether the followed native type is a function or an overload intersection.
        is_function: bool,

        /// Inferred candidate type, empty when unavailable.
        type_name: String,

        /// Native documentation catalog symbol, empty when unavailable.
        documentation_symbol: String,

        /// Documentation text, empty when unavailable.
        documentation: String,

        /// Exact insertion text.
        insert_text: String,

        /// Whether insertion text uses snippet placeholders.
        snippet: bool,

        /// Source text replaced by insertion.
        replacement: SourceLocation,

        /// Whether the preceding dot must become a colon.
        replace_dot: bool,

        /// Dot range when replacement is required.
        operation: Range,

        /// Whether the candidate is deprecated.
        deprecated: bool,
    }

    /// An owned function parameter.
    #[derive(Clone, Debug, Eq, PartialEq)]
    pub struct Parameter {
        /// Argument name, empty when unavailable.
        name: String,

        /// Inferred argument type.
        type_name: String,

        /// Documentation text, empty when unavailable.
        documentation: String,

        /// Whether this parameter represents a variadic tail.
        variadic: bool,
    }

    /// An owned call signature.
    #[derive(Clone, Debug, Eq, PartialEq)]
    pub struct Signature {
        /// Native module identity of the call.
        identity: String,

        /// Displayable function signature.
        label: String,

        /// Native documentation catalog symbol, empty when unavailable.
        documentation_symbol: String,

        /// Documentation text, empty when unavailable.
        documentation: String,

        /// Function parameters, excluding implicit method self.
        parameters: Vec<Parameter>,

        /// Zero-based argument containing the cursor.
        active_parameter: u32,

        /// Whether native overload resolution selected this signature.
        selected: bool,
    }

    /// Semantic occurrence role.
    #[derive(Copy, Clone, Debug, Eq, PartialEq)]
    pub enum OccurrenceRole {
        /// Introduces the declaration.
        Declaration,

        /// Reads a value.
        Read,

        /// Writes a value.
        Write,

        /// Reads and writes a value.
        ReadWrite,

        /// References the type namespace.
        Type,
    }

    /// An occurrence linked to a stable source declaration.
    #[derive(Clone, Debug, Eq, PartialEq)]
    pub struct Occurrence {
        /// Occurrence source range.
        location: SourceLocation,

        /// Declaration identity and range.
        declaration: SourceLocation,

        /// Semantic access role.
        role: OccurrenceRole,

        /// Whether the declaration belongs to the type namespace.
        type_namespace: bool,
    }

    /// Inferred inlay hint category.
    #[derive(Copy, Clone, Debug, Eq, PartialEq)]
    pub enum HintKind {
        /// Inferred binding or parameter type.
        Type,

        /// Argument name.
        Parameter,

        /// Inferred function return type.
        Return,
    }

    /// An owned inferred inlay hint.
    #[derive(Clone, Debug, Eq, PartialEq)]
    pub struct Hint {
        /// Zero-width insertion location.
        location: SourceLocation,

        /// Displayable inferred information.
        label: String,

        /// Hint category.
        kind: HintKind,

        /// Whether the source already makes this information explicit.
        obvious: bool,
    }

    /// An authoritatively resolved native require argument.
    #[derive(Clone, Debug, Eq, PartialEq)]
    pub struct Import {
        /// Source range of the require argument.
        location: SourceLocation,

        /// Resolved physical source identifier.
        target: String,

        /// Resolved native module identity.
        identity: String,
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

        fn progress(self: &Context<'_>, module: &str, completed: bool) -> bool;

        fn advance(self: &Context<'_>, completed: usize, total: usize) -> bool;
    }

    unsafe extern "C++" {
        include!("native/frontend.hpp");
        include!("native/environment.hpp");
        include!("native/cancellation.hpp");
        include!("native/flags.hpp");
        type Frontend<'host>;

        fn create<'host>(
            host: &'host Context<'host>,
            retain_types: bool,
        ) -> Result<UniquePtr<Frontend<'host>>>;

        fn analyze(self: Pin<&mut Frontend<'_>>, module: &str) -> Result<Vec<Diagnostic>>;

        fn queue(self: Pin<&mut Frontend<'_>>, module: &str) -> Result<()>;

        fn checks(self: &Frontend<'_>) -> usize;

        fn selectors(settings: &[Flag], scoped: &[Flag], names: &[&str]) -> Result<Vec<bool>>;

        fn invalidate(self: Pin<&mut Frontend<'_>>, module: &str) -> Result<()>;

        fn refresh(self: Pin<&mut Frontend<'_>>) -> Result<()>;

        fn clear(self: Pin<&mut Frontend<'_>>) -> Result<()>;

        fn hover(
            self: Pin<&mut Frontend<'_>>,
            module: &str,
            line: u32,
            column: u32,
        ) -> Result<Vec<Hover>>;

        fn completion(
            self: Pin<&mut Frontend<'_>>,
            module: &str,
            line: u32,
            column: u32,
        ) -> Result<Vec<Completion>>;

        fn signatures(
            self: Pin<&mut Frontend<'_>>,
            module: &str,
            line: u32,
            column: u32,
        ) -> Result<Vec<Signature>>;

        fn definitions(
            self: Pin<&mut Frontend<'_>>,
            module: &str,
            line: u32,
            column: u32,
            types: bool,
        ) -> Result<Vec<SourceLocation>>;

        fn occurrences(self: Pin<&mut Frontend<'_>>, module: &str) -> Result<Vec<Occurrence>>;

        fn rename(
            self: Pin<&mut Frontend<'_>>,
            module: &str,
            line: u32,
            column: u32,
            name: &str,
            candidates: &[String],
        ) -> Result<Vec<Occurrence>>;

        fn hints(self: Pin<&mut Frontend<'_>>, module: &str) -> Result<Vec<Hint>>;
        fn imports(self: Pin<&mut Frontend<'_>>, module: &str) -> Result<Vec<Import>>;
        fn cancellation(self: &Frontend<'_>) -> Result<u64>;
        fn cancel(identifier: u64);
        fn reset(identifier: u64);
        fn release(identifier: u64);
    }
}

use std::collections::BTreeMap;

use crate::{
    configuration::{Configuration, Mode},
    environment::Environment,
    flags::Flag,
    host::Context,
    instances::Instance,
};

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
            has_enumerations: environment.enumerations.is_some(),
            enumerations: environment
                .enumerations
                .unwrap_or_default()
                .into_iter()
                .map(|(name, items)| bindings::Enumeration { name, items })
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
