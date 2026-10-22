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

    struct DeclarationFlags<'flags> {
        settings: &'flags [Flag],
        scoped: &'flags [Flag],
    }

    struct Definition {
        name: String,

        #[cxx_name = "namespace_name"]
        namespace: String,

        source: String,
    }

    struct Property {
        name: String,
        can_read: bool,
        can_write: bool,
    }

    struct Class {
        name: String,
        is_service: bool,
        is_creatable: bool,
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

    /// The selected hover subject's declaration category.
    #[derive(Copy, Clone, Debug, Eq, PartialEq)]
    pub enum HoverKind {
        /// A local binding or reference.
        Local,

        /// A global binding reference.
        Global,

        /// A type alias declaration.
        Alias,

        /// A property or named annotation parameter.
        Field,

        /// An individual type component or unnamed expression.
        Type,

        /// A locally declared function or reference to it.
        LocalFunction,

        /// A global function binding.
        Function,

        /// A function parameter or reference to it.
        Parameter,

        /// A loop variable or reference to it.
        Loop,

        /// A constant binding or reference to it.
        Constant,

        /// A constant function declaration or reference to it.
        ConstantFunction,
    }

    struct NativeHover {
        location: SourceLocation,
        symbol: Vec<u8>,
        kind: HoverKind,
        snapshot: NativeSnapshot,
        documentation_symbol: String,
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

    struct NativeCompletion {
        label: String,
        kind: CompletionKind,
        is_function: bool,
        snapshots: Vec<NativeSnapshot>,
        documentation_symbol: String,
        documentation: String,
        insert_text: String,
        is_snippet: bool,
        replacement: SourceLocation,
        replace_dot: bool,
        operation: Range,
        is_deprecated: bool,
    }

    struct NativeParameter {
        name: String,
        subject: usize,
        is_pack: bool,
        source_index: u32,
        documentation: String,
        is_variadic: bool,
    }

    struct NativeSignature {
        identity: String,
        snapshot: NativeSnapshot,
        documentation_symbol: String,
        documentation: String,
        parameters: Vec<NativeParameter>,
        active_parameter: u32,
        is_selected: bool,
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

    /// A direct semantic inlay selector.
    #[derive(Clone, Debug, Eq, PartialEq)]
    pub struct HintSelector {
        /// Exact native module identity.
        identity: String,

        /// Zero-width native insertion position.
        position: Position,

        /// Selected subject category.
        kind: HintKind,
    }

    struct NativeHint {
        location: SourceLocation,
        name: String,
        snapshot: NativeSnapshot,
        kind: HintKind,
        is_obvious: bool,
        documentation_symbol: String,
        documentation: String,
        has_parameter_index: bool,
        parameter_index: u32,
    }

    #[repr(u8)]
    enum NativeTypeKind {
        Primitive = 0,
        Singleton = 1,
        Named = 2,
        Generic = 3,
        Function = 4,
        Table = 5,
        Union = 6,
        Intersection = 7,
        Metatable = 8,
        Negation = 9,
        Application = 10,
        Reference = 11,
        Unresolved = 12,
        Omitted = 13,
    }

    #[repr(u8)]
    enum NativePackKind {
        Sequence = 0,
        Variadic = 1,
        Generic = 2,
        Application = 3,
        Reference = 4,
        Unresolved = 5,
        Omitted = 6,
    }

    #[repr(u8)]
    enum NativePrimitiveState {
        Nil = 0,
        Boolean = 1,
        Number = 2,
        Integer = 3,
        String = 4,
        Thread = 5,
        Function = 6,
        Table = 7,
        Buffer = 8,
        Any = 9,
        Unknown = 10,
        Never = 11,
        Invalid = 255,
    }

    #[repr(u8)]
    enum NativeSingletonState {
        Boolean = 0,
        String = 1,
    }

    #[repr(u8)]
    enum NativeProvenance {
        Alias = 0,
        External = 1,
        Synthetic = 2,
        Unverified = 3,
    }

    #[repr(u8)]
    enum NativeUnresolvedState {
        Free = 0,
        Blocked = 1,
        Lazy = 2,
        Error = 3,
        InternalRefinement = 4,
    }

    #[repr(u8)]
    enum NativeTableState {
        Sealed = 0,
        Unsealed = 1,
        Free = 2,
        Generic = 3,
        Exact = 4,
        Invalid = 255,
    }

    #[repr(u8)]
    enum NativeApplicationState {
        Pending = 0,
        Unreduced = 1,
    }

    #[derive(Copy, Clone)]
    struct NativeIndex {
        has_value: bool,
        index: usize,
    }

    struct NativeApplication {
        name: String,
        qualifier: Text,
        type_arguments: Vec<usize>,
        pack_arguments: Vec<usize>,
        state: NativeApplicationState,
    }

    struct NativeFunction {
        generics: Vec<usize>,
        generic_packs: Vec<usize>,
        arguments: usize,
        returns: usize,
        argument_names: Vec<Text>,
        is_checked: bool,
    }

    struct NativeProperty {
        name: Vec<u8>,
        read: NativeIndex,
        write: NativeIndex,
        is_shared: bool,
    }

    struct NativeIndexer {
        key: usize,
        value: usize,
        is_read_only: bool,
    }

    struct NativeTable {
        properties: Vec<NativeProperty>,
        indexers: Vec<NativeIndexer>,
        state: NativeTableState,
    }

    struct NativeType {
        kind: NativeTypeKind,
        state: u8,
        name: String,
        has_explicit_name: bool,
        qualifier: Text,
        identity: usize,
        bytes: Vec<u8>,
        boolean: bool,
        types: Vec<usize>,
        packs: Vec<usize>,
        expansion: NativeIndex,
        functions: Vec<NativeFunction>,
        tables: Vec<NativeTable>,
        applications: Vec<NativeApplication>,
    }

    struct NativePack {
        kind: NativePackKind,
        state: u8,
        name: String,
        has_explicit_name: bool,
        identity: usize,
        types: Vec<usize>,
        packs: Vec<usize>,
        tail: NativeIndex,
        is_hidden: bool,
        applications: Vec<NativeApplication>,
    }

    struct NativeSnapshot {
        types: Vec<NativeType>,
        packs: Vec<NativePack>,
        root: usize,
        is_root_pack: bool,
        is_expanded: bool,
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
        has_value: bool,
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

        fn progress(self: &Context<'_>, module: &str, is_completed: bool) -> bool;

        fn advance(self: &Context<'_>, completed: usize, total: usize) -> bool;
    }

    unsafe extern "C++" {
        include!("native/frontend.hpp");
        include!("native/environment.hpp");
        include!("native/cancellation.hpp");
        type Frontend<'host>;

        #[namespace = "Luau"]
        type FrontendCancellationToken;

        fn create<'host>(
            host: &'host Context<'host>,
            retain_types: bool,
        ) -> Result<UniquePtr<Frontend<'host>>>;

        fn check(self: Pin<&mut Frontend<'_>>, module: &str) -> Result<Vec<Diagnostic>>;

        fn queue(self: Pin<&mut Frontend<'_>>, module: &str) -> Result<()>;

        fn checks(self: &Frontend<'_>) -> usize;

        fn invalidate(self: Pin<&mut Frontend<'_>>, module: &str) -> Result<()>;

        fn refresh(self: Pin<&mut Frontend<'_>>) -> Result<()>;

        fn clear(self: Pin<&mut Frontend<'_>>) -> Result<()>;

        fn hover(
            self: Pin<&mut Frontend<'_>>,
            module: &str,
            line: u32,
            column: u32,
        ) -> Result<Vec<NativeHover>>;

        fn completion(
            self: Pin<&mut Frontend<'_>>,
            module: &str,
            line: u32,
            column: u32,
        ) -> Result<Vec<NativeCompletion>>;

        fn completion_detail(
            self: Pin<&mut Frontend<'_>>,
            module: &str,
            line: u32,
            column: u32,
            index: usize,
        ) -> Result<Vec<NativeCompletion>>;

        fn signatures(
            self: Pin<&mut Frontend<'_>>,
            module: &str,
            line: u32,
            column: u32,
        ) -> Result<Vec<NativeSignature>>;

        fn definitions(
            self: Pin<&mut Frontend<'_>>,
            module: &str,
            line: u32,
            column: u32,
            resolve_types: bool,
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

        fn hints_range(
            self: Pin<&mut Frontend<'_>>,
            module: &str,
            start: Position,
            end: Position,
            is_expanded: bool,
        ) -> Result<Vec<NativeHint>>;

        fn hint_subjects(
            self: Pin<&mut Frontend<'_>>,
            selectors: &[HintSelector],
        ) -> Result<Vec<NativeHint>>;

        fn imports(self: Pin<&mut Frontend<'_>>, module: &str) -> Result<Vec<Import>>;
        fn cancellation(self: &Frontend<'_>) -> SharedPtr<FrontendCancellationToken>;

        fn declarations(configuration: &DeclarationFlags<'_>) -> Result<String>;

        fn cancel(token: &SharedPtr<FrontendCancellationToken>);
        fn reset(token: &SharedPtr<FrontendCancellationToken>);
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

unsafe impl Send for bindings::FrontendCancellationToken {}
unsafe impl Sync for bindings::FrontendCancellationToken {}

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
            Mode::NonStrict => bindings::Mode::NonStrict,
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
                    is_service: class.service,
                    is_creatable: class.creatable,
                    properties: class
                        .properties
                        .into_iter()
                        .map(|property| bindings::Property {
                            name: property.name,
                            can_read: property.read,
                            can_write: property.write,
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
            has_value: value.is_some(),
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
