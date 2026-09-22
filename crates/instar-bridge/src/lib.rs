//! Native Luau analysis bridge.

#![expect(unsafe_code, reason = "the bridge owns the C ABI marshalling boundary")]

use std::{ffi::c_void, io, path::Path, ptr, slice, str};

/// Raw C ABI declarations generated from the native bridge headers.
pub mod native {
    include!(concat!(env!("OUT_DIR"), "/bindings.rs"));
}

/// Source bytes and kind supplied to the native checker.
pub struct Source<'source> {
    /// Source bytes.
    pub bytes: &'source [u8],

    /// Native source kind.
    pub kind: native::SourceKind,
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
    pub severity: native::DiagnosticSeverity,

    /// Diagnostic message.
    pub message: &'diagnostic str,

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
    handle: *mut c_void,
}

impl Configuration {
    /// Creates a native configuration from merged Luau JSON.
    ///
    /// # Errors
    /// Returns an error when the JSON cannot be parsed.
    pub fn new(source: &[u8]) -> io::Result<Self> {
        let mut error = native::String::default();

        let handle = unsafe { native::configuration_create(text_bytes(source), &raw mut error) };

        if handle.is_null() {
            Err(io::Error::other(take_string(error)?))
        } else {
            Ok(Self { handle })
        }
    }

    fn pointer(&self) -> *const c_void {
        self.handle.cast_const()
    }
}

impl Drop for Configuration {
    fn drop(&mut self) {
        unsafe { native::configuration_destroy(self.handle) };
    }
}

struct CallbackContext<'callbacks> {
    callbacks: &'callbacks mut dyn Callbacks,
    source: Option<Source<'callbacks>>,
    resolved: Option<String>,
    error: Option<io::Error>,
}

