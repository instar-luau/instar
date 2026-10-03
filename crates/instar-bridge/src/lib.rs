//! Native Luau analysis bridge.

#![expect(unsafe_code, reason = "the bridge owns the CXX boundary")]

use std::{collections::BTreeMap, io, path::Path, pin::Pin, ptr};

mod bridge;
mod editor;

pub use bridge::native;

pub use native::{
    Completion, DiagnosticSeverity, EditorCompletionKind, EditorRenameKind, Hover, Navigation,
    Reference, ReferenceTarget, RenameTarget, RobloxClass, SignatureHelp, SourceKind, TypeHint,
};

/// Rejected definition source.
#[derive(Debug)]
pub struct DefinitionFailure {
    message: String,
}

impl std::fmt::Display for DefinitionFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for DefinitionFailure {}

/// Current value of a supported compiled Luau fast flag.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FastFlagValue {
    /// Boolean flag value.
    Bool(bool),

    /// Integer flag value.
    Int(i32),
}

/// Returns compiled bool and int fast flags by their full Roblox names.
///
/// # Errors
/// Returns an error if the native registry cannot be enumerated.
pub fn fast_flags() -> io::Result<BTreeMap<String, FastFlagValue>> {
    let mut values = Vec::new();
    native::fast_flags(&mut values).into_result()?;

    Ok(values
        .into_iter()
        .map(|value| {
            let kind = if value.boolean {
                FastFlagValue::Bool(value.bool_value)
            } else {
                FastFlagValue::Int(value.int_value)
            };

            (value.name, kind)
        })
        .collect())
}

/// Sets a compiled fast flag, rejecting unknown names and value type mismatches.
///
/// # Errors
/// Returns an error for an unknown flag, a type mismatch, or a native failure.
pub fn set_fast_flag(name: &str, value: FastFlagValue) -> io::Result<()> {
    let (boolean, bool_value, int_value) = match value {
        FastFlagValue::Bool(value) => (true, value, 0),
        FastFlagValue::Int(value) => (false, false, value),
    };

    native::set_fast_flag(name, boolean, bool_value, int_value).into_result()
}

/// Source bytes and kind supplied to the native checker.
pub struct Source<'source> {
    /// Source bytes.
    pub bytes: &'source [u8],

    /// Native source kind.
    pub kind: SourceKind,
}

/// Resolution request supplied by the native checker.
pub struct ResolveRequest<'request> {
    /// Original source module being traced, independent of intermediate navigation.
    pub from: &'request str,

    /// Result of the preceding navigation step, if Luau resolved it.
    pub context: Option<&'request str>,

    /// Whether resolution failure is allowed.
    pub optional: bool,

    /// Expression source range as line, column, end line, and end column.
    pub location: [u32; 4],
}

/// Related diagnostic supplied by the native checker.
pub struct RelatedDiagnostic<'diagnostic> {
    /// Related source path.
    pub path: &'diagnostic str,

    /// Related source range.
    pub location: [u32; 4],

    /// Related message.
    pub message: &'diagnostic str,
}

/// Diagnostic supplied by the native checker.
pub struct Diagnostic<'diagnostic> {
    /// Source path.
    pub path: &'diagnostic str,

    /// Source range.
    pub location: [u32; 4],

    /// Native diagnostic severity.
    pub severity: DiagnosticSeverity,

    /// Diagnostic message.
    pub message: &'diagnostic str,

    /// Native lint rule name, empty for other diagnostics.
    pub rule: &'diagnostic str,

    /// Related diagnostic.
    pub related: Option<RelatedDiagnostic<'diagnostic>>,
}

