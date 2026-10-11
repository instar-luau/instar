#![expect(
    clippy::inline_modules,
    reason = "CXX bridge declarations require an inline module"
)]

pub(crate) use self::native::{
    Engine, NativeDiagnostic, NativeDocument, NativeFlag, NativeModule, NativeSource, apply_flags,
    create_engine, evaluate_configuration, normalize_flags,
};

use crate::worker::operation::NativeOperation;

#[cxx::bridge(namespace = "instar")]
pub(crate) mod native {
    struct NativeSite {
        call_start: usize,
        call_end: usize,
        argument_start: usize,
        argument_end: usize,
        static_resolution: bool,
        target: String,
        request_kind: u8,
        request_text: String,
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
        classes: Vec<NativeClass>,
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
        text: String,
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
        is_any: bool,
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
        declaration: NativeLocation,
        has_declaration: bool,
    }

    struct NativeSignature {
        label: String,
        parameters: Vec<NativeParameter>,
        variadic: bool,
        minimum_arguments: usize,
        has_minimum_arguments: bool,
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
        read: bool,
        annotation: bool,
        binding_kind: u8,
        type_text: String,
        has_type: bool,
        is_any: bool,
        scope: NativeLocation,
        shadow: NativeLocation,
        has_shadow: bool,
        type_declarations: Vec<NativeLocation>,
    }

    struct NativeHierarchyItem {
        name: String,
        kind: u8,
        location: NativeLocation,
        selection_start: usize,
        selection_end: usize,
        body_start: usize,
        body_end: usize,
        signatures: NativeSignatures,
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
        inheritance_complete: bool,
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
        Resource,
        Unsupported,
    }

    struct NativeInstance {
        identity: String,
        name: String,
        classification: String,
        parent: String,
        children: Vec<String>,
    }

    struct NativeModuleInput {
        name: String,
        text: String,
        revision: u64,
        logical_revision: u64,
        signature: String,
        environment_signature: String,
        declaration_syntax: bool,
        inference: bool,
        mode: u8,
        reporting: bool,
        retained: Vec<u8>,
        has_environment: bool,
        environment_identity: u64,
        globals: Vec<String>,
        definitions: Vec<NativeDefinition>,
        classes: Vec<NativeClass>,
        has_instances: bool,
        instance_root: String,
        instance_script: String,
        instances: Vec<NativeInstance>,
        sites: Vec<NativeSite>,
    }

    struct NativeAnalysisRequest {
        signature: String,
        purpose: u8,
        roots: Vec<String>,
        modules: Vec<NativeModuleInput>,
    }

    struct NativeDirective {
        location: NativeLocation,
        text: String,
    }

    struct NativeModuleAnalysis {
        name: String,
        logical_revision: u64,
        parsed: bool,
        inferred: bool,
        semantic_complete: bool,
        recovered_syntax: bool,
        environment_available: bool,
        diagnostics: Vec<NativeDiagnostic>,
        environment_diagnostics: Vec<NativeDiagnostic>,
        directives: Vec<NativeDirective>,
        completion: NativeCompletion,
    }

    struct NativeAnalysisResult {
        frontend_revision: u64,
        modules: Vec<NativeModuleAnalysis>,
        completion: NativeCompletion,
    }

    struct NativeFactsInput {
        name: String,
        logical_revision: u64,
        frontend_revision: u64,
        purpose: u8,
        categories: Vec<u8>,
    }

    struct NativePositionInput {
        name: String,
        logical_revision: u64,
        frontend_revision: u64,
        purpose: u8,
        offset: usize,
        line: u32,
        column: u32,
    }

    struct NativeLintInput {
        name: String,
        logical_revision: u64,
        frontend_revision: u64,
        purpose: u8,
        mode: u8,
        warnings: Vec<String>,
    }

    struct NativeExpression {
        location: NativeLocation,
        has_type: bool,
        type_text: String,
        is_any: bool,
        constant_kind: u8,
        constant_text: String,
        constant_number: f64,
        constant_boolean: bool,
        literal: bool,
        dependency: NativeLocation,
        has_dependency: bool,
        read: bool,
        write: bool,
        builtin_dependency: String,
        known_effects: bool,
        type_declarations: Vec<NativeLocation>,
    }

    struct NativeScope {
        location: NativeLocation,
        parent: NativeLocation,
        has_parent: bool,
    }

    struct NativeCallEvidence {
        location: NativeLocation,
        callee: NativeLocation,
        signatures: NativeSignatures,
        arguments: Vec<NativeLocation>,
        argument_count: usize,
        has_argument_count: bool,
    }