fn callback_context<'callbacks>(
    context: *mut c_void,
) -> io::Result<&'callbacks mut CallbackContext<'callbacks>> {
    unsafe { context.cast::<CallbackContext<'callbacks>>().as_mut() }
        .ok_or_else(|| io::Error::other("null native callback context"))
}

fn set_callback_error(context: &mut CallbackContext<'_>, error: io::Error) {
    context.error = Some(error);
}

fn callback_status<T>(
    context: *mut c_void,
    result: io::Result<()>,
    set_error: impl FnOnce(&mut T, io::Error),
) -> u8 {
    match result {
        Ok(()) => 1,

        Err(error) => {
            if let Some(context) = unsafe { context.cast::<T>().as_mut() } {
                set_error(context, error);
            }

            0
        }
    }
}

unsafe extern "C" fn source_callback(
    context: *mut c_void,
    name: native::Text,
    result: *mut native::SourceResult,
) -> u8 {
    let result = (|| {
        let name = unsafe { borrow_text(name)? };

        let context = callback_context(context)?;
        let source = context.callbacks.source(name)?;
        context.source = Some(source);

        let source = context
            .source
            .as_ref()
            .expect("source callback stored source");

        unsafe {
            *result = native::SourceResult {
                source: text_bytes(source.bytes),
                kind: source.kind,
            };
        }

        Ok(())
    })();

    callback_status(context, result, set_callback_error)
}

unsafe extern "C" fn configuration_callback(
    context: *mut c_void,
    name: native::Text,
    result: *mut *const c_void,
) -> u8 {
    let result_value = (|| {
        let name = unsafe { borrow_text(name)? };

        let configuration = callback_context(context)?.callbacks.configuration(name)?;

        unsafe { *result = configuration.pointer() };

        Ok(())
    })();

    callback_status(context, result_value, set_callback_error)
}

unsafe extern "C" fn resolve_callback(
    context: *mut c_void,
    request: *const native::ResolveRequest,
    result: *mut native::ResolveResult,
) -> u8 {
    let result_value = (|| {
        let request =
            unsafe { request.as_ref() }.ok_or_else(|| io::Error::other("null resolve request"))?;

        let from = unsafe { borrow_text(request.from)? };

        let request_value = ResolveRequest {
            from,
            context: if request.has_context != 0 {
                Some(unsafe { borrow_text(request.context)? })
            } else {
                None
            },
            optional: request.optional != 0,
            location: range(request.expression),
        };

        let context = callback_context(context)?;
        context.resolved = context.callbacks.resolve(&request_value)?;
        // Native code copies this path after the callback returns.
        let path = &context.resolved;

        unsafe {
            *result = native::ResolveResult {
                path: path.as_deref().map_or_else(|| text_bytes(&[]), text),
                present: u8::from(path.is_some()),
            };
        }

        Ok(())
    })();

    callback_status(context, result_value, set_callback_error)
}

unsafe extern "C" fn diagnostic_callback(
    context: *mut c_void,
    value: *const native::Diagnostic,
) -> u8 {
    let result = (|| {
        let value = unsafe { value.as_ref() }.ok_or_else(|| io::Error::other("null diagnostic"))?;

        let path = unsafe { borrow_text(value.path)? };

        let message = unsafe { borrow_text(value.message)? };

        let related_path = unsafe { borrow_text(value.related.path)? };

        let related_message = unsafe { borrow_text(value.related.message)? };

        let diagnostic = Diagnostic {
            path,
            location: range(value.location),
            severity: value.severity,
            message,
            related: (value.has_related != 0).then_some(RelatedDiagnostic {
                path: related_path,
                location: range(value.related.location),
                message: related_message,
            }),
        };

        callback_context(context)?.callbacks.diagnostic(diagnostic)
    })();

    callback_status(context, result, set_callback_error)
}

const CALLBACKS: native::BridgeCallbacks = native::BridgeCallbacks {
    source: Some(source_callback),
    configuration: Some(configuration_callback),
    resolve: Some(resolve_callback),
    diagnostic: Some(diagnostic_callback),
};

/// Options used when creating a native checker.
#[derive(Default)]
pub struct CheckerOptions {
    /// Retains complete type graphs or annotation data.
    pub retain_full_type_graphs: u8,

    /// Configures the checker for autocomplete.
    pub for_autocomplete: u8,

    /// Runs lint checks.
    pub run_lint_checks: u8,
}

/// Options used when emitting checker results.
#[derive(Default)]
pub struct ResultOptions {
    /// Accumulates nested module results.
    pub accumulate_nested: bool,

    /// Emits results for autocomplete.
    pub for_autocomplete: bool,
}

/// Roblox class metadata supplied to the checker.
pub struct RobloxClass<'name> {
    /// Class name.
    pub name: &'name str,

    /// Whether the class is a service.
    pub service: bool,

    /// Whether instances can be created.
    pub creatable: bool,
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

impl Checker {
    fn items(
        operation: impl FnOnce(native::ItemCallback, *mut c_void, *mut native::String) -> i32,
        callback: &mut impl FnMut(&str) -> io::Result<()>,
    ) -> io::Result<()> {
        let mut context = ItemContext {
            callback,
            error: None,
        };

        let mut error = native::String::default();

        let status = operation(
            Some(item_callback),
            ptr::from_mut(&mut context).cast(),
            &raw mut error,
        );

        let message = take_string(error)?;

        if let Some(error) = context.error {
            return Err(error);
        }

        if status == native::Status::StatusSuccess as i32 {
            Ok(())
        } else if message.is_empty() {
            Err(io::Error::other("native checker enumeration failed"))
        } else {
            Err(io::Error::other(message))
        }
    }
}

/// Owns native Luau analysis state; the host controls its lifetime.
pub struct Checker {
    handle: *mut c_void,
}

impl Checker {
    /// Creates a checker without binding host callbacks.
    ///
    /// # Errors
    /// Returns an error when native checker creation fails.
    pub fn new(options: &CheckerOptions) -> io::Result<Self> {
        let native_options = native::FrontendOptions {
            retain_full_type_graphs: options.retain_full_type_graphs,
            for_autocomplete: options.for_autocomplete,
            run_lint_checks: options.run_lint_checks,
        };

        let mut error = native::String::default();

        let handle = unsafe {
            native::checker_create(
                &CALLBACKS,
                ptr::null_mut(),
                &raw const native_options,
                &raw mut error,
            )
        };

        if handle.is_null() {
            Err(io::Error::other(take_string(error)?))
        } else {
            Ok(Self { handle })
        }
    }

    fn call(operation: impl FnOnce(*mut native::String) -> i32) -> io::Result<()> {
        let mut error = native::String::default();
        let status = operation(&raw mut error);
        let message = take_string(error)?;

        if status == native::Status::StatusSuccess as i32 {
            Ok(())
        } else if message.is_empty() {
            Err(io::Error::other("native checker operation failed"))
        } else {
            Err(io::Error::other(message))
        }
    }

    fn with_callbacks<T>(
        &mut self,
        callbacks: &mut dyn Callbacks,
        operation: impl FnOnce(&Self) -> io::Result<T>,
    ) -> io::Result<T> {
        let mut context = CallbackContext {
            callbacks,
            source: None,
            resolved: None,
            error: None,
        };

        Self::call(|error| unsafe {
            native::checker_set_context(self.handle, ptr::from_mut(&mut context).cast(), error)
        })?;

        let result = operation(self);

        let reset = Self::call(|error| unsafe {
            native::checker_set_context(self.handle, ptr::null_mut(), error)
        });

        if let Some(error) = context.error.take() {
            return Err(error);
        }

        reset?;

        result
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
        self.with_callbacks(callbacks, |session| {
            let options = native::DefinitionOptions {
                capture_comments: 0,
                type_check_for_autocomplete: 0,
            };

            Self::call(|error| unsafe {
                native::checker_load_definition(
                    session.handle,
                    text_bytes(source),
                    text(package),
                    &raw const options,
                    error,
                )
            })
        })
    }

    /// Registers Roblox class magic after loading definitions.
    ///
    /// # Errors
    /// Returns an error for invalid class metadata or changes after registration.
    pub fn register_roblox_classes(&mut self, classes: &[RobloxClass<'_>]) -> io::Result<()> {
        let classes = classes
            .iter()
            .map(|value| native::RobloxClass {
                name: text(value.name),
                service: u8::from(value.service),
                creatable: u8::from(value.creatable),
            })
            .collect::<Vec<_>>();

        Self::call(|error| unsafe {
            native::checker_register_roblox_classes(
                self.handle,
                classes.as_ptr(),
                classes.len(),
                error,
            )
        })
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
                name: text(value.name),
                class_name: text(value.class_name),
                parent: value.parent.unwrap_or(usize::MAX),
                has_module: u8::from(value.module.is_some()),
                module: text(value.module.unwrap_or("")),
            })
            .collect::<Vec<_>>();

        Self::call(|error| unsafe {
            native::checker_register_roblox_tree(self.handle, nodes.as_ptr(), nodes.len(), error)
        })
    }

    /// Freezes the native global type arena.
    ///
    /// # Errors
    /// Returns an error when freezing fails.
    pub fn freeze(&mut self) -> io::Result<()> {
        Self::call(|error| unsafe { native::checker_freeze(self.handle, error) })
    }

    /// Marks a module and its dependents dirty.
    ///
    /// # Errors
    /// Returns an error when invalidation fails.
    pub fn mark_dirty(&mut self, path: &Path) -> io::Result<()> {
        let name = path
            .to_str()
            .ok_or_else(|| io::Error::other("module identity requires UTF-8"))?;

        Self::call(|error| unsafe { native::checker_mark_dirty(self.handle, text(name), error) })
    }

    /// Clears ordinary source caches without changing definitions or globals.
    /// Create a new checker to rebuild the global environment.
    ///
    /// # Errors
    /// Returns an error when clearing fails.
    pub fn clear_sources(&mut self) -> io::Result<()> {
        Self::call(|error| unsafe { native::checker_clear_sources(self.handle, error) })
    }

    /// Checks a module and returns the names of modules that timed out.
    ///
    /// # Errors
    /// Returns an error when checking fails.
    pub fn check(&mut self, callbacks: &mut dyn Callbacks, path: &Path) -> io::Result<Vec<String>> {
        let name = path
            .to_str()
            .ok_or_else(|| io::Error::other("module identity requires UTF-8"))?;

        self.with_callbacks(callbacks, |checker| {
            let mut timeouts = Vec::new();

            Self::items(
                |item, context, error| unsafe {
                    native::checker_check(checker.handle, text(name), item, context, error)
                },
                &mut |name| {
                    timeouts.push(name.to_owned());

                    Ok(())
                },
            )?;

            Ok(timeouts)
        })
    }

    /// Emits cached diagnostics and returns their timeout module names.
    ///
    /// # Errors
    /// Returns an error when result emission fails.
    pub fn result(
        &mut self,
        callbacks: &mut dyn Callbacks,
        path: &Path,
    ) -> io::Result<Vec<String>> {
        self.result_with_options(callbacks, path, &ResultOptions::default())
    }

    /// Emits diagnostics with explicit options and returns their timeout module names.
    ///
    /// # Errors
    /// Returns an error when result emission fails.
    pub fn result_with_options(
        &mut self,
        callbacks: &mut dyn Callbacks,
        path: &Path,
        options: &ResultOptions,
    ) -> io::Result<Vec<String>> {
        let name = path
            .to_str()
            .ok_or_else(|| io::Error::other("module identity requires UTF-8"))?;

        self.with_callbacks(callbacks, |checker| {
            let mut timeouts = Vec::new();

            Self::items(
                |item, context, error| unsafe {
                    native::checker_result(
                        checker.handle,
                        text(name),
                        u8::from(options.accumulate_nested),
                        u8::from(options.for_autocomplete),
                        item,
                        context,
                        error,
                    )
                },
                &mut |name| {
                    timeouts.push(name.to_owned());

                    Ok(())
                },
            )?;

            Ok(timeouts)
        })
    }

    /// Returns hover information at a source position.
    ///
    /// # Errors
    /// Returns an error when the native operation fails.
    pub fn hover(
        &mut self,
        callbacks: &mut dyn Callbacks,
        path: &str,
        line: u32,
        column: u32,
    ) -> io::Result<Option<Hover>> {
        self.with_callbacks(callbacks, |checker| {
            let mut result = OneResult::default();

            Self::editor_call(
                |context, error| unsafe {
                    native::editor_hover(
                        checker.handle,
                        text(path),
                        line,
                        column,
                        Some(hover_callback),
                        context,
                        error,
                    )
                },
                &mut result,
            )?;

            Ok(result.value)
        })
    }

    /// Returns completion items at a source position.
    ///
    /// # Errors
    /// Returns an error when the native operation fails.
    pub fn completion(
        &mut self,
        callbacks: &mut dyn Callbacks,
        path: &str,
        line: u32,
        column: u32,
    ) -> io::Result<Vec<Completion>> {
        self.with_callbacks(callbacks, |checker| {
            let mut result = ManyResults::default();

            Self::editor_call(
                |context, error| unsafe {
                    native::editor_completion(
                        checker.handle,
                        text(path),
                        line,
                        column,
                        Some(completion_callback),
                        context,
                        error,
                    )
                },
                &mut result,
            )?;

            Ok(result.values)
        })
    }

    /// Returns signature help at a source position.
    ///
    /// # Errors
    /// Returns an error when the native operation fails.
    pub fn signature_help(
        &mut self,
        callbacks: &mut dyn Callbacks,
        path: &str,
        line: u32,
        column: u32,
    ) -> io::Result<Option<SignatureHelp>> {
        self.with_callbacks(callbacks, |checker| {
            let mut result = OneResult::default();

            Self::editor_call(
                |context, error| unsafe {
                    native::editor_signature_help(
                        checker.handle,
                        text(path),
                        line,
                        column,
                        Some(signature_callback),
                        context,
                        error,
                    )
                },
                &mut result,
            )?;

            Ok(result.value)
        })
    }

    /// Returns inferred type hints for a module.
    ///
    /// # Errors
    /// Returns an error when the native operation fails.
    pub fn type_hints(
        &mut self,
        callbacks: &mut dyn Callbacks,
        path: &str,
    ) -> io::Result<Vec<TypeHint>> {
        self.with_callbacks(callbacks, |checker| {
            let mut result = ManyResults::default();

            Self::editor_call(
                |context, error| unsafe {
                    native::editor_type_hints(
                        checker.handle,
                        text(path),
                        Some(hint_callback),
                        context,
                        error,
                    )
                },
                &mut result,
            )?;

            Ok(result.values)
        })
    }

    /// Returns semantic tokens for a module.
    ///
    /// # Errors
    /// Returns an error when the native operation fails.
    pub fn semantic_tokens(
        &mut self,
        callbacks: &mut dyn Callbacks,
        path: &str,
    ) -> io::Result<Vec<SemanticToken>> {
        self.with_callbacks(callbacks, |checker| {
            let mut result = ManyResults::default();

            Self::editor_call(
                |context, error| unsafe {
                    native::editor_semantic_tokens(
                        checker.handle,
                        text(path),
                        Some(token_callback),
                        context,
                        error,
                    )
                },
                &mut result,
            )?;

            Ok(result.values)
        })
    }

    /// Returns definition targets at a source position.
    ///
    /// # Errors
    /// Returns an error when the native operation fails.
    pub fn definition(
        &mut self,
        callbacks: &mut dyn Callbacks,
        path: &str,
        line: u32,
        column: u32,
    ) -> io::Result<Vec<Navigation>> {
        self.navigation(callbacks, path, line, column, native::editor_definition)
    }

    /// Returns declaration targets at a source position.
    ///
    /// # Errors
    /// Returns an error when the native operation fails.
    pub fn declaration(
        &mut self,
        callbacks: &mut dyn Callbacks,
        path: &str,
        line: u32,
        column: u32,
    ) -> io::Result<Vec<Navigation>> {
        self.navigation(callbacks, path, line, column, native::editor_declaration)
    }

    /// Returns type-definition targets at a source position.
    ///
    /// # Errors
    /// Returns an error when the native operation fails.
    pub fn type_definition(
        &mut self,
        callbacks: &mut dyn Callbacks,
        path: &str,
        line: u32,
        column: u32,
    ) -> io::Result<Vec<Navigation>> {
        self.navigation(
            callbacks,
            path,
            line,
            column,
            native::editor_type_definition,
        )
    }

    fn navigation(
        &mut self,
        callbacks: &mut dyn Callbacks,
        path: &str,
        line: u32,
        column: u32,
        operation: unsafe extern "C" fn(
            *mut c_void,
            native::Text,
            u32,
            u32,
            native::NavigationCallback,
            *mut c_void,
            *mut native::String,
        ) -> i32,
    ) -> io::Result<Vec<Navigation>> {
        self.with_callbacks(callbacks, |checker| {
            let mut result = ManyResults::default();

            Self::editor_call(
                |context, error| unsafe {
                    operation(
                        checker.handle,
                        text(path),
                        line,
                        column,
                        Some(navigation_callback),
                        context,
                        error,
                    )
                },
                &mut result,
            )?;

            Ok(result.values)
        })
    }

    /// Returns reference occurrences at a source position.
    ///
    /// # Errors
    /// Returns an error when the native operation fails.
    pub fn references(
        &mut self,
        callbacks: &mut dyn Callbacks,
        path: &str,
        line: u32,
        column: u32,
    ) -> io::Result<Vec<Reference>> {
        self.with_callbacks(callbacks, |checker| {
            let mut result = ManyResults::default();

            Self::editor_call(
                |context, error| unsafe {
                    native::editor_references(
                        checker.handle,
                        text(path),
                        line,
                        column,
                        Some(reference_callback),
                        context,
                        error,
                    )
                },
                &mut result,
            )?;

            Ok(result.values)
        })
    }

    /// Returns the prepared symbol at a source position.
    ///
    /// # Errors
    /// Returns an error when the native operation fails.
    pub fn prepare(
        &mut self,
        callbacks: &mut dyn Callbacks,
        path: &str,
        line: u32,
        column: u32,
    ) -> io::Result<Option<Symbol>> {
        self.with_callbacks(callbacks, |checker| {
            let mut result = OneResult::default();

            Self::editor_call(
                |context, error| unsafe {
                    native::editor_prepare(
                        checker.handle,
                        text(path),
                        line,
                        column,
                        Some(prepared_symbol_callback),
                        context,
                        error,
                    )
                },
                &mut result,
            )?;

            Ok(result.value)
        })
    }

    /// Returns local symbol occurrences at a source position.
    ///
    /// # Errors
    /// Returns an error when the native operation fails.
    pub fn local_references(
        &mut self,
        callbacks: &mut dyn Callbacks,
        path: &str,
        line: u32,
        column: u32,
    ) -> io::Result<Vec<Symbol>> {
        self.with_callbacks(callbacks, |checker| {
            let mut result = ManyResults::default();

            Self::editor_call(
                |context, error| unsafe {
                    native::editor_local_references(
                        checker.handle,
                        text(path),
                        line,
                        column,
                        Some(symbol_callback),
                        context,
                        error,
                    )
                },
                &mut result,
            )?;

            Ok(result.values)
        })
    }

    /// Parses a module without type checking it.
    ///
    /// # Errors
    /// Returns an error when native parsing fails.
    pub fn parse(&mut self, callbacks: &mut dyn Callbacks, path: &Path) -> io::Result<()> {
        self.with_callbacks(callbacks, |checker| {
            Self::call_name(path, |name, error| unsafe {
                native::checker_parse(checker.handle, name, error)
            })
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
        self.with_callbacks(callbacks, |checker| {
            Self::call_name(path, |name, error| unsafe {
                native::checker_parse_diagnostics(checker.handle, name, error)
            })
        })
    }

    /// Attaches inferred type data to a checked module.
    ///
    /// # Errors
    /// Returns an error when the checked module is unavailable.
    pub fn attach_type_data(&mut self, path: &Path) -> io::Result<()> {
        Self::call_name(path, |name, error| unsafe {
            native::checker_attach_type_data(self.handle, name, error)
        })
    }

    /// Enumerates names in the global scope.
    ///
    /// # Errors
    /// Returns an error when enumeration fails.
    pub fn globals(&self, callback: &mut impl FnMut(&str) -> io::Result<()>) -> io::Result<()> {
        Self::items(
            |item, context, error| unsafe {
                native::checker_globals(self.handle, item, context, error)
            },
            callback,
        )
    }

    /// Enumerates modules known to the checker.
    ///
    /// # Errors
    /// Returns an error when enumeration fails.
    pub fn modules(&self, callback: &mut impl FnMut(&str) -> io::Result<()>) -> io::Result<()> {
        Self::items(
            |item, context, error| unsafe {
                native::checker_modules(self.handle, item, context, error)
            },
            callback,
        )
    }

    fn call_name(
        path: &Path,
        operation: impl FnOnce(native::Text, *mut native::String) -> i32,
    ) -> io::Result<()> {
        let name = path
            .to_str()
            .ok_or_else(|| io::Error::other("module identity requires UTF-8"))?;

        Self::call(|error| operation(text(name), error))
    }
}

impl Drop for Checker {
    fn drop(&mut self) {
        unsafe { native::checker_destroy(self.handle) };
    }
}

struct ItemContext<'callback> {
    callback: &'callback mut dyn FnMut(&str) -> io::Result<()>,
    error: Option<io::Error>,
}

unsafe extern "C" fn item_callback(context: *mut c_void, value: native::Text) -> u8 {
    let result = (|| {
        let value = unsafe { borrow_text(value)? };

        let context = unsafe { context.cast::<ItemContext<'_>>().as_mut() }
            .ok_or_else(|| io::Error::other("null item callback context"))?;

        (context.callback)(value)
    })();

    match result {
        Ok(()) => 1,

        Err(error) => {
            if let Some(context) = unsafe { context.cast::<ItemContext<'_>>().as_mut() } {
                context.error = Some(error);
            }

            0
        }
    }
}

/// Hover information returned for a source position.
pub struct Hover {
    /// Symbol name.
    pub name: String,

    /// Displayed type.
    pub type_: String,

    /// Documentation symbol identifier.
    pub documentation_symbol: String,

    /// Hover range when present.
    pub range: Option<[u32; 4]>,
}

/// Completion item returned for a source position.
pub struct Completion {
    /// Completion label.
    pub name: String,

    /// Completion detail.
    pub detail: String,

    /// Documentation symbol identifier.
    pub documentation_symbol: String,

    /// Completion insertion text.
    pub insert: String,

    /// Completion range when present.
    pub range: Option<[u32; 4]>,

    /// Completion source mode.
    pub mode: native::EditorCompletionMode,

    /// Completion item kind.
    pub kind: native::EditorCompletionKind,

    /// Whether the item is deprecated.
    pub deprecated: bool,
}

/// Signature help returned for a call site.
pub struct SignatureHelp {
    /// Signature label.
    pub label: String,

    /// Signature parameter labels.
    pub parameters: Vec<String>,

    /// Active parameter index when present.
    pub active_parameter: Option<u32>,
}

/// Inferred type for a source range.
pub struct TypeHint {
    /// Type-hint range.
    pub range: [u32; 4],

    /// Inferred type text.
    pub type_: String,
}

/// Semantic token returned for a source range.
pub struct SemanticToken {
    /// Token range.
    pub range: [u32; 4],

    /// Token kind.
    pub kind: native::EditorSemanticTokenKind,

    /// Token modifier bits.
    pub modifiers: u32,

    /// Whether the token is a declaration.
    pub declaration: bool,
}

/// Navigation target and selection range.
pub struct Navigation {
    /// Target source path.
    pub path: String,

    /// Full target range.
    pub range: [u32; 4],

    /// Name-selection range.
    pub selection: [u32; 4],
}

/// Reference occurrence.
pub struct Reference {
    /// Reference source path.
    pub path: String,

    /// Reference range.
    pub range: [u32; 4],

    /// Whether the reference is a declaration.
    pub declaration: bool,
}

/// Document or local symbol.
pub struct Symbol {
    /// Symbol name.
    pub name: String,

    /// Symbol source path.
    pub path: String,

    /// Full symbol range.
    pub range: [u32; 4],

    /// Name-selection range.
    pub selection: [u32; 4],

    /// Symbol kind.
    pub kind: native::EditorSymbolKind,

    /// Symbol modifier bits.
    pub modifiers: u32,

    /// Whether the symbol is a declaration.
    pub declaration: bool,
}

struct ManyResults<T> {
    values: Vec<T>,
    error: Option<io::Error>,
}

impl<T> Default for ManyResults<T> {
    fn default() -> Self {
        Self {
            values: Vec::new(),
            error: None,
        }
    }
}

struct OneResult<T> {
    value: Option<T>,
    error: Option<io::Error>,
}

impl<T> Default for OneResult<T> {
    fn default() -> Self {
        Self {
            value: None,
            error: None,
        }
    }
}

trait ResultContext {
    fn error(&mut self) -> &mut Option<io::Error>;
}

impl<T> ResultContext for ManyResults<T> {
    fn error(&mut self) -> &mut Option<io::Error> {
        &mut self.error
    }
}

impl<T> ResultContext for OneResult<T> {
    fn error(&mut self) -> &mut Option<io::Error> {
        &mut self.error
    }
}

fn text(value: &str) -> native::Text {
    text_bytes(value.as_bytes())
}

fn text_bytes(value: &[u8]) -> native::Text {
    native::Text {
        data: value.as_ptr(),
        length: value.len(),
    }
}

// The caller must keep the native bytes alive and immutable for the returned borrow.
unsafe fn borrow_text<'text>(value: native::Text) -> io::Result<&'text str> {
    if value.data.is_null() {
        return if value.length == 0 {
            Ok("")
        } else {
            Err(io::Error::other("invalid native text"))
        };
    }

    str::from_utf8(unsafe { slice::from_raw_parts(value.data, value.length) })
        .map_err(io::Error::other)
}

fn read_text(value: native::Text) -> io::Result<String> {
    unsafe { borrow_text(value) }.map(str::to_owned)
}

fn range(value: native::Location) -> [u32; 4] {
    [
        value.begin_line,
        value.begin_column,
        value.end_line,
        value.end_column,
    ]
}

fn callback_result<T: ResultContext>(context: *mut c_void, result: io::Result<()>) -> u8 {
    match result {
        Ok(()) => 1,

        Err(error) => {
            if let Some(context) = unsafe { context.cast::<T>().as_mut() } {
                *context.error() = Some(error);
            }

            0
        }
    }
}

unsafe extern "C" fn hover_callback(context: *mut c_void, value: *const native::EditorHover) -> u8 {
    let result = (|| {
        let value = unsafe { value.as_ref() }.ok_or_else(|| io::Error::other("null hover"))?;

        let context = unsafe { context.cast::<OneResult<Hover>>().as_mut() }
            .ok_or_else(|| io::Error::other("null hover callback context"))?;

        if context.value.is_some() {
            return Err(io::Error::other("native hover returned multiple results"));
        }

        context.value = Some(Hover {
            name: read_text(value.name)?,
            type_: read_text(value.type_)?,
            documentation_symbol: read_text(value.documentation)?,
            range: (value.has_range != 0).then(|| range(value.range)),
        });

        Ok(())
    })();

    callback_result::<OneResult<Hover>>(context, result)
}

unsafe extern "C" fn completion_callback(
    context: *mut c_void,
    value: *const native::EditorCompletionItem,
) -> u8 {
    let result = (|| {
        let value = unsafe { value.as_ref() }.ok_or_else(|| io::Error::other("null completion"))?;

        let context = unsafe { context.cast::<ManyResults<Completion>>().as_mut() }
            .ok_or_else(|| io::Error::other("null completion callback context"))?;

        context.values.push(Completion {
            name: read_text(value.name)?,
            detail: read_text(value.detail)?,
            documentation_symbol: read_text(value.documentation)?,
            insert: read_text(value.insert)?,
            range: (value.has_range != 0).then(|| range(value.range)),
            mode: value.mode,
            kind: value.kind,
            deprecated: value.deprecated != 0,
        });

        Ok(())
    })();

    callback_result::<ManyResults<Completion>>(context, result)
}

unsafe extern "C" fn signature_callback(
    context: *mut c_void,
    value: *const native::EditorSignatureHelp,
) -> u8 {
    let result = (|| {
        let value = unsafe { value.as_ref() }.ok_or_else(|| io::Error::other("null signature"))?;

        let context = unsafe { context.cast::<OneResult<SignatureHelp>>().as_mut() }
            .ok_or_else(|| io::Error::other("null signature callback context"))?;

        if context.value.is_some() {
            return Err(io::Error::other(
                "native signature help returned multiple results",
            ));
        }

        let parameters = if value.parameters.is_null() {
            Vec::new()
        } else {
            unsafe { slice::from_raw_parts(value.parameters, value.parameter_count) }
                .iter()
                .map(|value| read_text(*value))
                .collect::<io::Result<Vec<_>>>()?
        };

        context.value = Some(SignatureHelp {
            label: read_text(value.label)?,
            parameters,
            active_parameter: (value.has_active_parameter != 0).then_some(value.active_parameter),
        });

        Ok(())
    })();

    callback_result::<OneResult<SignatureHelp>>(context, result)
}

unsafe extern "C" fn navigation_callback(
    context: *mut c_void,
    value: *const native::EditorNavigation,
) -> u8 {
    let result = (|| {
        let value = unsafe { value.as_ref() }.ok_or_else(|| io::Error::other("null navigation"))?;

        let context = unsafe { context.cast::<ManyResults<Navigation>>().as_mut() }
            .ok_or_else(|| io::Error::other("null navigation callback context"))?;

        context.values.push(Navigation {
            path: read_text(value.path)?,
            range: range(value.range),
            selection: range(value.selection),
        });

        Ok(())
    })();

    callback_result::<ManyResults<Navigation>>(context, result)
}

unsafe extern "C" fn reference_callback(
    context: *mut c_void,
    value: *const native::EditorReference,
) -> u8 {
    let result = (|| {
        let value = unsafe { value.as_ref() }.ok_or_else(|| io::Error::other("null reference"))?;

        let context = unsafe { context.cast::<ManyResults<Reference>>().as_mut() }
            .ok_or_else(|| io::Error::other("null reference callback context"))?;

        context.values.push(Reference {
            path: read_text(value.path)?,
            range: range(value.range),
            declaration: value.declaration != 0,
        });

        Ok(())
    })();

    callback_result::<ManyResults<Reference>>(context, result)
}

unsafe extern "C" fn symbol_callback(
    context: *mut c_void,
    value: *const native::EditorSymbol,
) -> u8 {
    let result = (|| {
        let value = unsafe { value.as_ref() }.ok_or_else(|| io::Error::other("null symbol"))?;

        let context = unsafe { context.cast::<ManyResults<Symbol>>().as_mut() }
            .ok_or_else(|| io::Error::other("null symbol callback context"))?;

        context.values.push(Symbol {
            name: read_text(value.name)?,
            path: read_text(value.path)?,
            range: range(value.range),
            selection: range(value.selection),
            kind: value.kind,
            modifiers: value.modifiers,
            declaration: value.declaration != 0,
        });

        Ok(())
    })();

    callback_result::<ManyResults<Symbol>>(context, result)
}

unsafe extern "C" fn prepared_symbol_callback(
    context: *mut c_void,
    value: *const native::EditorSymbol,
) -> u8 {
    let result = (|| {
        let value = unsafe { value.as_ref() }.ok_or_else(|| io::Error::other("null symbol"))?;

        let context = unsafe { context.cast::<OneResult<Symbol>>().as_mut() }
            .ok_or_else(|| io::Error::other("null symbol callback context"))?;

        if context.value.is_some() {
            return Err(io::Error::other("native prepare returned multiple symbols"));
        }

        context.value = Some(Symbol {
            name: read_text(value.name)?,
            path: read_text(value.path)?,
            range: range(value.range),
            selection: range(value.selection),
            kind: value.kind,
            modifiers: value.modifiers,
            declaration: value.declaration != 0,
        });

        Ok(())
    })();

    callback_result::<OneResult<Symbol>>(context, result)
}

unsafe extern "C" fn token_callback(
    context: *mut c_void,
    value: *const native::EditorSemanticToken,
) -> u8 {
    let result = (|| {
        let value = unsafe { value.as_ref() }.ok_or_else(|| io::Error::other("null token"))?;

        let context = unsafe { context.cast::<ManyResults<SemanticToken>>().as_mut() }
            .ok_or_else(|| io::Error::other("null token callback context"))?;

        context.values.push(SemanticToken {
            range: range(value.range),
            kind: value.kind,
            modifiers: value.modifiers,
            declaration: value.declaration != 0,
        });

        Ok(())
    })();

    callback_result::<ManyResults<SemanticToken>>(context, result)
}

unsafe extern "C" fn hint_callback(
    context: *mut c_void,
    value: *const native::EditorTypeHint,
) -> u8 {
    let result = (|| {
        let value = unsafe { value.as_ref() }.ok_or_else(|| io::Error::other("null hint"))?;

        let context = unsafe { context.cast::<ManyResults<TypeHint>>().as_mut() }
            .ok_or_else(|| io::Error::other("null hint callback context"))?;

        context.values.push(TypeHint {
            range: range(value.range),
            type_: read_text(value.type_)?,
        });

        Ok(())
    })();

    callback_result::<ManyResults<TypeHint>>(context, result)
}

impl Checker {
    fn editor_call<T: ResultContext>(
        operation: impl FnOnce(*mut c_void, *mut native::String) -> i32,
        context: &mut T,
    ) -> io::Result<()> {
        let context_pointer = ptr::from_mut(context).cast();
        let mut error = native::String::default();
        let status = operation(context_pointer, &raw mut error);
        let message = take_string(error)?;

        if let Some(error) = context.error().take() {
            return Err(error);
        }

        if status == native::Status::StatusSuccess as i32 {
            Ok(())
        } else if message.is_empty() {
            Err(io::Error::other("native editor operation failed"))
        } else {
            Err(io::Error::other(message))
        }
    }
}

fn take_string(value: native::String) -> io::Result<String> {
    let result = if value.data.is_null() {
        if value.length == 0 {
            Ok(String::new())
        } else {
            Err(io::Error::other("invalid native string"))
        }
    } else {
        str::from_utf8(unsafe { slice::from_raw_parts(value.data, value.length) })
            .map(str::to_owned)
            .map_err(io::Error::other)
    };

    unsafe { native::string_destroy(value) };

    result
}