/// Supplies host callbacks to a native checker.
pub trait Callbacks {
    /// Supplies source bytes and kind.
    ///
    /// # Errors
    /// Returns an error when the source cannot be loaded.
    fn source<'source>(&'source mut self, name: &str) -> io::Result<Source<'source>>;

    /// Supplies a configuration handle.
    ///
    /// # Errors
    /// Returns an error when configuration lookup fails.
    fn configuration(&mut self, name: &str) -> io::Result<&Configuration>;

    /// Resolves a module or instance request.
    ///
    /// # Errors
    /// Returns an error when resolution fails.
    fn resolve(&mut self, request: &ResolveRequest<'_>) -> io::Result<Option<String>>;

    /// Receives one diagnostic.
    ///
    /// # Errors
    /// Returns an error when the diagnostic cannot be recorded.
    fn diagnostic(&mut self, diagnostic: Diagnostic<'_>) -> io::Result<()>;
}

/// Native configuration handle.
pub struct Configuration {
    handle: cxx::UniquePtr<native::Configuration>,
}

impl Configuration {
    /// Creates a native configuration from merged Luau JSON.
    ///
    /// # Errors
    /// Returns an error when the JSON cannot be parsed.
    pub fn new(source: &[u8]) -> io::Result<Self> {
        let mut failure = native::Failure::default();
        let handle = native::configuration_create(source, &mut failure);
        failure.into_result()?;

        Ok(Self { handle })
    }
}

/// Borrowed host callbacks for a synchronous native operation.
pub struct Host<'callbacks> {
    callbacks: &'callbacks mut dyn Callbacks,
    error: Option<io::Error>,
}

impl Host<'_> {
    fn source<'source>(&'source mut self, name: &str) -> native::SourceData<'source> {
        match self.callbacks.source(name) {
            Ok(source) => native::SourceData {
                bytes: source.bytes,
                kind: source.kind,
                success: true,
            },

            Err(error) => {
                self.error = Some(error);

                native::SourceData {
                    bytes: &[],
                    kind: SourceKind::SourceUnknown,
                    success: false,
                }
            }
        }
    }

    fn configuration(&mut self, name: &str) -> *const native::Configuration {
        match self.callbacks.configuration(name) {
            Ok(configuration) => configuration
                .handle
                .as_ref()
                .map_or(ptr::null(), ptr::from_ref),

            Err(error) => {
                self.error = Some(error);

                ptr::null()
            }
        }
    }

    fn resolve(&mut self, request: native::Resolution<'_>) -> native::Resolved {
        let request = ResolveRequest {
            from: request.from,
            context: request.has_context.then_some(request.context),
            optional: request.optional,
            location: request.location,
        };

        match self.callbacks.resolve(&request) {
            Ok(path) => native::Resolved {
                present: path.is_some(),
                path: path.unwrap_or_default(),
                success: true,
            },

            Err(error) => {
                self.error = Some(error);

                native::Resolved {
                    path: String::new(),
                    present: false,
                    success: false,
                }
            }
        }
    }

    fn diagnostic(&mut self, value: native::DiagnosticData<'_>) -> bool {
        let diagnostic = Diagnostic {
            path: value.path,
            location: value.location,
            severity: value.severity,
            message: value.message,
            rule: value.rule,
            related: value.has_related.then_some(RelatedDiagnostic {
                path: value.related_path,
                location: value.related_location,
                message: value.related_message,
            }),
        };

        match self.callbacks.diagnostic(diagnostic) {
            Ok(()) => true,

            Err(error) => {
                self.error = Some(error);

                false
            }
        }
    }
}

/// Borrowed consumer for synchronous native enumeration.
pub struct Items<'callback> {
    callback: &'callback mut dyn FnMut(&str) -> io::Result<()>,
    error: Option<io::Error>,
}

impl Items<'_> {
    fn item(&mut self, value: &str) -> bool {
        match (self.callback)(value) {
            Ok(()) => true,

            Err(error) => {
                self.error = Some(error);

                false
            }
        }
    }
}

impl Default for native::Failure {
    fn default() -> Self {
        Self {
            kind: native::FailureKind::Success,
            message: cxx::UniquePtr::null(),
        }
    }
}

impl native::Failure {
    fn into_result(self) -> io::Result<()> {
        if self.kind == native::FailureKind::Success {
            return Ok(());
        }

        let message = self.message.as_ref().map_or_else(
            || Ok(String::new()),
            |message| {
                message
                    .to_str()
                    .map(str::to_owned)
                    .map_err(io::Error::other)
            },
        )?;

        if self.kind == native::FailureKind::Definition {
            Err(io::Error::new(
                io::ErrorKind::InvalidData,
                DefinitionFailure { message },
            ))
        } else if message.is_empty() {
            Err(io::Error::other("native checker operation failed"))
        } else {
            Err(io::Error::other(message))
        }
    }
}

