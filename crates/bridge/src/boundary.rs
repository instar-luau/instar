pub(super) use native::{
    NativeAlias, NativeCheck, NativeClass, NativeCompletion, NativeConfiguration, NativeDefinition,
    NativeDiagnostic, NativeDocument, NativeFact, NativeFactKind, NativeFlag, NativeFrontend,
    NativeHierarchy, NativeKind, NativeLink, NativeLint, NativeLintResult, NativeLocation,
    NativeModule, NativeProperty, NativeReference, NativeSignatures, NativeSite, NativeSnapshot,
    NativeSource, NativeSuggestion, NativeSuggestions, apply_flags, create, create_frontend,
    normalize_flags,
};

use crate::frontend::{Cancellation, Host};

#[cxx::bridge(namespace = "instar")]
mod native {
    struct NativeOutcome {
        message: String,
        timed_out: bool,
        cancelled: bool,
    }

    struct NativeLint {
        name: String,
        enabled: bool,
        fatal: bool,
    }

    struct NativeAlias {
        name: String,
        value: String,
        location: String,
        original_case: String,
    }

    struct NativeSnapshot {
        mode: String,
        lint: Vec<NativeLint>,
        lint_errors: bool,
        type_errors: bool,
        globals: Vec<String>,
        aliases: Vec<NativeAlias>,
    }

    struct NativeSite {
        call_start: usize,
        call_end: usize,
        argument_start: usize,
        argument_end: usize,
        static_request: bool,
        target: String,
    }

    struct NativeDocument {
        found: bool,
        text: String,
        revision: u64,
    }

    struct NativeModule {
        found: bool,
        revision: u64,
        sites: Vec<NativeSite>,
    }

    struct NativeSource {
        definitions: Vec<NativeDefinition>,
        classes: Vec<NativeClass>,
    }

    struct NativeLink {
        module: String,
        revision: u64,
        call_start: usize,
        call_end: usize,
        argument_start: usize,
        argument_end: usize,
        target: String,
    }

    struct NativeFlag {
        name: String,
        boolean: bool,
        integer: i32,
        is_boolean: bool,
    }

    struct NativeDefinition {
        name: String,

        #[cxx_name = "namespace_name"]
        namespace: String,

        revision: u64,
    }

    struct NativeClass {
        name: String,
        service: bool,
        creatable: bool,
        properties: Vec<NativeProperty>,
    }

    struct NativeProperty {
        name: String,
        read: bool,
        write: bool,
    }

    struct NativeLocation {
        module: String,
        revision: u64,
        start: usize,
        end: usize,
    }

    struct NativeHover {
        found: bool,
        location: NativeLocation,
        text: String,
        documentation: String,
        deprecated: bool,
    }

    struct NativeSuggestion {
        name: String,
        kind: u8,
        detail: String,
        documentation: String,
        insert_text: String,
        parentheses: u8,
        deprecated: bool,
        callable: bool,
        preferred: bool,
        notation: u8,
    }

    struct NativeSuggestions {
        location: NativeLocation,
        context: u8,
        index_start: usize,
        index_end: usize,
        has_index: bool,
        require: bool,
        loader: bool,
        prefix: String,
        has_prefix: bool,
        entries: Vec<NativeSuggestion>,
    }

    struct NativeParameter {
        start: usize,
        end: usize,
    }

    struct NativeSignature {
        label: String,
        parameters: Vec<NativeParameter>,
        variadic: bool,
    }

    struct NativeSignatures {
        found: bool,
        location: NativeLocation,
        entries: Vec<NativeSignature>,
        active: usize,
        has_active: bool,
        argument: usize,
        documentation: String,
    }

    struct NativeReference {
        location: NativeLocation,
        targets: Vec<NativeLocation>,
        documentation: String,
        name: String,
        kind: u8,
        declaration: bool,
        write: bool,
        literal: bool,
        function: bool,
        parameter: bool,
        deprecated: bool,
    }

    struct NativeHierarchyItem {
        name: String,
        kind: u8,
        location: NativeLocation,
        selection_start: usize,
        selection_end: usize,
    }

    struct NativeCall {
        caller: usize,
        callee: usize,
        location: NativeLocation,
    }

    struct NativeInheritance {
        child: usize,
        parent: usize,
    }

    struct NativeHierarchy {
        items: Vec<NativeHierarchyItem>,
        calls: Vec<NativeCall>,
        inheritance: Vec<NativeInheritance>,
        selected: Vec<usize>,
    }

    struct NativeRelated {
        location: NativeLocation,
        message: String,
    }

    enum NativeKind {
        Syntax,
        Type,
        Resolution,
        Analysis,
    }

    struct NativeDiagnostic {
        location: NativeLocation,
        kind: NativeKind,
        code: i32,
        message: String,
        related: Vec<NativeRelated>,
    }

