/// Typed CXX interface to native Luau operations.
#[cxx::bridge(namespace = "instar")]
pub mod native {
    /// Kind of source supplied to the checker.
    #[derive(Debug)]
    #[repr(u32)]
    enum SourceKind {
        /// Source is unavailable.
        SourceUnknown,

        /// Source is a module.
        SourceModule,

        /// Source is a script.
        SourceScript,
    }

    /// Severity of a native diagnostic.
    #[derive(Debug)]
    #[repr(u32)]
    enum DiagnosticSeverity {
        /// Error diagnostic.
        DiagnosticError,

        /// Warning diagnostic.
        DiagnosticWarning,

        /// Informational diagnostic.
        DiagnosticInformation,
    }

    /// Native operation outcome.
    enum FailureKind {
        /// The operation succeeded.
        Success,

        /// The native operation failed.
        Operation,

        /// A host callback rejected the operation.
        Callback,

        /// The definition source was rejected.
        Definition,
    }

    /// Native operation outcome with best-effort error text.
    #[must_use]
    struct Failure {
        /// Outcome category.
        kind: FailureKind,

        /// Error text when allocation succeeded.
        message: UniquePtr<CxxString>,
    }

    /// Compiled Luau fast flag and its current value.
    struct FastFlag {
        /// Full flag name including its prefix.
        name: String,

        /// Whether the flag has a boolean value.
        boolean: bool,

        /// Boolean value when boolean is true.
        bool_value: bool,

        /// Integer value when boolean is false.
        int_value: i32,
    }

    /// Borrowed source supplied by the host.
    struct SourceData<'a> {
        /// Source bytes.
        bytes: &'a [u8],

        /// Source category.
        kind: SourceKind,