/// Options used when creating a native checker.
#[derive(Default)]
pub struct CheckerOptions {
    /// Retains complete type graphs or annotation data.
    pub retain_full_type_graphs: u8,
}

/// One instance in a hierarchy; its slice index is its identity.
pub struct RobloxNode<'name> {
    /// Instance name.
    pub name: &'name str,

    /// Class name.
    pub class_name: &'name str,

    /// Parent node index, or `None` for a root.
    pub parent: Option<usize>,

    /// Source module whose `script` global refers to this node.
    pub module: Option<&'name str>,
}

/// Owns native Luau analysis state; the host controls its lifetime.
pub struct Checker {
    handle: cxx::UniquePtr<native::Checker>,
}

impl Checker {
    /// Creates a checker without binding host callbacks.
    ///
    /// # Errors
    /// Returns an error when native checker creation fails.
    pub fn new(options: &CheckerOptions) -> io::Result<Self> {
        let mut failure = native::Failure::default();
        let handle = native::checker_create(options.retain_full_type_graphs != 0, &mut failure);
        failure.into_result()?;

        Ok(Self { handle })
    }

    fn with_callbacks<T>(
        &mut self,
        callbacks: &mut dyn Callbacks,
        operation: impl FnOnce(Pin<&mut native::Checker>, &mut Host<'_>) -> io::Result<T>,
    ) -> io::Result<T> {
        let mut host = Host {
            callbacks,
            error: None,
        };

        let result = operation(self.handle.pin_mut(), &mut host);

        host.error.map_or(result, Err)
    }

    /// Loads a definition source.
    ///
    /// # Errors
    /// Returns an error when loading fails.
    pub fn load_definition(
        &mut self,
        callbacks: &mut dyn Callbacks,
        source: &[u8],
        package: &str,
    ) -> io::Result<()> {
        self.with_callbacks(callbacks, |checker, host| {
            native::checker_load_definition(checker, host, source, package).into_result()
        })
    }

    /// Discovers classes from loaded declarations and applies service/creatable flags.
    ///
    /// # Errors
    /// Returns an error for invalid class metadata or changes after registration.
    pub fn register_roblox_classes(&mut self, classes: &[RobloxClass<'_>]) -> io::Result<()> {
        native::checker_register_roblox_classes(self.handle.pin_mut(), classes).into_result()
    }

    /// Installs node-specific types and per-module `script` bindings.
    /// A root `DataModel` binds `game`; its `Workspace` child binds `workspace`.
    /// Register classes first. Recreate the checker when the hierarchy changes.
    ///
    /// # Errors
    /// Rejects invalid classes, parent indices, cycles, duplicate modules, or late registration.
    pub fn register_roblox_tree(&mut self, nodes: &[RobloxNode<'_>]) -> io::Result<()> {
        if nodes
            .iter()
            .any(|node| node.parent.is_some_and(|parent| parent >= nodes.len()))
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Roblox parent index is out of range",
            ));
        }

        let nodes = nodes
            .iter()
            .map(|value| native::RobloxNode {
                name: value.name,
                class_name: value.class_name,
                parent: value.parent.unwrap_or(usize::MAX),
                has_module: value.module.is_some(),
                module: value.module.unwrap_or(""),
            })
            .collect::<Vec<_>>();

        native::checker_register_roblox_tree(self.handle.pin_mut(), &nodes).into_result()
    }

    /// Freezes the native global type arena.
    ///
    /// # Errors
    /// Returns an error when freezing fails.
    pub fn freeze(&mut self) -> io::Result<()> {
        native::checker_freeze(self.handle.pin_mut()).into_result()
    }