    enum NativeCompletion {
        Complete,
        Cancelled,
        Timeout,
        Environment,
        Analysis,
    }

    struct NativeCheck {
        diagnostics: Vec<NativeDiagnostic>,
        completion: NativeCompletion,
    }

    struct NativeWarning {
        location: NativeLocation,
        code: i32,
        name: String,
        message: String,
        fatal: bool,
    }

    enum NativeFactKind {
        ImplicitAnyLocal,
        ImplicitAnyParameter,
    }

    struct NativeFact {
        location: NativeLocation,
        kind: NativeFactKind,
        message: String,
    }

    struct NativeLintResult {
        warnings: Vec<NativeWarning>,
        facts: Vec<NativeFact>,
        diagnostics: Vec<NativeDiagnostic>,
        completion: NativeCompletion,
    }

    extern "Rust" {
        type Host;
        fn progress(
            self: &Host,
            phase: &str,
            source: &str,
            completed: usize,
            total: usize,
        ) -> Result<()>;
        fn read_module(self: &Host, name: &str) -> NativeModule;
        fn read_source(self: &Host, name: &str) -> NativeSource;
        fn read_document(self: &Host, name: &str) -> NativeDocument;
        fn revision(self: &Host, name: &str) -> Result<u64>;
        fn offset(self: &Host, name: &str, line: usize, column: usize) -> Result<usize>;
        fn require(self: &Host, name: &str, start: usize, end: usize) -> bool;
        fn resolve(self: &Host, name: &str, start: usize, end: usize) -> String;
        type Cancellation;
        fn requested(self: &Cancellation) -> bool;
        fn started(self: &Cancellation);
    }

    unsafe extern "C++" {
        include!("native/configuration.hpp");
        include!("native/flags.hpp");
        include!("native/frontend.hpp");
        type NativeConfiguration;
        type NativeFrontend;
        fn apply_flags(flags: &[NativeFlag]) -> Result<()>;
        fn normalize_flags(flags: &[NativeFlag]) -> Result<Vec<NativeFlag>>;
        fn create_frontend() -> Result<UniquePtr<NativeFrontend>>;
        fn documentation(self: &NativeFrontend, module: &str, symbol: &str) -> Result<String>;
        fn retain(self: Pin<&mut NativeFrontend>) -> Result<()>;
        fn hover(
            self: Pin<&mut NativeFrontend>,
            host: &Host,
            module: &str,
            line: u32,
            column: u32,
        ) -> Result<NativeHover>;
        fn definition(
            self: Pin<&mut NativeFrontend>,
            host: &Host,
            module: &str,
            line: u32,
            column: u32,
            type_definition: bool,
        ) -> Result<Vec<NativeLocation>>;
        fn suggest(
            self: Pin<&mut NativeFrontend>,
            host: &Host,
            module: &str,
            line: u32,
            column: u32,
        ) -> Result<NativeSuggestions>;
        fn signatures(
            self: Pin<&mut NativeFrontend>,
            host: &Host,
            module: &str,
            line: u32,
            column: u32,
        ) -> Result<NativeSignatures>;
        fn references(
            self: Pin<&mut NativeFrontend>,
            host: &Host,
            modules: &[String],
        ) -> Result<Vec<NativeReference>>;
        fn hierarchy(
            self: Pin<&mut NativeFrontend>,
            host: &Host,
            modules: &[String],
            module: &str,
            position: &[u32],
        ) -> Result<NativeHierarchy>;
        fn configure(
            self: Pin<&mut NativeFrontend>,
            name: &str,
            configuration: &NativeConfiguration,
        ) -> Result<()>;
        fn prepare(
            self: Pin<&mut NativeFrontend>,
            host: &Host,
            names: &[String],
        ) -> Result<Vec<NativeLink>>;
        fn invalidate(self: Pin<&mut NativeFrontend>, names: &[String]) -> Result<()>;
        fn check(
            self: Pin<&mut NativeFrontend>,
            host: &Host,
            entries: &[String],
            timeout_seconds: f64,
            names: &[String],
            cancellation: &Cancellation,
        ) -> Result<NativeCheck>;
        fn lint(
            self: Pin<&mut NativeFrontend>,
            host: &Host,
            entries: &[String],
            timeout_seconds: f64,
            names: &[String],
            cancellation: &Cancellation,
            semantic_modules: &[String],
        ) -> Result<NativeLintResult>;
        fn create() -> Result<UniquePtr<NativeConfiguration>>;
        fn apply(
            self: Pin<&mut NativeConfiguration>,
            source: &str,
            path: &str,
            executable: bool,
            timeout_seconds: f64,
            cancellation: &Cancellation,
        ) -> Result<NativeOutcome>;
        fn snapshot(self: &NativeConfiguration) -> Result<NativeSnapshot>;
        fn restore(self: Pin<&mut NativeConfiguration>, snapshot: &NativeSnapshot) -> Result<()>;
    }
}
