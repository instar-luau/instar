#[cxx::bridge(namespace = "instar::luau")]
pub(crate) mod bindings {
    #[derive(Debug)]
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
        should_report_type_errors: bool,
    }

    #[derive(Clone, Debug)]
    struct Language {
        mode: Mode,
        globals: Vec<String>,
        aliases: Vec<Alias>,
        should_report_type_errors: bool,
    }

    #[derive(Clone, Debug)]
    struct Alias {
        name: String,
        value: String,
        configuration: String,
    }

    enum FlagKind {
        Boolean,
        Integer,
    }

    struct Flag {
        name: String,
        kind: FlagKind,
        is_enabled: bool,
        integer: i32,
    }

    struct BuiltinFlags<'flags> {
        settings: &'flags [Flag],
        scoped: &'flags [Flag],
    }

    /// A symbol and its documentation identity from Luau's builtin registries.
    #[derive(Clone, Debug)]
    pub struct Builtin {
        /// Qualified symbol name.
        name: String,

        /// Documentation identity assigned by Luau, or empty when absent.
        documentation: String,

        /// Whether the symbol belongs to a type.
        is_type: bool,

        /// Whether the symbol is a member rather than a root binding.
        is_member: bool,

        /// Whether the symbol belongs to the type-function environment.
        is_type_function: bool,
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

    /// Catalog identity and ordered source declarations for semantic documentation.
    #[derive(Clone, Debug, Eq, PartialEq)]
    pub struct Documentation {
        /// Native documentation catalog symbol.
        symbol: String,

        /// Canonical selected overload signature, empty for an unselected overload set.
        signature: String,

        /// Source declarations in lookup priority order.
        declarations: Vec<SourceLocation>,
    }

    struct NativeHover {
        location: SourceLocation,
        presentation: String,
        documentation: Documentation,
        is_namespace: bool,
        has_receiver: bool,
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

    /// Recommended call parentheses for a completion insertion.
    #[derive(Copy, Clone, Debug, Eq, PartialEq)]
    pub enum Parentheses {
        /// Insert the completion text alone.
        None,

        /// Place the cursor after the closing parenthesis.
        CursorAfter,

        /// Place the cursor between the parentheses.
        CursorInside,
    }

    struct NativeCompletion {
        label: String,
        kind: CompletionKind,
        is_function: bool,
        has_receiver: bool,
        presentations: Vec<String>,
        is_complete: bool,
        equivalence: usize,
        documentation: Documentation,
        insert_text: String,
        parentheses: Parentheses,
        replacement: SourceLocation,
        should_replace_dot: bool,
        operation: Range,
        is_deprecated: bool,
    }

    struct NativeParameter {
        name: String,
        start: usize,
        end: usize,
        source_index: u32,
        is_variadic: bool,
    }

    struct NativeSignature {
        label: String,
        expanded: String,
        documentation: Documentation,
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
        is_type_namespace: bool,
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
        presentation: NativePresentation,
        kind: HintKind,
        is_obvious: bool,
        documentation: Documentation,
        has_parameter_index: bool,
        parameter_index: u32,
    }

    struct NativePresentation {
        compact: String,
        expanded: String,
        is_complete: bool,
        is_pack: bool,
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

        /// Whether the target uses instance navigation.
        is_instance: bool,
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

        fn children(self: &Context<'_>, module: &str) -> Result<Vec<String>>;

        fn alias(
            self: &Context<'_>,
            module: &str,
            name: &str,
            destination: &mut String,
        ) -> Result<Status>;

        fn aliases(self: &Context<'_>, module: &str) -> Result<Vec<String>>;

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
        include!("native/configuration.hpp");

        fn configure(
            previous: &[Language],
            name: &str,
            source: &str,
            is_executable: bool,
        ) -> Result<Language>;

        type Frontend<'host>;

        #[namespace = "Luau"]
        type FrontendCancellationToken;

        fn create<'host>(
            host: &'host Context<'host>,
            should_retain_types: bool,
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
            should_resolve_types: bool,
        ) -> Result<Vec<SourceLocation>>;

        fn occurrences(
            self: Pin<&mut Frontend<'_>>,
            module: &str,
            declarations: &[Occurrence],
        ) -> Result<Vec<Occurrence>>;

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

        fn builtins(configuration: &BuiltinFlags<'_>, symbols: &mut Vec<Builtin>) -> Result<bool>;

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
            should_report_type_errors,
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
            should_report_type_errors,
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
                    source: definition.source.to_string(),
                })
                .collect(),
            has_enumerations: environment.enumerations.is_some(),
            enumerations: environment
                .enumerations
                .as_deref()
                .into_iter()
                .flat_map(BTreeMap::iter)
                .map(|(name, items)| bindings::Enumeration {
                    name: name.clone(),
                    items: items.clone(),
                })
                .collect(),
            has_capabilities: environment.classes.is_some(),
            classes: environment
                .classes
                .as_deref()
                .unwrap_or_default()
                .iter()
                .map(|class| bindings::Class {
                    name: class.name.clone(),
                    is_service: class.is_service,
                    is_creatable: class.is_creatable,
                    properties: class
                        .properties
                        .iter()
                        .map(|property| bindings::Property {
                            name: property.name.clone(),
                            can_read: property.can_read,
                            can_write: property.can_write,
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
            Flag::Boolean(is_enabled) => bindings::Flag {
                name,
                kind: bindings::FlagKind::Boolean,
                is_enabled,
                integer: 0,
            },

            Flag::Integer(integer) => bindings::Flag {
                name,
                kind: bindings::FlagKind::Integer,
                is_enabled: false,
                integer,
            },
        })
        .collect()
}
