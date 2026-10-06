use crate::frontend::{Cancellation, Host};

pub(super) use native::{
    NativeAlias, NativeClass, NativeCompletion, NativeConfiguration, NativeDefinition,
    NativeDiagnostic, NativeFact, NativeFactKind, NativeFrontend, NativeKind, NativeLint,
    NativeLocation, NativeProperty, NativeSite, NativeSnapshot, NativeSource, create,
    create_frontend,
};

#[cxx::bridge(namespace = "instar")]
mod native {
    struct NativeOutcome {
        message: String,
        timed_out: bool,
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

    struct NativeSource {
        found: bool,
        text: String,
        revision: u64,
        sites: Vec<NativeSite>,
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

    struct NativeDefinition {
        name: String,
        text: String,
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
        fn read_source(self: &Host, name: &str) -> NativeSource;
        fn resolve(self: &Host, name: &str, start: usize, end: usize) -> String;
        type Cancellation;
        fn requested(self: &Cancellation) -> bool;
    }

    unsafe extern "C++" {
        include!("src/configuration.hpp");
        include!("src/frontend.hpp");

        type NativeConfiguration;
        type NativeFrontend;

        fn create_frontend() -> Result<UniquePtr<NativeFrontend>>;
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
        ) -> Result<NativeOutcome>;
        fn snapshot(self: &NativeConfiguration) -> Result<NativeSnapshot>;
        fn restore(self: Pin<&mut NativeConfiguration>, snapshot: &NativeSnapshot) -> Result<()>;
    }
}
