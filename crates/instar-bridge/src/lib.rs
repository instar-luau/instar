//! Native Luau analysis bridge.

#![expect(
    unsafe_code,
    unsafe_op_in_unsafe_fn,
    reason = "the bridge owns the C ABI marshalling boundary"
)]

use std::{collections::BTreeMap, ffi::c_void, io, path::Path, ptr, slice, str};

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
    /// Originating module.
    pub from: &'request str,

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
    fn configuration(&mut self, _: &str) -> io::Result<Option<&Configuration>> {
        Ok(None)
    }

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

/// Alias parsing options for a configuration source.
pub struct AliasOptions<'value> {
    /// Replaces existing aliases.
    pub overwrite: bool,

    /// Configuration location used to resolve aliases.
    pub location: Option<&'value str>,
}

/// Options used when parsing a configuration source.
#[derive(Default)]
pub struct ConfigurationOptions<'value> {
    /// Enables compatibility behavior.
    pub compatibility: bool,

    /// Alias parsing options.
    pub aliases: Option<AliasOptions<'value>>,
}

impl ConfigurationOptions<'_> {
    fn native(&self) -> native::ConfigurationOptions {
        let aliases = self.aliases.as_ref();

        native::ConfigurationOptions {
            compat: u8::from(self.compatibility),
            has_alias_options: u8::from(aliases.is_some()),
            overwrite_aliases: u8::from(aliases.is_some_and(|value| value.overwrite)),
            has_config_location: u8::from(aliases.is_some_and(|value| value.location.is_some())),
            config_location: aliases
                .and_then(|value| value.location)
                .map_or_else(|| text(""), text),
        }
    }
}

/// Native configuration handle.
pub struct Configuration {
    handle: *mut c_void,
}

impl Configuration {
    /// Creates an empty configuration.
    ///
    /// # Errors
    /// Returns an error when native configuration creation fails.
    pub fn new() -> io::Result<Self> {
        let mut error = native::String::default();

        let handle = unsafe { native::configuration_create(&raw mut error) };

        if handle.is_null() {
            Err(io::Error::other(take_string(error)?))
        } else {
            Ok(Self { handle })
        }
    }

    /// Loads JSON configuration data.
    ///
    /// # Errors
    /// Returns an error when native configuration parsing fails.
    pub fn load_json(&mut self, source: &[u8]) -> io::Result<()> {
        self.load_json_with_options(source, &ConfigurationOptions::default())
    }

    /// Loads JSON configuration data with explicit options.
    ///
    /// # Errors
    /// Returns an error when native configuration parsing fails.
    pub fn load_json_with_options(
        &mut self,
        source: &[u8],
        options: &ConfigurationOptions<'_>,
    ) -> io::Result<()> {
        let options = options.native();
        let handle = self.handle;

        Self::call(|error| unsafe {
            native::configuration_parse_json(handle, text_bytes(source), &raw const options, error)
        })
    }

    /// Loads executable Luau configuration data.
    ///
    /// # Errors
    /// Returns an error when native configuration extraction fails.
    pub fn load_luau(&mut self, source: &[u8]) -> io::Result<()> {
        self.load_luau_with_options(source, &ConfigurationOptions::default())
    }

    /// Loads executable Luau configuration data with explicit options.
    ///
    /// # Errors
    /// Returns an error when native configuration extraction fails.
    pub fn load_luau_with_options(
        &mut self,
        source: &[u8],
        options: &ConfigurationOptions<'_>,
    ) -> io::Result<()> {
        let options = options.native();
        let handle = self.handle;

        Self::call(|error| unsafe {
            native::configuration_extract_luau(
                handle,
                text_bytes(source),
                &raw const options,
                error,
            )
        })
    }

    /// Sets the type-checking mode.
    ///
    /// # Errors
    /// Returns an error when native configuration updates fail.
    pub fn set_mode(&mut self, mode: native::ConfigurationMode) -> io::Result<()> {
        let handle = self.handle;

        Self::call(|error| unsafe { native::configuration_set_mode(handle, mode, error) })
    }