    struct NativeEnvironmentBinding {
        name: String,
        kind: u8,
        type_text: String,
        is_any: bool,
        documentation: String,
        declaration: NativeLocation,
        has_declaration: bool,
        deprecated: bool,
    }

    struct NativeFactsResult {
        references: Vec<NativeReference>,
        hierarchy: NativeHierarchy,
        expressions: Vec<NativeExpression>,
        scopes: Vec<NativeScope>,
        calls: Vec<NativeCallEvidence>,
        environment_bindings: Vec<NativeEnvironmentBinding>,
        evidence_complete: bool,
        completion: NativeCompletion,
    }

    struct NativeInspectionResult {
        hover: NativeHover,
        definitions: Vec<NativeLocation>,
        type_definitions: Vec<NativeLocation>,
        signatures: NativeSignatures,
        hierarchy: NativeHierarchy,
        completion: NativeCompletion,
    }

    struct NativeCompletions {
        suggestions: NativeSuggestions,
        completion: NativeCompletion,
    }

    struct NativeWarning {
        location: NativeLocation,
        code: i32,
        name: String,
        message: String,
        related: Vec<NativeRelated>,
    }

    struct NativeWarningAvailability {
        name: String,
        status: u8,
    }

    struct NativeLintResult {
        warnings: Vec<NativeWarning>,
        suppression_warnings: Vec<NativeWarning>,
        availability: Vec<NativeWarningAvailability>,
        diagnostics: Vec<NativeDiagnostic>,
        completion: NativeCompletion,
    }

    struct NativeConfigurationInput {
        text: String,
        base: String,
        executable: bool,
        memory_bytes: u64,
        extraction_depth: u32,
        extraction_entries: u32,
        root: u32,
        nodes: Vec<NativeConfigurationNode>,
    }

    struct NativeConfigurationEntry {
        key_kind: u8,
        key_text: String,
        key_number: f64,
        value_index: u32,
    }

    struct NativeConfigurationNode {
        kind: u8,
        text: String,
        number: f64,
        boolean: bool,
        entries: Vec<NativeConfigurationEntry>,
    }

    struct NativeConfigurationResult {
        has_values: bool,
        root: u32,
        nodes: Vec<NativeConfigurationNode>,
        message: String,
        completion: NativeCompletion,
    }

    extern "Rust" {
        type NativeOperation;
        fn progress(
            self: &NativeOperation,
            phase: &str,
            source: &str,
            completed: usize,
            total: usize,
        ) -> Result<()>;
        fn read_module(self: &NativeOperation, name: &str) -> NativeModule;
        fn read_source(self: &NativeOperation, name: &str) -> NativeSource;
        fn read_document(self: &NativeOperation, name: &str) -> NativeDocument;
        fn revision(self: &NativeOperation, name: &str) -> Result<u64>;
        fn offset(self: &NativeOperation, name: &str, line: usize, column: usize) -> Result<usize>;
        fn require(self: &NativeOperation, name: &str, start: usize, end: usize) -> bool;
        fn requested(self: &NativeOperation) -> bool;
        fn remaining_nanoseconds(self: &NativeOperation) -> u64;
        fn finding(self: &NativeOperation, diagnostic: &NativeDiagnostic) -> Result<()>;
        fn log(self: &NativeOperation, level: u8, message: &str) -> Result<()>;
    }

    unsafe extern "C++" {
        include!("native/engine.hpp");
        include!("native/configuration.hpp");
        include!("native/flags.hpp");
        type Engine;
        fn create_engine() -> Result<UniquePtr<Engine>>;
        fn analyze(
            self: Pin<&mut Engine>,
            input: &NativeAnalysisRequest,
            operation: &NativeOperation,
        ) -> Result<NativeAnalysisResult>;
        fn facts(
            self: Pin<&mut Engine>,
            input: &NativeFactsInput,
            operation: &NativeOperation,
        ) -> Result<NativeFactsResult>;
        fn lint(
            self: Pin<&mut Engine>,
            input: &NativeLintInput,
            operation: &NativeOperation,
        ) -> Result<NativeLintResult>;
        fn inspect(
            self: Pin<&mut Engine>,
            input: &NativePositionInput,
            operation: &NativeOperation,
        ) -> Result<NativeInspectionResult>;
        fn complete(
            self: Pin<&mut Engine>,
            input: &NativePositionInput,
            operation: &NativeOperation,
        ) -> Result<NativeCompletions>;
        fn evaluate_configuration(
            input: &NativeConfigurationInput,
            operation: &NativeOperation,
        ) -> Result<NativeConfigurationResult>;
        fn apply_flags(flags: &[NativeFlag]) -> Result<()>;
        fn normalize_flags(flags: &[NativeFlag]) -> Result<Vec<NativeFlag>>;
    }
}