    /// Marks a module and its dependents dirty.
    ///
    /// # Errors
    /// Returns an error when invalidation fails.
    pub fn mark_dirty(&mut self, path: &Path) -> io::Result<()> {
        native::checker_mark_dirty(self.handle.pin_mut(), module_name(path)?).into_result()
    }

    /// Clears ordinary source caches without changing definitions or globals.
    /// Create a new checker to rebuild the global environment.
    ///
    /// # Errors
    /// Returns an error when clearing fails.
    pub fn clear_sources(&mut self) -> io::Result<()> {
        native::checker_clear_sources(self.handle.pin_mut()).into_result()
    }

    /// Prepares the semantic module graph without diagnostics and returns timeout module names.
    ///
    /// # Errors
    /// Returns an error when semantic preparation fails.
    pub fn prepare(
        &mut self,
        callbacks: &mut dyn Callbacks,
        path: &Path,
    ) -> io::Result<Vec<String>> {
        let name = module_name(path)?;

        self.with_callbacks(callbacks, |checker, host| {
            let mut timeouts = Vec::new();
            native::checker_prepare(checker, host, name, &mut timeouts).into_result()?;

            Ok(timeouts)
        })
    }

    /// Emits cached syntax and type diagnostics for one prepared module.
    ///
    /// # Errors
    /// Returns an error when the module is not prepared or diagnostic emission fails.
    pub fn check(&mut self, callbacks: &mut dyn Callbacks, path: &Path) -> io::Result<()> {
        let name = module_name(path)?;

        self.with_callbacks(callbacks, |checker, host| {
            native::checker_check(checker, host, name).into_result()
        })
    }

    /// Runs native lint on one prepared module and emits only warnings.
    /// Requires retained full type graphs; sources with parse errors are skipped.
    ///
    /// # Errors
    /// Returns an error when preparation or retained types are unavailable, or emission fails.
    pub fn lint(&mut self, callbacks: &mut dyn Callbacks, path: &Path) -> io::Result<()> {
        let name = module_name(path)?;

        self.with_callbacks(callbacks, |checker, host| {
            native::checker_lint(checker, host, name).into_result()
        })
    }

    /// Parses a module without type checking it.
    ///
    /// # Errors
    /// Returns an error when native parsing fails.
    pub fn parse(&mut self, callbacks: &mut dyn Callbacks, path: &Path) -> io::Result<()> {
        let name = module_name(path)?;

        self.with_callbacks(callbacks, |checker, host| {
            native::checker_parse(checker, host, name).into_result()
        })
    }

    /// Emits syntax diagnostics for a parsed module.
    ///
    /// # Errors
    /// Returns an error when diagnostic emission fails.
    pub fn parse_diagnostics(
        &mut self,
        callbacks: &mut dyn Callbacks,
        path: &Path,
    ) -> io::Result<()> {
        let name = module_name(path)?;

        self.with_callbacks(callbacks, |checker, host| {
            native::checker_parse_diagnostics(checker, host, name).into_result()
        })
    }

    /// Attaches inferred type data to a checked module.
    ///
    /// # Errors
    /// Returns an error when the checked module is unavailable.
    pub fn attach_type_data(&mut self, path: &Path) -> io::Result<()> {
        native::checker_attach_type_data(self.handle.pin_mut(), module_name(path)?).into_result()
    }

    /// Enumerates names in the global scope.
    ///
    /// # Errors
    /// Returns an error when enumeration fails.
    pub fn globals(&self, callback: &mut impl FnMut(&str) -> io::Result<()>) -> io::Result<()> {
        Self::items(callback, |items| {
            native::checker_globals(&self.handle, items)
        })
    }

    /// Enumerates modules known to the checker.
    ///
    /// # Errors
    /// Returns an error when enumeration fails.
    pub fn modules(&self, callback: &mut impl FnMut(&str) -> io::Result<()>) -> io::Result<()> {
        Self::items(callback, |items| {
            native::checker_modules(&self.handle, items)
        })
    }