        /// Whether the source callback succeeded.
        success: bool,
    }

    /// Borrowed module-resolution request.
    #[derive(Clone, Copy)]
    struct Resolution<'a> {
        /// Original source module.
        from: &'a str,

        /// Preceding navigation result when present.
        context: &'a str,

        /// Whether a navigation context is present.
        has_context: bool,

        /// Whether resolution failure is allowed.
        optional: bool,

        /// Expression line and column range.
        location: [u32; 4],
    }

    /// Module-resolution outcome.
    struct Resolved {
        /// Resolved module identity when present.
        path: String,

        /// Whether a module was resolved.
        present: bool,

        /// Whether the resolution callback succeeded.
        success: bool,
    }

    /// Borrowed native diagnostic.
    #[derive(Clone, Copy)]
    struct DiagnosticData<'a> {
        /// Source module.
        path: &'a str,

        /// Diagnostic line and column range.
        location: [u32; 4],

        /// Diagnostic severity.
        severity: DiagnosticSeverity,

        /// Diagnostic message.
        message: &'a str,

        /// Lint rule name, empty for other diagnostics.
        rule: &'a str,

        /// Whether related information is present.
        has_related: bool,

        /// Related source module.
        related_path: &'a str,

        /// Related line and column range.
        related_location: [u32; 4],

        /// Related diagnostic message.
        related_message: &'a str,
    }

    /// Roblox class metadata supplied to the checker.
    struct RobloxClass<'a> {
        /// Class name.
        name: &'a str,

        /// Whether the class is a service.
        service: bool,

        /// Whether instances can be created.
        creatable: bool,
    }

    /// Borrowed Roblox hierarchy node.
    struct RobloxNode<'a> {
        /// Instance name.
        name: &'a str,

        /// Declared class name.
        class_name: &'a str,

        /// Parent index, or `usize::MAX` for a root.
        parent: usize,

        /// Whether a source module is present.
        has_module: bool,

        /// Source module whose script global refers to this node.
        module: &'a str,
    }

    /// Completion item kind.
    #[derive(Debug, Default)]
    #[repr(u32)]
    enum EditorCompletionKind {
        /// Plain text completion.
        #[default]
        CompletionText,

        /// Method completion.
        CompletionMethod,

        /// Function completion.
        CompletionFunction,

        /// Constructor completion.
        CompletionConstructor,

        /// Field completion.
        CompletionField,

        /// Variable completion.
        CompletionVariable,

        /// Class completion.
        CompletionClass,

        /// Interface completion.
        CompletionInterface,

        /// Module completion.
        CompletionModule,

        /// Property completion.
        CompletionProperty,

        /// Unit completion.
        CompletionUnit,

        /// Value completion.
        CompletionValue,

        /// Enum completion.
        CompletionEnum,

        /// Keyword completion.
        CompletionKeyword,

        /// Snippet completion.
        CompletionSnippet,

        /// Color completion.
        CompletionColor,

        /// File completion.
        CompletionFile,

        /// Reference completion.
        CompletionReference,

        /// Folder completion.
        CompletionFolder,

        /// Enum-member completion.
        CompletionEnumMember,

        /// Constant completion.
        CompletionConstant,

        /// Struct completion.
        CompletionStruct,

        /// Event completion.
        CompletionEvent,

        /// Operator completion.
        CompletionOperator,

        /// Type-parameter completion.
        CompletionTypeParameter,
    }

    /// Identity used to resolve a rename.
    #[derive(Debug, Default)]
    #[repr(u32)]
    enum EditorRenameKind {
        /// Lexically scoped variable.
        #[default]
        RenameLocal,

        /// Source-defined global variable.
        RenameGlobal,

        /// Table or class property.
        RenameProperty,

        /// Type alias.
        RenameType,
    }

    /// Hover information returned for a source position.
    #[derive(Default)]
    struct Hover {
        /// Symbol name.
        name: String,

        /// Displayed type.
        type_: String,

        /// Documentation symbol identifier.
        documentation_symbol: String,

        /// Whether a hover range is present.
        has_range: bool,

        /// Hover range when present.
        range: [u32; 4],

        /// Whether the selected symbol is a named type alias.
        is_type: bool,
    }

    /// Completion item returned for a source position.
    #[derive(Default)]
    struct Completion {
        /// Completion label.
        name: String,

        /// Completion detail.
        detail: String,

        /// Documentation symbol identifier.
        documentation_symbol: String,

        /// Completion insertion text.
        insert: String,

        /// Completion item kind.
        kind: EditorCompletionKind,

        /// Whether the item is deprecated.
        deprecated: bool,

        /// Source declaration module, empty when unavailable.
        definition_module: String,

        /// Source declaration used for comment documentation.
        definition: [u32; 4],
    }

    /// Signature help returned for a call site.
    #[derive(Default)]
    struct SignatureHelp {
        /// Signature label.
        label: String,

        /// Signature parameter labels.
        parameters: Vec<String>,

        /// Whether an active parameter is present.
        has_active_parameter: bool,

        /// Active parameter index when present.
        active_parameter: u32,
    }

    /// Semantic identity information for reference candidate selection.
    #[derive(Default)]
    struct ReferenceTarget {
        /// Selected symbol name.
        name: String,

        /// Whether occurrences are confined to the declaration module.
        local: bool,

        /// Whether the symbol is a property.
        property: bool,
    }

    /// Inferred type for a source range.
    #[derive(Default)]
    struct TypeHint {
        /// Type-hint range.
        range: [u32; 4],

        /// Inferred type text.
        type_: String,

        /// Parameter name for an argument hint, empty for a variable type hint.
        parameter: String,
    }

    /// Navigation target and selection range.
    #[derive(Default)]
    struct Navigation {
        /// Target source path.
        path: String,

        /// Full target range.
        range: [u32; 4],

        /// Name-selection range.
        selection: [u32; 4],
    }

    /// Reference occurrence.
    #[derive(Default)]
    struct Reference {
        /// Reference source path.
        path: String,

        /// Reference range.
        range: [u32; 4],

        /// Whether the reference is a declaration.
        declaration: bool,
    }

    /// Source-editable semantic rename target.
    #[derive(Default)]
    struct RenameTarget {
        /// Symbol name.
        name: String,

        /// Declaration module.
        path: String,

        /// Declaration name range.
        definition: [u32; 4],

        /// Name-selection range.
        selection: [u32; 4],

        /// Target identity category.
        kind: EditorRenameKind,
    }

    /// Optional restriction to exact module identities.
    #[derive(Clone, Copy)]
    struct Candidates<'a> {
        /// Whether the module list restricts the operation.
        restricted: bool,

        /// Permitted module identities when restricted.
        modules: &'a [String],
    }

    extern "Rust" {
        type Host<'a>;
        unsafe fn source<'a>(self: &'a mut Host<'_>, name: &str) -> SourceData<'a>;
        fn configuration(self: &mut Host<'_>, name: &str) -> *const Configuration;
        fn resolve(self: &mut Host<'_>, request: Resolution<'_>) -> Resolved;
        fn diagnostic(self: &mut Host<'_>, diagnostic: DiagnosticData<'_>) -> bool;

        type Items<'a>;
        fn item(self: &mut Items<'_>, value: &str) -> bool;
    }

    unsafe extern "C++" {
        include!("instar-bridge/native/bridge.hpp");
        include!("instar-bridge/native/editor.hpp");
        /// Native Luau configuration.
        type Configuration;
        /// Native Luau analysis state.
        type Checker;

        /// Creates a configuration from JSON, recording any failure.
        fn configuration_create(source: &[u8], failure: &mut Failure) -> UniquePtr<Configuration>;
        /// Creates a checker, recording any failure.
        fn checker_create(retain: bool, failure: &mut Failure) -> UniquePtr<Checker>;
        /// Enumerates compiled boolean and integer flags.
        fn fast_flags(output: &mut Vec<FastFlag>) -> Failure;
        /// Sets a known flag, rejecting name or type mismatches.
        fn set_fast_flag(name: &str, boolean: bool, bool_value: bool, int_value: i32) -> Failure;
        /// Invalidates a source module and its dependents.
        fn checker_mark_dirty(checker: Pin<&mut Checker>, name: &str) -> Failure;
        /// Clears source caches while retaining definitions and globals.
        fn checker_clear_sources(checker: Pin<&mut Checker>) -> Failure;
        /// Freezes the global type arena.
        fn checker_freeze(checker: Pin<&mut Checker>) -> Failure;
        /// Loads definitions and emits diagnostics through the host.
        fn checker_load_definition(
            checker: Pin<&mut Checker>,
            host: &mut Host<'_>,
            source: &[u8],
            package: &str,
        ) -> Failure;
        /// Registers Roblox class metadata and magic.
        fn checker_register_roblox_classes(
            checker: Pin<&mut Checker>,
            classes: &[RobloxClass<'_>],
        ) -> Failure;
        /// Registers validated instance types and module environments.
        fn checker_register_roblox_tree(
            checker: Pin<&mut Checker>,
            nodes: &[RobloxNode<'_>],
        ) -> Failure;
        /// Parses a source module.
        fn checker_parse(checker: Pin<&mut Checker>, host: &mut Host<'_>, name: &str) -> Failure;
        /// Emits cached parse diagnostics.
        fn checker_parse_diagnostics(
            checker: Pin<&mut Checker>,
            host: &mut Host<'_>,
            name: &str,
        ) -> Failure;
        /// Prepares semantic modules and collects timeout identities.
        fn checker_prepare(
            checker: Pin<&mut Checker>,
            host: &mut Host<'_>,
            name: &str,
            timeouts: &mut Vec<String>,
        ) -> Failure;
        /// Emits cached type diagnostics for a prepared module.
        fn checker_check(checker: Pin<&mut Checker>, host: &mut Host<'_>, name: &str) -> Failure;
        /// Runs typed lint on a prepared module.
        fn checker_lint(checker: Pin<&mut Checker>, host: &mut Host<'_>, name: &str) -> Failure;
        /// Enumerates globals, stopping when the consumer rejects an item.
        fn checker_globals(checker: &Checker, output: &mut Items<'_>) -> Failure;
        /// Enumerates modules, stopping when the consumer rejects an item.
        fn checker_modules(checker: &Checker, output: &mut Items<'_>) -> Failure;
        /// Attaches inferred type data to a checked module.
        fn checker_attach_type_data(checker: Pin<&mut Checker>, name: &str) -> Failure;
        /// Collects hover information at a source position.
        fn editor_hover(
            checker: Pin<&mut Checker>,
            host: &mut Host<'_>,
            name: &str,
            position: [u32; 2],
            output: &mut Vec<Hover>,
        ) -> Failure;
        /// Collects completion items at a source position.
        fn editor_completion(
            checker: Pin<&mut Checker>,
            host: &mut Host<'_>,
            name: &str,
            position: [u32; 2],
            output: &mut Vec<Completion>,
        ) -> Failure;
        /// Collects signature help at a source position.
        fn editor_signature_help(
            checker: Pin<&mut Checker>,
            host: &mut Host<'_>,
            name: &str,
            position: [u32; 2],
            output: &mut Vec<SignatureHelp>,
        ) -> Failure;
        /// Collects inferred type hints for a module.
        fn editor_type_hints(
            checker: Pin<&mut Checker>,
            host: &mut Host<'_>,
            name: &str,
            output: &mut Vec<TypeHint>,
        ) -> Failure;
        /// Collects definition targets at a source position.
        fn editor_definition(
            checker: Pin<&mut Checker>,
            host: &mut Host<'_>,
            name: &str,
            position: [u32; 2],
            output: &mut Vec<Navigation>,
        ) -> Failure;
        /// Collects declaration targets at a source position.
        fn editor_declaration(
            checker: Pin<&mut Checker>,
            host: &mut Host<'_>,
            name: &str,
            position: [u32; 2],
            output: &mut Vec<Navigation>,
        ) -> Failure;
        /// Collects concrete source implementation targets.
        fn editor_implementation(
            checker: Pin<&mut Checker>,
            host: &mut Host<'_>,
            name: &str,
            position: [u32; 2],
            output: &mut Vec<Navigation>,
        ) -> Failure;
        /// Collects type-definition targets at a source position.
        fn editor_type_definition(
            checker: Pin<&mut Checker>,
            host: &mut Host<'_>,
            name: &str,
            position: [u32; 2],
            output: &mut Vec<Navigation>,
        ) -> Failure;
        /// Collects references within the optional candidate restriction.
        fn editor_references(
            checker: Pin<&mut Checker>,
            host: &mut Host<'_>,
            name: &str,
            position: [u32; 2],
            candidates: Candidates<'_>,
            output: &mut Vec<Reference>,
        ) -> Failure;
        /// Resolves reference-candidate identity information.
        fn editor_reference_target(
            checker: Pin<&mut Checker>,
            host: &mut Host<'_>,
            name: &str,
            position: [u32; 2],
            output: &mut Vec<ReferenceTarget>,
        ) -> Failure;
        /// Resolves a source-editable rename target.
        fn editor_rename_target(
            checker: Pin<&mut Checker>,
            host: &mut Host<'_>,
            name: &str,
            position: [u32; 2],
            output: &mut Vec<RenameTarget>,
        ) -> Failure;
        /// Validates a rename and collects its affected occurrences.
        fn editor_rename(
            checker: Pin<&mut Checker>,
            host: &mut Host<'_>,
            name: &str,
            position: [u32; 2],
            new_name: &str,
            candidates: Candidates<'_>,
            output: &mut Vec<Reference>,
        ) -> Failure;
    }
}

use crate::{Host, Items};