    /// Returns the type-checking mode.
    #[must_use]
    pub fn mode(&self) -> native::ConfigurationMode {
        unsafe { native::configuration_mode(self.pointer()) }
    }

    /// Enables comment capture.
    ///
    /// # Errors
    /// Returns an error when native configuration updates fail.
    pub fn capture_comments(&mut self) -> io::Result<()> {
        let handle = self.handle;

        Self::call(|error| unsafe { native::configuration_set_capture_comments(handle, 1, error) })
    }

    /// Parses and enumerates aliases in configuration data.
    ///
    /// # Errors
    /// Returns an error when parsing or alias enumeration fails.
    pub fn aliases(source: &[u8], executable: bool) -> io::Result<BTreeMap<String, String>> {
        let mut configuration = Self::new()?;

        let options = ConfigurationOptions {
            aliases: Some(AliasOptions {
                overwrite: false,
                location: None,
            }),
            ..ConfigurationOptions::default()
        };

        if executable {
            configuration.load_luau_with_options(source, &options)?;
        } else {
            configuration.load_json_with_options(source, &options)?;
        }

        let mut context = AliasContext {
            values: BTreeMap::new(),
            error: None,
        };

        let mut error = native::String::default();

        let status = unsafe {
            native::configuration_aliases(
                configuration.handle,
                Some(alias_callback),
                ptr::from_mut(&mut context).cast(),
                &raw mut error,
            )
        };

        let message = take_string(error)?;

        if let Some(error) = context.error {
            return Err(error);
        }

        if status == native::Status::StatusSuccess as i32 {
            Ok(context.values)
        } else if message.is_empty() {
            Err(io::Error::other("native alias enumeration failed"))
        } else {
            Err(io::Error::other(message))
        }
    }

    fn pointer(&self) -> *const c_void {
        self.handle.cast_const()
    }

    fn call(operation: impl FnOnce(*mut native::String) -> i32) -> io::Result<()> {
        let mut error = native::String::default();
        let status = operation(&raw mut error);
        let message = take_string(error)?;

        if status == native::Status::StatusSuccess as i32 {
            Ok(())
        } else if message.is_empty() {
            Err(io::Error::other("native configuration operation failed"))
        } else {
            Err(io::Error::other(message))
        }
    }
}

impl Drop for Configuration {
    fn drop(&mut self) {
        unsafe { native::configuration_destroy(self.handle) };
    }
}

struct AliasContext {
    values: BTreeMap<String, String>,
    error: Option<io::Error>,
}

unsafe extern "C" fn alias_callback(context: *mut c_void, value: *const native::Alias) -> u8 {
    let result = (|| {
        let value = unsafe { value.as_ref() }.ok_or_else(|| io::Error::other("null alias"))?;

        let name = read_text(value.name)?;
        let target = read_text(value.value)?;

        let context = unsafe { context.cast::<AliasContext>().as_mut() }
            .ok_or_else(|| io::Error::other("null alias callback context"))?;

        context.values.insert(name, target);

        Ok(())
    })();

    callback_status(context, result, set_alias_error)
}

struct CallbackContext<'callbacks> {
    callbacks: &'callbacks mut dyn Callbacks,
    source: Option<Source<'callbacks>>,
    error: Option<io::Error>,
}