    fn items(
        callback: &mut impl FnMut(&str) -> io::Result<()>,
        operation: impl FnOnce(&mut Items<'_>) -> native::Failure,
    ) -> io::Result<()> {
        let mut items = Items {
            callback,
            error: None,
        };

        let result = operation(&mut items);

        match items.error {
            Some(error) => Err(error),
            None => result.into_result(),
        }
    }
}

fn module_name(path: &Path) -> io::Result<&str> {
    path.to_str()
        .ok_or_else(|| io::Error::other("module identity requires UTF-8"))
}

#[cfg(test)]
mod tests {
    use std::{io, path::Path};

    use super::{
        Callbacks, Checker, CheckerOptions, Configuration, DefinitionFailure, Diagnostic,
        DiagnosticSeverity, EditorCompletionKind, FastFlagValue, ResolveRequest, Source,
        fast_flags, native, set_fast_flag,
    };

    struct TestHost {
        config: Configuration,
        source: Vec<u8>,
        diagnostics: Vec<(String, String, native::DiagnosticSeverity)>,
        diagnostic_error: Option<io::Error>,
    }

    impl Callbacks for TestHost {
        fn source<'source>(&'source mut self, name: &str) -> io::Result<Source<'source>> {
            if name != "main" {
                return Err(io::Error::other(format!("unexpected source {name}")));
            }

            Ok(Source {
                bytes: &self.source,
                kind: native::SourceKind::SourceModule,
            })
        }

        fn configuration(&mut self, _: &str) -> io::Result<&Configuration> {
            Ok(&self.config)
        }

        fn resolve(&mut self, _: &ResolveRequest<'_>) -> io::Result<Option<String>> {
            Ok(None)
        }

        fn diagnostic(&mut self, diagnostic: Diagnostic<'_>) -> io::Result<()> {
            self.diagnostics.push((
                diagnostic.path.into(),
                diagnostic.message.into(),
                diagnostic.severity,
            ));

            if let Some(error) = self.diagnostic_error.take() {
                return Err(error);
            }

            Ok(())
        }
    }

    fn assert_completion_uses_loaded_definitions() {
        let mut checker = Checker::new(&CheckerOptions {
            retain_full_type_graphs: 1,
        })
        .unwrap();

        let mut host = TestHost {
            config: Configuration::new(br#"{"languageMode":"strict"}"#).unwrap(),
            source: b"return Widget.value\n".to_vec(),
            diagnostics: Vec::new(),
            diagnostic_error: None,
        };

        checker
            .load_definition(&mut host, b"declare Widget: { value: number }\n", "@test")
            .unwrap_or_else(|error| panic!("{error}: {:?}", host.diagnostics));

        checker.freeze().unwrap();
        checker.prepare(&mut host, Path::new("main")).unwrap();
        let items = checker.completion(&mut host, "main", 0, 14).unwrap();
        let property = items.iter().find(|item| item.name == "value").unwrap();
        assert_eq!(property.detail, "number");

        assert_eq!(property.kind, EditorCompletionKind::CompletionProperty);
    }

    #[test]
    fn definition_rejection_is_distinct_from_operation_and_callback_failures() {
        let mut checker = Checker::new(&CheckerOptions::default()).unwrap();

        let mut host = TestHost {
            config: Configuration::new(br#"{"languageMode":"strict"}"#).unwrap(),
            source: Vec::new(),
            diagnostics: Vec::new(),
            diagnostic_error: None,
        };

        let malformed = b"declare function incomplete(";

        let rejected = checker
            .load_definition(&mut host, malformed, "rejected")
            .unwrap_err();

        assert_eq!(rejected.kind(), io::ErrorKind::InvalidData);

        assert!(
            rejected
                .get_ref()
                .is_some_and(<dyn std::error::Error + Send + Sync>::is::<DefinitionFailure>)
        );

        let misleading_message = rejected.to_string();

        host.diagnostic_error = Some(io::Error::new(
            io::ErrorKind::InvalidData,
            misleading_message.clone(),
        ));

        let callback = checker
            .load_definition(&mut host, malformed, "callback")
            .unwrap_err();

        assert_eq!(callback.kind(), io::ErrorKind::InvalidData);
        assert_eq!(callback.to_string(), misleading_message);

        assert!(
            !callback
                .get_ref()
                .is_some_and(<dyn std::error::Error + Send + Sync>::is::<DefinitionFailure>)
        );

        let retried = checker
            .load_definition(&mut host, malformed, "retried")
            .unwrap_err();

        assert!(
            retried
                .get_ref()
                .is_some_and(<dyn std::error::Error + Send + Sync>::is::<DefinitionFailure>)
        );

        checker.freeze().unwrap();

        let state = checker
            .load_definition(&mut host, malformed, "frozen")
            .unwrap_err();

        assert_eq!(state.kind(), io::ErrorKind::Other);

        assert!(
            !state
                .get_ref()
                .is_some_and(<dyn std::error::Error + Send + Sync>::is::<DefinitionFailure>)
        );
    }

    fn assert_prepared_check_and_typed_lint_are_independent() {
        let path = Path::new("main");
        let source = b"local wrong: number = \"bad\"\nlocal value: string = \"text\"\nprint(value:match(\"[]\"))\nreturn wrong\n";
        let warning_config = br#"{"languageMode":"strict","lint":{"*":false,"FormatString":true}}"#;

        let mut checker = Checker::new(&CheckerOptions {
            retain_full_type_graphs: 1,
        })
        .unwrap();

        checker.freeze().unwrap();

        let mut host = TestHost {
            config: Configuration::new(warning_config).unwrap(),
            source: source.to_vec(),
            diagnostics: Vec::new(),
            diagnostic_error: None,
        };

        assert!(checker.check(&mut host, path).is_err());
        assert!(checker.lint(&mut host, path).is_err());
        assert_eq!(checker.prepare(&mut host, path).unwrap().len(), 0);
        assert_eq!(checker.prepare(&mut host, path).unwrap().len(), 0);
        assert_eq!(host.diagnostics.len(), 0);

        checker.check(&mut host, path).unwrap();
        assert_eq!(host.diagnostics.len(), 1, "{:?}", host.diagnostics);
        let type_diagnostics = std::mem::take(&mut host.diagnostics);
        assert_eq!(type_diagnostics[0].0, "main");

        assert_eq!(type_diagnostics[0].2, DiagnosticSeverity::DiagnosticError);

        assert!(!type_diagnostics[0].1.starts_with("FormatString:"));

        checker.lint(&mut host, path).unwrap();
        assert_eq!(host.diagnostics.len(), 1, "{:?}", host.diagnostics);
        assert_eq!(host.diagnostics[0].0, "main");

        assert!(
            host.diagnostics[0]
                .1
                .starts_with("FormatString: Invalid match pattern:")
        );

        assert_eq!(host.diagnostics[0].2, DiagnosticSeverity::DiagnosticWarning);

        host.diagnostics.clear();

        checker.check(&mut host, path).unwrap();
        assert_eq!(host.diagnostics, type_diagnostics);
        host.diagnostics.clear();

        assert_pass_callback_failures(&mut checker, &mut host, path);

        host.config = Configuration::new(
            br#"{"languageMode":"strict","lint":{"*":false,"FormatString":true},"lintErrors":true}"#,
        )
        .unwrap();

        checker.prepare(&mut host, path).unwrap();
        assert_eq!(host.diagnostics.len(), 0);
        checker.lint(&mut host, path).unwrap();
        assert_eq!(host.diagnostics.len(), 1);

        assert_eq!(host.diagnostics[0].2, DiagnosticSeverity::DiagnosticError);

        assert!(host.diagnostics[0].1.starts_with("FormatString:"));
        host.diagnostics.clear();

        host.config =
            Configuration::new(br#"{"languageMode":"strict","lint":{"*":false}}"#).unwrap();

        checker.lint(&mut host, path).unwrap();
        assert_eq!(host.diagnostics.len(), 0);

        host.config = Configuration::new(warning_config).unwrap();
        host.source = [b"--!nolint FormatString\n".as_slice(), source.as_slice()].concat();
        checker.mark_dirty(path).unwrap();
        assert!(checker.check(&mut host, path).is_err());
        assert!(checker.lint(&mut host, path).is_err());
        checker.prepare(&mut host, path).unwrap();
        assert_eq!(host.diagnostics.len(), 0);
        checker.lint(&mut host, path).unwrap();
        assert_eq!(host.diagnostics.len(), 0);
        checker.check(&mut host, path).unwrap();
        assert_eq!(host.diagnostics, type_diagnostics);
        host.diagnostics.clear();

        host.source = [source.as_slice(), b"local broken =\n".as_slice()].concat();
        checker.mark_dirty(path).unwrap();
        checker.prepare(&mut host, path).unwrap();
        assert_eq!(host.diagnostics.len(), 0);
        checker.lint(&mut host, path).unwrap();
        assert_eq!(host.diagnostics.len(), 0);
        checker.check(&mut host, path).unwrap();

        assert!(
            host.diagnostics.len() > type_diagnostics.len(),
            "{:?}",
            host.diagnostics
        );

        assert!(host.diagnostics.iter().all(|(_, message, severity)| {
            *severity == DiagnosticSeverity::DiagnosticError
                && !message.starts_with("FormatString:")
        }));

        host.diagnostics.clear();

        host.source = source.to_vec();
        let mut without_graphs = Checker::new(&CheckerOptions::default()).unwrap();
        without_graphs.freeze().unwrap();
        without_graphs.prepare(&mut host, path).unwrap();
        assert_eq!(host.diagnostics.len(), 0);

        assert!(
            without_graphs
                .lint(&mut host, path)
                .unwrap_err()
                .to_string()
                .contains("retained full type graphs")
        );
    }

    fn assert_pass_callback_failures(checker: &mut Checker, host: &mut TestHost, path: &Path) {
        host.diagnostic_error = Some(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "check callback",
        ));

        assert_eq!(
            checker.check(host, path).unwrap_err().kind(),
            io::ErrorKind::PermissionDenied
        );

        host.diagnostics.clear();
        host.diagnostic_error = Some(io::Error::new(io::ErrorKind::BrokenPipe, "lint callback"));

        assert_eq!(
            checker.lint(host, path).unwrap_err().kind(),
            io::ErrorKind::BrokenPipe
        );

        host.diagnostics.clear();
    }

    #[test]
    fn native_flags_and_unnamed_complexity_error_owner() {
        let original = fast_flags().unwrap();
        let name = "FIntLuauTarjanChildLimit";
        assert!(matches!(original.get(name), Some(FastFlagValue::Int(_))));

        assert!(set_fast_flag(name, FastFlagValue::Bool(true)).is_err());
        assert!(set_fast_flag("FIntUnknownTestFlag", FastFlagValue::Int(1)).is_err());
        assert_eq!(fast_flags().unwrap().get(name), original.get(name));

        set_fast_flag(name, FastFlagValue::Int(12_345)).unwrap();

        assert_eq!(
            fast_flags().unwrap().get(name),
            Some(&FastFlagValue::Int(12_345))
        );

        set_fast_flag(name, original[name]).unwrap();
        let mut checker = Checker::new(&CheckerOptions::default()).unwrap();
        checker.freeze().unwrap();

        set_fast_flag(name, FastFlagValue::Int(1)).unwrap();

        let result = (|| -> io::Result<_> {
            let mut host = TestHost {
                config: Configuration::new(br#"{"languageMode":"strict"}"#)?,
                source: b"function f(t)\n    t.x.y.z = 441\nend\n".to_vec(),
                diagnostics: Vec::new(),
                diagnostic_error: None,
            };

            checker.prepare(&mut host, Path::new("main"))?;
            checker.check(&mut host, Path::new("main"))?;

            Ok(host.diagnostics)
        })();

        set_fast_flag(name, original[name]).unwrap();

        let diagnostics = result.unwrap();

        assert!(
            diagnostics.iter().any(|(path, message, _)| {
                path == "main" && message.to_ascii_lowercase().contains("complex")
            }),
            "{diagnostics:?}"
        );

        assert_prepared_check_and_typed_lint_are_independent();
        assert_completion_uses_loaded_definitions();
    }
}