fn callback_context<'callbacks>(
    context: *mut c_void,
) -> io::Result<&'callbacks mut CallbackContext<'callbacks>> {
    unsafe { context.cast::<CallbackContext<'callbacks>>().as_mut() }
        .ok_or_else(|| io::Error::other("null native callback context"))
}

fn set_alias_error(context: &mut AliasContext, error: io::Error) {
    context.error = Some(error);
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
        let name = read_text(name)?;
        let context = callback_context(context)?;
        let source = context.callbacks.source(&name)?;
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
        let name = read_text(name)?;
        let configuration = callback_context(context)?.callbacks.configuration(&name)?;

        unsafe { *result = configuration.map_or(ptr::null(), Configuration::pointer) };

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

        let from = read_text(request.from)?;

        let request_value = ResolveRequest {
            from: &from,
            optional: request.optional != 0,
            location: range(request.expression),
        };

        let path = callback_context(context)?
            .callbacks
            .resolve(&request_value)?;

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

        let path = read_text(value.path)?;
        let message = read_text(value.message)?;
        let related_path = read_text(value.related.path)?;
        let related_message = read_text(value.related.message)?;

        let diagnostic = Diagnostic {
            path: &path,
            location: range(value.location),
            severity: value.severity,
            message: &message,
            related: (value.has_related != 0).then_some(RelatedDiagnostic {
                path: &related_path,
                location: range(value.related.location),
                message: &related_message,
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
    /// Selects the legacy type solver.
    pub old_solver: u8,

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

/// Roblox instance metadata supplied to the checker.
pub struct RobloxNode<'name> {
    /// Instance name.
    pub name: &'name str,

    /// Class name.
    pub class_name: &'name str,
}

/// Native checker handle with safe callback dispatch.
pub struct Checker<'callbacks> {
    handle: *mut c_void,
    context: Box<CallbackContext<'callbacks>>,
}

impl<'callbacks> Checker<'callbacks> {
    /// Creates a checker using project callbacks.
    ///
    /// # Errors
    /// Returns an error when native checker creation fails.
    pub fn new(
        callbacks: &'callbacks mut dyn Callbacks,
        options: &CheckerOptions,
    ) -> io::Result<Self> {
        let mut context = Box::new(CallbackContext {
            callbacks,
            source: None,
            error: None,
        });

        let native_options = native::FrontendOptions {
            old_solver: options.old_solver,
            retain_full_type_graphs: options.retain_full_type_graphs,
            for_autocomplete: options.for_autocomplete,
            run_lint_checks: options.run_lint_checks,
        };

        let mut error = native::String::default();

        let handle = unsafe {
            native::checker_create(
                &CALLBACKS,
                ptr::from_mut(context.as_mut()).cast(),
                &raw const native_options,
                &raw mut error,
            )
        };

        if handle.is_null() {
            let callback_error = context.error.take();
            let message = take_string(error)?;

            if let Some(error) = callback_error {
                Err(error)
            } else {
                Err(io::Error::other(if message.is_empty() {
                    "cannot create native checker"
                } else {
                    &message
                }))
            }
        } else {
            Ok(Self { handle, context })
        }
    }

    fn call_checker(&self, operation: impl FnOnce(*mut native::String) -> i32) -> io::Result<()> {
        let mut error = native::String::default();
        let status = operation(&raw mut error);
        let message = take_string(error)?;

        if let Some(error) = self.context.error.as_ref() {
            return Err(io::Error::new(error.kind(), error.to_string()));
        }

        if status == native::Status::StatusSuccess as i32 {
            Ok(())
        } else if message.is_empty() {
            Err(io::Error::other("native checker operation failed"))
        } else {
            Err(io::Error::other(message))
        }
    }

    /// Freezes checker configuration.
    ///
    /// # Errors
    /// Returns an error when native checker freezing fails.
    pub fn freeze(&self) -> io::Result<()> {
        self.call_checker(|error| unsafe { native::checker_freeze(self.handle, error) })
    }

    /// Loads a definition source.
    ///
    /// # Errors
    /// Returns an error when the definition cannot be loaded.
    pub fn load_definition(&self, source: &[u8], package: &str) -> io::Result<()> {
        let options = native::DefinitionOptions {
            capture_comments: 0,
            type_check_for_autocomplete: 0,
        };

        self.call_checker(|error| unsafe {
            native::checker_load_definition(
                self.handle,
                text_bytes(source),
                text(package),
                &raw const options,
                error,
            )
        })
    }

    fn call_name(
        &self,
        path: &Path,
        operation: impl FnOnce(native::Text, *mut native::String) -> i32,
    ) -> io::Result<()> {
        let name = path
            .to_str()
            .ok_or_else(|| io::Error::other("module identity requires UTF-8"))?;

        self.call_checker(|error| operation(text(name), error))
    }

    /// Parses one module.
    ///
    /// # Errors
    /// Returns an error when native parsing fails.
    pub fn parse(&self, path: &Path) -> io::Result<()> {
        self.call_name(path, |name, error| unsafe {
            native::checker_parse(self.handle, name, error)
        })
    }

    /// Emits parse diagnostics for one module.
    ///
    /// # Errors
    /// Returns an error when native diagnostic emission fails.
    pub fn parse_diagnostics(&self, path: &Path) -> io::Result<()> {
        self.call_name(path, |name, error| unsafe {
            native::checker_parse_diagnostics(self.handle, name, error)
        })
    }

    /// Checks one module.
    ///
    /// # Errors
    /// Returns an error when native checking fails.
    pub fn check(&self, path: &Path) -> io::Result<()> {
        self.call_name(path, |name, error| unsafe {
            native::checker_check(self.handle, name, error)
        })
    }

    /// Emits checker results for one module.
    ///
    /// # Errors
    /// Returns an error when native result emission fails.
    pub fn result(&self, path: &Path) -> io::Result<()> {
        self.result_with_options(path, &ResultOptions::default())
    }

    /// Emits checker results for one module with explicit options.
    ///
    /// # Errors
    /// Returns an error when native result emission fails.
    pub fn result_with_options(&self, path: &Path, options: &ResultOptions) -> io::Result<()> {
        self.call_name(path, |name, error| unsafe {
            native::checker_result(
                self.handle,
                name,
                u8::from(options.accumulate_nested),
                u8::from(options.for_autocomplete),
                error,
            )
        })
    }

    /// Attaches inferred type data to one module.
    ///
    /// # Errors
    /// Returns an error when native type-data attachment fails.
    pub fn attach_type_data(&self, path: &Path) -> io::Result<()> {
        self.call_name(path, |name, error| unsafe {
            native::checker_attach_type_data(self.handle, name, error)
        })
    }

    /// Registers Roblox classes and instances.
    ///
    /// # Errors
    /// Returns an error when native Roblox registration fails.
    pub fn register_roblox(
        &self,
        classes: &[RobloxClass<'_>],
        nodes: &[RobloxNode<'_>],
    ) -> io::Result<()> {
        let classes = classes
            .iter()
            .map(|value| native::RobloxClass {
                name: text(value.name),
                service: u8::from(value.service),
                creatable: u8::from(value.creatable),
            })
            .collect::<Vec<_>>();

        let nodes = nodes
            .iter()
            .map(|value| native::RobloxNode {
                name: text(value.name),
                class_name: text(value.class_name),
            })
            .collect::<Vec<_>>();

        self.call_checker(|error| unsafe {
            native::checker_register_roblox(
                self.handle,
                classes.as_ptr(),
                classes.len(),
                nodes.as_ptr(),
                nodes.len(),
                error,
            )
        })
    }

    /// Runs one typed editor operation.
    ///
    /// # Errors
    /// Returns an error when the native operation or result conversion fails.
    pub fn editor(
        &self,
        path: &str,
        line: u32,
        column: u32,
        operation: EditorOperation<'_>,
    ) -> io::Result<Vec<EditorItem>> {
        unsafe { editor(self.handle, path, line, column, operation) }
    }

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

    /// Enumerates checker globals.
    ///
    /// # Errors
    /// Returns an error when enumeration or callback processing fails.
    pub fn globals(&self, callback: &mut impl FnMut(&str) -> io::Result<()>) -> io::Result<()> {
        Self::items(
            |item, context, error| unsafe {
                native::checker_globals(self.handle, item, context, error)
            },
            callback,
        )
    }

    /// Enumerates checker timeout modules.
    ///
    /// # Errors
    /// Returns an error when enumeration or callback processing fails.
    pub fn timeouts(&self, callback: &mut impl FnMut(&str) -> io::Result<()>) -> io::Result<()> {
        Self::items(
            |item, context, error| unsafe {
                native::checker_timeouts(self.handle, item, context, error)
            },
            callback,
        )
    }

    /// Enumerates modules known to the checker.
    ///
    /// # Errors
    /// Returns an error when enumeration or callback processing fails.
    pub fn modules(&self, callback: &mut impl FnMut(&str) -> io::Result<()>) -> io::Result<()> {
        Self::items(
            |item, context, error| unsafe {
                native::checker_modules(self.handle, item, context, error)
            },
            callback,
        )
    }
}

/// Reusable native checker session.
pub struct NativeSession {
    handle: *mut c_void,
}

impl NativeSession {
    /// Creates an empty reusable checker session.
    ///
    /// # Errors
    /// Returns an error when native checker creation fails.
    pub fn new(options: &CheckerOptions) -> io::Result<Self> {
        let native_options = native::FrontendOptions {
            old_solver: options.old_solver,
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
        &self,
        callbacks: &mut dyn Callbacks,
        operation: impl FnOnce(&Self) -> io::Result<T>,
    ) -> io::Result<T> {
        let mut context = Box::new(CallbackContext {
            callbacks,
            source: None,
            error: None,
        });

        Self::call(|error| unsafe {
            native::checker_set_context(self.handle, ptr::from_mut(context.as_mut()).cast(), error)
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
        &self,
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

    /// Registers Roblox classes and instances.
    ///
    /// # Errors
    /// Returns an error when registration fails.
    pub fn register_roblox(
        &self,
        callbacks: &mut dyn Callbacks,
        classes: &[RobloxClass<'_>],
        nodes: &[RobloxNode<'_>],
    ) -> io::Result<()> {
        self.with_callbacks(callbacks, |session| {
            let classes = classes
                .iter()
                .map(|value| native::RobloxClass {
                    name: text(value.name),
                    service: u8::from(value.service),
                    creatable: u8::from(value.creatable),
                })
                .collect::<Vec<_>>();

            let nodes = nodes
                .iter()
                .map(|value| native::RobloxNode {
                    name: text(value.name),
                    class_name: text(value.class_name),
                })
                .collect::<Vec<_>>();

            Self::call(|error| unsafe {
                native::checker_register_roblox(
                    session.handle,
                    classes.as_ptr(),
                    classes.len(),
                    nodes.as_ptr(),
                    nodes.len(),
                    error,
                )
            })
        })
    }

    /// Freezes checker configuration.
    ///
    /// # Errors
    /// Returns an error when freezing fails.
    pub fn freeze(&self) -> io::Result<()> {
        Self::call(|error| unsafe { native::checker_freeze(self.handle, error) })
    }

    /// Marks a module and its dependents dirty.
    ///
    /// # Errors
    /// Returns an error when invalidation fails.
    pub fn mark_dirty(&self, path: &Path) -> io::Result<()> {
        let name = path
            .to_str()
            .ok_or_else(|| io::Error::other("module identity requires UTF-8"))?;

        Self::call(|error| unsafe { native::checker_mark_dirty(self.handle, text(name), error) })
    }

    /// Clears parsed and checked source modules.
    ///
    /// # Errors
    /// Returns an error when clearing fails.
    pub fn clear(&self) -> io::Result<()> {
        Self::call(|error| unsafe { native::checker_clear(self.handle, error) })
    }

    /// Checks a module.
    ///
    /// # Errors
    /// Returns an error when checking fails.
    pub fn check(&self, callbacks: &mut dyn Callbacks, path: &Path) -> io::Result<()> {
        self.with_callbacks(callbacks, |session| {
            Self::call_name(path, |name, error| unsafe {
                native::checker_check(session.handle, name, error)
            })
        })
    }

    /// Emits diagnostics for a checked module.
    ///
    /// # Errors
    /// Returns an error when result emission fails.
    pub fn result(&self, callbacks: &mut dyn Callbacks, path: &Path) -> io::Result<()> {
        self.with_callbacks(callbacks, |session| {
            Self::call_name(path, |name, error| unsafe {
                native::checker_result(session.handle, name, 0, 0, error)
            })
        })
    }

    /// Runs one editor operation against the persistent checker.
    ///
    /// # Errors
    /// Returns an error when the native editor operation fails.
    pub fn editor(
        &self,
        callbacks: &mut dyn Callbacks,
        path: &str,
        line: u32,
        column: u32,
        operation: EditorOperation<'_>,
    ) -> io::Result<Vec<EditorItem>> {
        self.with_callbacks(callbacks, |session| unsafe {
            editor(session.handle, path, line, column, operation)
        })
    }

    /// Enumerates timeout modules.
    ///
    /// # Errors
    /// Returns an error when enumeration fails.
    pub fn timeouts(&self, callback: &mut impl FnMut(&str) -> io::Result<()>) -> io::Result<()> {
        Checker::items(
            |item, context, error| unsafe {
                native::checker_timeouts(self.handle, item, context, error)
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

impl Drop for NativeSession {
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
        let value = read_text(value)?;

        let context = unsafe { context.cast::<ItemContext<'_>>().as_mut() }
            .ok_or_else(|| io::Error::other("null item callback context"))?;

        (context.callback)(&value)
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

impl Drop for Checker<'_> {
    fn drop(&mut self) {
        let _ = &self.context;

        unsafe { native::checker_destroy(self.handle) };
    }
}

/// Editor operation supported by the native bridge.
#[derive(Clone, Copy)]
pub enum EditorOperation<'value> {
    /// Hover information at a source position.
    Hover,

    /// Completion items at a source position.
    Completion,

    /// Resolve one completion item.
    CompletionResolve(&'value str),

    /// Signature help at a source position.
    Signature,

    /// Inferred type hints for a source.
    TypeHints,

    /// Semantic tokens for a source.
    SemanticTokens,

    /// Symbol definition at a source position.
    Definition,

    /// Symbol declaration at a source position.
    Declaration,

    /// Type definition at a source position.
    TypeDefinition,

    /// References at a source position.
    References,

    /// Symbol preparation at a source position.
    Prepare,

    /// Local references at a source position.
    LocalReferences,

    /// Symbols in scope at a source position.
    Scope,
}

/// Owned result item returned by a native editor operation.
#[derive(Default)]
pub struct EditorItem {
    /// Symbol or completion name.
    pub name: Option<String>,

    /// Type or detail text.
    pub description: Option<String>,

    /// Documentation identifier.
    pub documentation: Option<String>,

    /// Documentation text.
    pub documentation_text: Option<String>,

    /// Source path.
    pub path: Option<String>,

    /// Full source range.
    pub range: Option<[u32; 4]>,

    /// Symbol or token kind.
    pub kind: Option<u32>,

    /// Modifier bits.
    pub modifiers: Option<u32>,

    /// Name-selection range.
    pub selection: Option<[u32; 4]>,

    /// Whether the item declares a symbol.
    pub declaration: Option<bool>,

    /// Completion insertion text.
    pub insert: Option<String>,

    /// Whether the item is deprecated.
    pub deprecated: Option<bool>,

    /// Active signature parameter.
    pub active: Option<u32>,

    /// Signature parameter labels.
    pub parameters: Option<Vec<String>>,
}

struct EditorContext {
    items: Vec<EditorItem>,
    error: Option<io::Error>,
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

fn read_text(value: native::Text) -> io::Result<String> {
    if value.data.is_null() {
        return if value.length == 0 {
            Ok(String::new())
        } else {
            Err(io::Error::other("invalid native text"))
        };
    }

    str::from_utf8(unsafe { slice::from_raw_parts(value.data, value.length) })
        .map(str::to_owned)
        .map_err(io::Error::other)
}

fn range(value: native::Location) -> [u32; 4] {
    [
        value.begin_line,
        value.begin_column,
        value.end_line,
        value.end_column,
    ]
}

fn callback_result(context: *mut c_void, result: io::Result<()>) -> u8 {
    match result {
        Ok(()) => 1,

        Err(error) => {
            if !context.is_null() {
                unsafe { (*context.cast::<EditorContext>()).error = Some(error) };
            }

            0
        }
    }
}

unsafe extern "C" fn hover_callback(context: *mut c_void, value: *const native::EditorHover) -> u8 {
    let result = (|| {
        let value = unsafe { value.as_ref() }.ok_or_else(|| io::Error::other("null hover"))?;

        unsafe {
            (*context.cast::<EditorContext>()).items.push(EditorItem {
                name: Some(read_text(value.name)?),
                description: Some(read_text(value.type_)?),
                documentation_text: Some(read_text(value.documentation)?),
                range: Some(range(value.range)),
                ..EditorItem::default()
            });
        };

        Ok(())
    })();

    callback_result(context, result)
}

unsafe extern "C" fn completion_callback(
    context: *mut c_void,
    value: *const native::EditorCompletionItem,
) -> u8 {
    let result = (|| {
        let value = unsafe { value.as_ref() }.ok_or_else(|| io::Error::other("null completion"))?;

        unsafe {
            (*context.cast::<EditorContext>()).items.push(EditorItem {
                name: Some(read_text(value.name)?),
                description: Some(read_text(value.detail)?),
                documentation_text: Some(read_text(value.documentation)?),
                insert: Some(read_text(value.insert)?),
                range: Some(range(value.range)),
                kind: Some(u32::try_from(value.kind as i32).unwrap_or_default()),
                deprecated: Some(value.deprecated != 0),
                ..EditorItem::default()
            });
        };

        Ok(())
    })();

    callback_result(context, result)
}

unsafe extern "C" fn signature_callback(
    context: *mut c_void,
    value: *const native::EditorSignatureHelp,
) -> u8 {
    let result = (|| {
        let value = unsafe { value.as_ref() }.ok_or_else(|| io::Error::other("null signature"))?;

        let parameters = if value.parameters.is_null() {
            Vec::new()
        } else {
            unsafe { slice::from_raw_parts(value.parameters, value.parameter_count) }
                .iter()
                .map(|value| read_text(*value))
                .collect::<io::Result<Vec<_>>>()?
        };

        unsafe {
            (*context.cast::<EditorContext>()).items.push(EditorItem {
                description: Some(read_text(value.label)?),
                parameters: Some(parameters),
                active: (value.has_active_parameter != 0).then_some(value.active_parameter),
                ..EditorItem::default()
            });
        };

        Ok(())
    })();

    callback_result(context, result)
}

unsafe extern "C" fn navigation_callback(
    context: *mut c_void,
    value: *const native::EditorNavigation,
) -> u8 {
    let result = (|| {
        let value = unsafe { value.as_ref() }.ok_or_else(|| io::Error::other("null navigation"))?;

        unsafe {
            (*context.cast::<EditorContext>()).items.push(EditorItem {
                path: Some(read_text(value.path)?),
                range: Some(range(value.range)),
                selection: Some(range(value.selection)),
                ..EditorItem::default()
            });
        };

        Ok(())
    })();

    callback_result(context, result)
}

unsafe extern "C" fn reference_callback(
    context: *mut c_void,
    value: *const native::EditorReference,
) -> u8 {
    let result = (|| {
        let value = unsafe { value.as_ref() }.ok_or_else(|| io::Error::other("null reference"))?;

        unsafe {
            (*context.cast::<EditorContext>()).items.push(EditorItem {
                path: Some(read_text(value.path)?),
                range: Some(range(value.range)),
                declaration: Some(value.declaration != 0),
                ..EditorItem::default()
            });
        };

        Ok(())
    })();

    callback_result(context, result)
}

unsafe extern "C" fn symbol_callback(
    context: *mut c_void,
    value: *const native::EditorSymbol,
) -> u8 {
    let result = (|| {
        let value = unsafe { value.as_ref() }.ok_or_else(|| io::Error::other("null symbol"))?;

        unsafe {
            (*context.cast::<EditorContext>()).items.push(EditorItem {
                name: Some(read_text(value.name)?),
                path: Some(read_text(value.path)?),
                range: Some(range(value.range)),
                selection: Some(range(value.selection)),
                kind: Some(u32::try_from(value.kind as i32).unwrap_or_default()),
                modifiers: Some(value.modifiers),
                declaration: Some(value.declaration != 0),
                ..EditorItem::default()
            });
        };

        Ok(())
    })();

    callback_result(context, result)
}

unsafe extern "C" fn token_callback(
    context: *mut c_void,
    value: *const native::EditorSemanticToken,
) -> u8 {
    let result = (|| {
        let value = unsafe { value.as_ref() }.ok_or_else(|| io::Error::other("null token"))?;

        unsafe {
            (*context.cast::<EditorContext>()).items.push(EditorItem {
                range: Some(range(value.range)),
                kind: Some(u32::try_from(value.kind as i32).unwrap_or_default()),
                modifiers: Some(value.modifiers),
                declaration: Some(value.declaration != 0),
                ..EditorItem::default()
            });
        };

        Ok(())
    })();

    callback_result(context, result)
}

unsafe extern "C" fn hint_callback(
    context: *mut c_void,
    value: *const native::EditorTypeHint,
) -> u8 {
    let result = (|| {
        let value = unsafe { value.as_ref() }.ok_or_else(|| io::Error::other("null hint"))?;

        unsafe {
            (*context.cast::<EditorContext>()).items.push(EditorItem {
                description: Some(read_text(value.type_)?),
                range: Some(range(value.range)),
                ..EditorItem::default()
            });
        };

        Ok(())
    })();

    callback_result(context, result)
}

/// Runs one typed native editor operation and owns all returned text.
///
/// # Errors
/// Returns an error when the native operation fails or returns invalid UTF-8.
///
/// # Safety
/// `checker` must be a live checker handle created by `checker_create`.
#[expect(
    clippy::too_many_lines,
    reason = "the typed operation dispatch stays beside the ABI calls"
)]
pub unsafe fn editor(
    checker: *mut c_void,
    name: &str,
    line: u32,
    column: u32,
    operation: EditorOperation<'_>,
) -> io::Result<Vec<EditorItem>> {
    let mut context = EditorContext {
        items: Vec::new(),
        error: None,
    };

    let mut error = native::String::default();
    let name = text(name);
    let context_pointer = ptr::from_mut(&mut context).cast();

    let status = match operation {
        EditorOperation::Hover => native::editor_hover(
            checker,
            name,
            line,
            column,
            Some(hover_callback),
            context_pointer,
            &raw mut error,
        ),

        EditorOperation::Completion => native::editor_completion(
            checker,
            name,
            line,
            column,
            Some(completion_callback),
            context_pointer,
            &raw mut error,
        ),

        EditorOperation::CompletionResolve(item) => native::editor_completion_resolve(
            checker,
            name,
            text(item),
            line,
            column,
            Some(completion_callback),
            context_pointer,
            &raw mut error,
        ),

        EditorOperation::Signature => native::editor_signature_help(
            checker,
            name,
            line,
            column,
            Some(signature_callback),
            context_pointer,
            &raw mut error,
        ),

        EditorOperation::TypeHints => native::editor_type_hints(
            checker,
            name,
            Some(hint_callback),
            context_pointer,
            &raw mut error,
        ),

        EditorOperation::SemanticTokens => native::editor_semantic_tokens(
            checker,
            name,
            Some(token_callback),
            context_pointer,
            &raw mut error,
        ),

        EditorOperation::Definition => native::editor_definition(
            checker,
            name,
            line,
            column,
            Some(navigation_callback),
            context_pointer,
            &raw mut error,
        ),

        EditorOperation::Declaration => native::editor_declaration(
            checker,
            name,
            line,
            column,
            Some(navigation_callback),
            context_pointer,
            &raw mut error,
        ),

        EditorOperation::TypeDefinition => native::editor_type_definition(
            checker,
            name,
            line,
            column,
            Some(navigation_callback),
            context_pointer,
            &raw mut error,
        ),

        EditorOperation::References => native::editor_references(
            checker,
            name,
            line,
            column,
            Some(reference_callback),
            context_pointer,
            &raw mut error,
        ),

        EditorOperation::Prepare => native::editor_prepare(
            checker,
            name,
            line,
            column,
            Some(symbol_callback),
            context_pointer,
            &raw mut error,
        ),

        EditorOperation::LocalReferences => native::editor_local_references(
            checker,
            name,
            line,
            column,
            Some(symbol_callback),
            context_pointer,
            &raw mut error,
        ),

        EditorOperation::Scope => native::editor_scope(
            checker,
            name,
            line,
            column,
            Some(symbol_callback),
            context_pointer,
            &raw mut error,
        ),
    };

    let message = take_string(error)?;

    if status != native::Status::StatusSuccess as i32 {
        return Err(io::Error::other(if message.is_empty() {
            "native editor operation failed".to_owned()
        } else {
            message
        }));
    }

    context.error.map_or(Ok(context.items), Err)
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
