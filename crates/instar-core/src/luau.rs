#![expect(unsafe_code, reason = "Luau is exposed through a checked native ABI")]

use crate::{
    analysis::{
        Annotation, Diagnostic, Documentation, EditorEntry, EditorResult, Mode, Options, RelatedDiagnostic, Report,
    },
    project::{resolution::Resolver, roblox::Environment},
    source::Source,
};

use serde_json::Value;

use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::c_void,
    io,
    path::{Path, PathBuf},
    ptr,
    sync::Arc,
};

const NATIVE_TYPE_ERROR: u32 = 1;
const NATIVE_PARSE_ERROR: u32 = 2;
const NATIVE_LINT_WARNING: u32 = 3;

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct NativeSlice {
    data: *const u8,
    length: usize,
}

impl NativeSlice {
    fn new(bytes: &[u8]) -> Self {
        Self {
            data: bytes.as_ptr(),
            length: bytes.len(),
        }
    }
}

#[repr(C)]
#[derive(Default)]
struct NativeString {
    data: *mut u8,
    length: usize,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct NativeFrontendOptions {
    old_solver: u8,
    retain_full_type_graphs: u8,
    for_autocomplete: u8,
    run_lint_checks: u8,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct NativeConfigurationOptions {
    compat: u8,
    has_alias_options: u8,
    overwrite_aliases: u8,
    has_config_location: u8,
    config_location: NativeSlice,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct NativeDefinitionOptions {
    capture_comments: u8,
    type_check_for_autocomplete: u8,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct NativeRobloxClass {
    name: NativeSlice,
    service: u8,
    creatable: u8,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct NativeRobloxNode {
    name: NativeSlice,
    class_name: NativeSlice,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct NativeTypeCheckLimits {
    has_finish_time: u8,
    finish_time: f64,
    has_instantiation_child_limit: u8,
    instantiation_child_limit: i32,
    has_unifier_iteration_limit: u8,
    unifier_iteration_limit: i32,
    cancellation_token: *const c_void,
}

type SourceCallback = extern "C" fn(*mut c_void, NativeSlice, *mut NativeSlice, *mut u32) -> u8;
type ConfigurationCallback = extern "C" fn(*mut c_void, NativeSlice, *mut *const c_void) -> u8;

type ResolveCallback = extern "C" fn(
    *mut c_void,
    NativeSlice,
    u8,
    NativeSlice,
    NativeTypeCheckLimits,
    *mut NativeSlice,
    *mut u8,
    *mut u8,
) -> u8;

type DiagnosticCallback = extern "C" fn(
    *mut c_void,
    NativeSlice,
    u32,
    u32,
    u32,
    u32,
    u32,
    i32,
    u32,
    u8,
    NativeSlice,
    NativeSlice,
    u32,
    u32,
    u32,
    u32,
    NativeSlice,
) -> u8;

type AliasCallback = extern "C" fn(*mut c_void, NativeSlice, NativeSlice, NativeSlice) -> u8;
type ItemCallback = extern "C" fn(*mut c_void, NativeSlice) -> u8;

#[repr(C)]
struct NativeCallbacks {
    source: SourceCallback,
    configuration: ConfigurationCallback,
    resolve: ResolveCallback,
    diagnostic: DiagnosticCallback,
}

unsafe extern "C" {
    fn instar_string_destroy(value: NativeString);

    fn instar_configuration_new(error: *mut NativeString) -> *mut c_void;
    fn instar_configuration_destroy(configuration: *mut c_void);
    fn instar_configuration_parse_json(
        configuration: *mut c_void,
        source: NativeSlice,
        options: *const NativeConfigurationOptions,
        error: *mut NativeString,
    ) -> i32;
    fn instar_configuration_extract_luau(
        configuration: *mut c_void,
        source: NativeSlice,
        options: *const NativeConfigurationOptions,
        error: *mut NativeString,
    ) -> i32;
    fn instar_configuration_set_capture_comments(
        configuration: *mut c_void,
        capture_comments: u8,
        error: *mut NativeString,
    ) -> i32;
    fn instar_configuration_set_mode(
        configuration: *mut c_void,
        mode: u32,
        error: *mut NativeString,
    ) -> i32;
    fn instar_configuration_aliases(
        configuration: *const c_void,
        callback: AliasCallback,
        context: *mut c_void,
        error: *mut NativeString,
    ) -> i32;

    fn instar_checker_new(
        callbacks: *const NativeCallbacks,
        context: *mut c_void,
        options: *const NativeFrontendOptions,
        error: *mut NativeString,
    ) -> *mut c_void;
    fn instar_checker_register_builtins(
        checker: *mut c_void,
        for_autocomplete: u8,
        error: *mut NativeString,
    ) -> i32;
    fn instar_checker_destroy(checker: *mut c_void);
    fn instar_checker_freeze(checker: *mut c_void, error: *mut NativeString) -> i32;
    fn instar_checker_load_definition(
        checker: *mut c_void,
        source: NativeSlice,
        package_name: NativeSlice,
        options: *const NativeDefinitionOptions,
        error: *mut NativeString,
    ) -> i32;
    fn instar_checker_register_roblox_magic(
        checker: *mut c_void,
        classes: *const NativeRobloxClass,
        count: usize,
        nodes: *const NativeRobloxNode,
        node_count: usize,
        error: *mut NativeString,
    ) -> i32;
    fn instar_checker_parse(
        checker: *mut c_void,
        name: NativeSlice,
        error: *mut NativeString,
    ) -> i32;
    fn instar_checker_parse_diagnostics(
        checker: *mut c_void,
        name: NativeSlice,
        error: *mut NativeString,
    ) -> i32;
    fn instar_checker_ast(
        checker: *mut c_void,
        name: NativeSlice,
        output: *mut NativeString,
        error: *mut NativeString,
    ) -> i32;
    fn instar_checker_globals(
        checker: *mut c_void,
        callback: ItemCallback,
        context: *mut c_void,
        error: *mut NativeString,
    ) -> i32;
    fn instar_checker_check(
        checker: *mut c_void,
        name: NativeSlice,
        error: *mut NativeString,
    ) -> i32;
    fn instar_checker_result(
        checker: *mut c_void,
        name: NativeSlice,
        accumulate_nested: u8,
        for_autocomplete: u8,
        error: *mut NativeString,
    ) -> i32;
    fn instar_checker_editor(
        checker: *mut c_void,
        name: NativeSlice,
        line: u32,
        column: u32,
        operation: NativeSlice,
        output: *mut NativeString,
        error: *mut NativeString,
    ) -> i32;
    fn instar_checker_timeouts(
        checker: *mut c_void,
        callback: ItemCallback,
        context: *mut c_void,
        error: *mut NativeString,
    ) -> i32;
    fn instar_checker_modules(
        checker: *mut c_void,
        callback: ItemCallback,
        context: *mut c_void,
        error: *mut NativeString,
    ) -> i32;
    fn instar_checker_attach_type_data(
        checker: *mut c_void,
        name: NativeSlice,
        error: *mut NativeString,
    ) -> i32;
    fn instar_state_new(error: *mut NativeString) -> *mut c_void;
    fn instar_state_destroy(state: *mut c_void);
    fn instar_state_openlibs(state: *mut c_void, error: *mut NativeString) -> i32;
    fn instar_state_get_global(
        state: *mut c_void,
        name: NativeSlice,
        error: *mut NativeString,
    ) -> i32;
    fn instar_state_get_field(
        state: *mut c_void,
        index: i32,
        name: NativeSlice,
        error: *mut NativeString,
    ) -> i32;
    fn instar_state_push_string(
        state: *mut c_void,
        value: NativeSlice,
        error: *mut NativeString,
    ) -> i32;
    fn instar_state_call(
        state: *mut c_void,
        arguments: i32,
        results: i32,
        error_function: i32,
        lua_status: *mut i32,
        error: *mut NativeString,
    ) -> i32;
    fn instar_state_is_nil(
        state: *mut c_void,
        index: i32,
        result: *mut u8,
        error: *mut NativeString,
    ) -> i32;
    fn instar_state_to_string(
        state: *mut c_void,
        index: i32,
        present: *mut u8,
        output: *mut NativeString,
        error: *mut NativeString,
    ) -> i32;
}

fn native_bytes<'value>(value: NativeSlice) -> io::Result<&'value [u8]> {
    if value.data.is_null() {
        return if value.length == 0 {
            Ok(&[])
        } else {
            Err(io::Error::other("invalid native string"))
        };
    }

    Ok(unsafe { std::slice::from_raw_parts(value.data, value.length) })
}

fn native_text(value: NativeSlice) -> io::Result<String> {
    std::str::from_utf8(native_bytes(value)?)
        .map(str::to_owned)
        .map_err(io::Error::other)
}

fn owned_text(value: NativeString) -> io::Result<String> {
    let result = if value.data.is_null() {
        if value.length == 0 {
            Ok(String::new())
        } else {
            Err(io::Error::other("invalid owned native string"))
        }
    } else {
        String::from_utf8(unsafe { std::slice::from_raw_parts(value.data, value.length) }.to_vec())
            .map_err(io::Error::other)
    };

    unsafe { instar_string_destroy(value) };

    result
}

fn status(
    code: i32,
    error: NativeString,
    callback_error: &mut Option<io::Error>,
) -> io::Result<()> {
    let message = owned_text(error)?;

    if let Some(error) = callback_error.take() {
        return Err(error);
    }

    match code {
        0 => Ok(()),
        2 if message.is_empty() => Err(io::Error::other("native callback failed")),
        _ if message.is_empty() => Err(io::Error::other("native operation failed")),
        _ => Err(io::Error::other(message)),
    }
}

struct Configuration {
    handle: usize,
}

impl Configuration {
    fn new() -> io::Result<Self> {
        let mut error = NativeString::default();

        let handle = unsafe { instar_configuration_new(&raw mut error) };

        if handle.is_null() {
            let message = owned_text(error)?;

            return Err(io::Error::other(if message.is_empty() {
                "cannot create native configuration".into()
            } else {
                message
            }));
        }

        Ok(Self {
            handle: handle as usize,
        })
    }

    fn parse_json(&mut self, source: &[u8]) -> io::Result<()> {
        self.parse(source, false)
    }

    fn extract_luau(&mut self, source: &[u8]) -> io::Result<()> {
        self.parse(source, true)
    }

    fn parse(&mut self, source: &[u8], executable: bool) -> io::Result<()> {
        let options = NativeConfigurationOptions {
            has_alias_options: 1,
            overwrite_aliases: 1,
            ..NativeConfigurationOptions::default()
        };

        let mut error = NativeString::default();

        let code = unsafe {
            if executable {
                instar_configuration_extract_luau(
                    self.handle as *mut c_void,
                    NativeSlice::new(source),
                    &raw const options,
                    &raw mut error,
                )
            } else {
                instar_configuration_parse_json(
                    self.handle as *mut c_void,
                    NativeSlice::new(source),
                    &raw const options,
                    &raw mut error,
                )
            }
        };

        status(code, error, &mut None)
    }

    fn set_capture_comments(&mut self) -> io::Result<()> {
        let mut error = NativeString::default();

        let code = unsafe {
            instar_configuration_set_capture_comments(self.handle as *mut c_void, 1, &raw mut error)
        };

        status(code, error, &mut None)
    }

    fn set_mode(&mut self, mode: Mode) -> io::Result<()> {
        let mode = match mode {
            Mode::Nocheck => 0,
            Mode::Nonstrict => 1,
            Mode::Strict => 2,
        };

        let mut error = NativeString::default();

        let code = unsafe {
            instar_configuration_set_mode(self.handle as *mut c_void, mode, &raw mut error)
        };

        status(code, error, &mut None)
    }

    fn pointer(&self) -> *const c_void {
        self.handle as *const c_void
    }
}

impl Drop for Configuration {
    fn drop(&mut self) {
        unsafe { instar_configuration_destroy(self.handle as *mut c_void) };
    }
}

struct AliasContext {
    values: BTreeMap<String, String>,
    error: Option<io::Error>,
}

extern "C" fn alias_callback(
    context: *mut c_void,
    key: NativeSlice,
    original_case: NativeSlice,
    value: NativeSlice,
) -> u8 {
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let context = unsafe { &mut *context.cast::<AliasContext>() };

        let key = native_text(key)?;
        let original_case = native_text(original_case)?;

        context.values.insert(
            if original_case.is_empty() {
                key
            } else {
                original_case
            },
            native_text(value)?,
        );

        Ok::<_, io::Error>(())
    }));

    match result {
        Ok(Ok(())) => 1,

        Ok(Err(error)) => {
            unsafe { &mut *context.cast::<AliasContext>() }.error = Some(error);

            0
        }

        Err(_) => {
            unsafe { &mut *context.cast::<AliasContext>() }.error =
                Some(io::Error::other("alias callback panicked"));

            0
        }
    }
}

pub(crate) fn aliases(source: &[u8], executable: bool) -> io::Result<BTreeMap<String, String>> {
    let mut configuration = Configuration::new()?;

    if executable {
        configuration.extract_luau(source)?;
    } else {
        configuration.parse_json(source)?;
    }

    let mut context = AliasContext {
        values: BTreeMap::new(),
        error: None,
    };

    let mut error = NativeString::default();

    let code = unsafe {
        instar_configuration_aliases(
            configuration.pointer(),
            alias_callback,
            ptr::from_mut(&mut context).cast(),
            &raw mut error,
        )
    };

    status(code, error, &mut context.error)?;

    Ok(context.values)
}

struct State {
    handle: *mut c_void,
}

impl State {
    fn new() -> io::Result<Self> {
        let mut error = NativeString::default();

        let handle = unsafe { instar_state_new(&raw mut error) };

        if handle.is_null() {
            let message = owned_text(error)?;

            return Err(io::Error::other(if message.is_empty() {
                "cannot create Luau state".into()
            } else {
                message
            }));
        }

        Ok(Self { handle })
    }

    fn open_libraries(&self) -> io::Result<()> {
        let mut error = NativeString::default();

        let code = unsafe { instar_state_openlibs(self.handle, &raw mut error) };

        status(code, error, &mut None)
    }

    fn get_global(&self, name: &[u8]) -> io::Result<()> {
        let mut error = NativeString::default();

        let code =
            unsafe { instar_state_get_global(self.handle, NativeSlice::new(name), &raw mut error) };

        status(code, error, &mut None)
    }

    fn get_field(&self, index: i32, name: &[u8]) -> io::Result<()> {
        let mut error = NativeString::default();

        let code = unsafe {
            instar_state_get_field(self.handle, index, NativeSlice::new(name), &raw mut error)
        };

        status(code, error, &mut None)
    }

    fn push_string(&self, value: &[u8]) -> io::Result<()> {
        let mut error = NativeString::default();

        let code = unsafe {
            instar_state_push_string(self.handle, NativeSlice::new(value), &raw mut error)
        };

        status(code, error, &mut None)
    }

    fn call(&self, arguments: i32, results: i32, error_function: i32) -> io::Result<()> {
        let mut lua_status = 0;
        let mut error = NativeString::default();

        let adapter_status = unsafe {
            instar_state_call(
                self.handle,
                arguments,
                results,
                error_function,
                &raw mut lua_status,
                &raw mut error,
            )
        };

        let message = owned_text(error)?;

        if adapter_status != 0 {
            return Err(io::Error::other(if message.is_empty() {
                "Luau call adapter failed".into()
            } else {
                message
            }));
        }

        if lua_status == 0 {
            return Ok(());
        }

        if !message.is_empty() {
            return Err(io::Error::other(message));
        }

        let mut present = 0;
        let mut output = NativeString::default();
        let mut text_error = NativeString::default();

        let text_code = unsafe {
            instar_state_to_string(
                self.handle,
                -1,
                &raw mut present,
                &raw mut output,
                &raw mut text_error,
            )
        };

        status(text_code, text_error, &mut None)?;
        let message = owned_text(output)?;

        Err(io::Error::other(if present != 0 && !message.is_empty() {
            message
        } else {
            format!("Luau call failed with status {lua_status}")
        }))
    }

    fn is_nil(&self, index: i32) -> io::Result<bool> {
        let mut result = 0;
        let mut error = NativeString::default();

        let code =
            unsafe { instar_state_is_nil(self.handle, index, &raw mut result, &raw mut error) };

        status(code, error, &mut None)?;

        Ok(result != 0)
    }
}

impl Drop for State {
    fn drop(&mut self) {
        unsafe { instar_state_destroy(self.handle) };
    }
}

pub(crate) fn matches(pattern: &str, subject: &str) -> io::Result<bool> {
    let state = State::new()?;
    state.open_libraries()?;
    state.get_global(b"string")?;
    state.get_field(-1, b"find")?;
    state.push_string(subject.as_bytes())?;
    state.push_string(pattern.as_bytes())?;
    state.call(2, 1, 0)?;

    Ok(!state.is_nil(-1)?)
}

struct CallbackContext<'resolver, 'store> {
    resolver: &'resolver mut Resolver<'store>,
    options: &'resolver Options,
    environment: Option<Arc<Environment>>,
    configurations: BTreeMap<PathBuf, Box<Configuration>>,
    source_buffer: Option<Arc<Source>>,
    output_buffer: Vec<u8>,
    report: Report,
    syntax: Option<String>,
    globals: BTreeSet<String>,
    modules: Vec<PathBuf>,
    timeout_hits: BTreeSet<PathBuf>,
    identities: BTreeMap<PathBuf, PathBuf>,
    error: Option<io::Error>,
}

impl CallbackContext<'_, '_> {
    fn physical(&self, path: &Path) -> PathBuf {
        self.environment.as_ref().map_or_else(
            || path.to_owned(),
            |environment| environment.configuration(path),
        )
    }

    fn configuration(&mut self, path: &Path) -> io::Result<*const c_void> {
        let path = self.physical(path);

        let key = path
            .parent()
            .ok_or_else(|| io::Error::other("module has no parent directory"))?
            .to_owned();

        if !self.configurations.contains_key(&key) {
            let configuration_source = matches!(
                path.file_name().and_then(|name| name.to_str()),
                Some(".config.luau" | "config.luau")
            );

            let sources = self
                .resolver
                .discovery
                .configurations(&path)?
                .iter()
                .filter(|source| !configuration_source || source.path != path)
                .cloned()
                .collect::<Vec<_>>();

            let mut configuration = Box::new(Configuration::new()?);

            for source in sources {
                let executable = matches!(
                    source.path.file_name().and_then(|name| name.to_str()),
                    Some(".config.luau" | "config.luau")
                );

                let result = if executable {
                    configuration.extract_luau(&source.bytes)
                } else {
                    configuration.parse_json(&source.bytes)
                };

                if let Err(error) = result {
                    let message = error.to_string();

                    let message = if message.starts_with("Unknown lint ") {
                        message
                    } else {
                        format!("TypeError: {message}")
                    };

                    self.report.diagnostics.push(Diagnostic {
                        path: source.path,
                        line: 0,
                        column: 0,
                        end_line: 0,
                        end_column: 0,
                        message,
                        is_error: true,
                        related: Vec::new(),
                    });
                }
            }

            if let Some(mode) = self.options.mode {
                configuration.set_mode(mode)?;
            }

            configuration.set_capture_comments()?;
            self.configurations.insert(key.clone(), configuration);
        }

        Ok(self
            .configurations
            .get(&key)
            .expect("configuration inserted")
            .pointer())
    }

    fn source(&mut self, path: &Path) -> io::Result<(Option<Arc<Source>>, u32)> {
        if self
            .environment
            .as_ref()
            .is_some_and(|environment| !environment.readable(path))
        {
            return Ok((None, 0));
        }

        let physical = self
            .environment
            .as_ref()
            .map_or_else(|| path.to_owned(), |environment| environment.source(path));

        let source = self.resolver.load(&physical)?;

        let source_type = self
            .environment
            .as_ref()
            .and_then(|environment| {
                environment
                    .node(path)
                    .or_else(|| environment.node(&physical))
            })
            .map(|index| {
                u32::from(matches!(
                    self.environment.as_ref().expect("environment").nodes[index]
                        .class_name
                        .as_str(),
                    "Script" | "LocalScript"
                )) + 1
            })
            .unwrap_or_else(|| {
                let name = physical.file_stem().and_then(|name| name.to_str());

                u32::from(name.is_some_and(|name| {
                    name.ends_with(".server") || name.ends_with(".client") || name.ends_with(".plugin")
                })) + 1
            });

        Ok((Some(source), source_type))
    }

    fn resolve(
        &mut self,
        from: &Path,
        optional: bool,
        expression: &Value,
        _limits: NativeTypeCheckLimits,
    ) -> io::Result<Option<(PathBuf, bool)>> {
        let instance = string_expression(expression).is_none();

        let resolved = if let Some(specifier) = string_expression(expression) {
            if let Some(environment) = self.environment.clone() {
                environment.require(self.resolver, from, specifier)?
            } else {
                self.resolver.resolve(from, specifier)?
            }
        } else if let Some(environment) = self.environment.as_ref() {
            let origin = self
                .identities
                .get(from)
                .cloned()
                .unwrap_or_else(|| environment.source(from));

            instance_expression(environment, &origin, expression).map(|index| environment.identity(index))
        } else {
            None
        };

        let resolved = resolved.map(|path| {
            self.environment
                .as_ref()
                .map_or(path.clone(), |environment| environment.source(&path))
        });

        if instance
            && let Some(environment) = &self.environment
            && let Some(resolved) = &resolved
        {
            let origin = self
                .identities
                .get(from)
                .cloned()
                .unwrap_or_else(|| environment.source(from));

            self.identities.insert(resolved.clone(), origin);
        }

        Ok(resolved.map(|path| (path, optional)))
    }
}

fn callback_context<'value>(context: *mut c_void) -> &'value mut CallbackContext<'value, 'value> {
    unsafe { &mut *context.cast::<CallbackContext<'value, 'value>>() }
}

extern "C" fn source_callback(
    context: *mut c_void,
    name: NativeSlice,
    output: *mut NativeSlice,
    source_type: *mut u32,
) -> u8 {
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let path = PathBuf::from(native_text(name)?);
        let context = callback_context(context);
        let (source, kind) = context.source(&path)?;
        context.source_buffer = source;

        unsafe {
            *source_type = kind;

            *output = context
                .source_buffer
                .as_ref()
                .map_or_else(NativeSlice::default, |source| {
                    NativeSlice::new(source.bytes())
                });
        }

        Ok::<_, io::Error>(())
    }));

    callback_status(context, result, "source callback panicked")
}

extern "C" fn configuration_callback(
    context: *mut c_void,
    name: NativeSlice,
    output: *mut *const c_void,
) -> u8 {
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let path = PathBuf::from(native_text(name)?);
        let configuration = callback_context(context).configuration(&path)?;

        unsafe { *output = configuration };

        Ok::<_, io::Error>(())
    }));

    callback_status(context, result, "configuration callback panicked")
}

extern "C" fn resolve_callback(
    context: *mut c_void,
    from: NativeSlice,
    optional: u8,
    expression: NativeSlice,
    limits: NativeTypeCheckLimits,
    output: *mut NativeSlice,
    output_optional: *mut u8,
    present: *mut u8,
) -> u8 {
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let from = PathBuf::from(native_text(from)?);
        let expression: Value = serde_json::from_slice(native_bytes(expression)?)?;
        let context = callback_context(context);
        let resolved = context.resolve(&from, optional != 0, &expression, limits)?;

        context.output_buffer = resolved
            .as_ref()
            .map(|(path, _)| {
                path.to_str()
                    .ok_or_else(|| io::Error::other("module identity requires UTF-8"))
                    .map(str::as_bytes)
            })
            .transpose()?
            .unwrap_or_default()
            .to_vec();

        unsafe {
            *present = u8::from(resolved.is_some());

            *output_optional = resolved
                .as_ref()
                .map_or(0, |(_, optional)| u8::from(*optional));

            *output = NativeSlice::new(&context.output_buffer);
        }

        Ok::<_, io::Error>(())
    }));

    callback_status(context, result, "resolution callback panicked")
}

extern "C" fn diagnostic_callback(
    context: *mut c_void,
    path: NativeSlice,
    line: u32,
    column: u32,
    end_line: u32,
    end_column: u32,
    native_kind: u32,
    _native_code: i32,
    _native_variant: u32,
    is_error: u8,
    message: NativeSlice,
    related_path: NativeSlice,
    related_line: u32,
    related_column: u32,
    related_end_line: u32,
    related_end_column: u32,
    related_message: NativeSlice,
) -> u8 {
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let category = match native_kind {
            NATIVE_TYPE_ERROR => "TypeError",
            NATIVE_PARSE_ERROR => "SyntaxError",
            NATIVE_LINT_WARNING => "LintWarning",
            _ => return Err(io::Error::other("unknown native diagnostic kind")),
        };

        let path = PathBuf::from(native_text(path)?);
        let related_path = native_text(related_path)?;
        let related_message = native_text(related_message)?;

        let related = if related_path.is_empty() || related_message.is_empty() {
            Vec::new()
        } else {
            vec![RelatedDiagnostic {
                path: PathBuf::from(related_path),
                range: [related_line, related_column, related_end_line, related_end_column],
                message: related_message,
            }]
        };

        callback_context(context)
            .report
            .diagnostics
            .push(Diagnostic {
                path,
                line,
                column,
                end_line,
                end_column,
                message: format!("{category}: {}", native_text(message)?),
                is_error: is_error != 0,
                related,
            });

        Ok::<_, io::Error>(())
    }));

    callback_status(context, result, "diagnostic callback panicked")
}

fn callback_status(
    context: *mut c_void,
    result: Result<Result<(), io::Error>, Box<dyn std::any::Any + Send>>,
    panic_message: &'static str,
) -> u8 {
    match result {
        Ok(Ok(())) => 1,

        Ok(Err(error)) => {
            callback_context(context).error = Some(error);

            0
        }

        Err(_) => {
            callback_context(context).error = Some(io::Error::other(panic_message));

            0
        }
    }
}

extern "C" fn global_callback(context: *mut c_void, name: NativeSlice) -> u8 {
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        callback_context(context).globals.insert(native_text(name)?);

        Ok::<_, io::Error>(())
    }));

    callback_status(context, result, "global callback panicked")
}

extern "C" fn module_callback(context: *mut c_void, name: NativeSlice) -> u8 {
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        callback_context(context)
            .modules
            .push(PathBuf::from(native_text(name)?));

        Ok::<_, io::Error>(())
    }));

    callback_status(context, result, "module callback panicked")
}

extern "C" fn timeout_callback(context: *mut c_void, name: NativeSlice) -> u8 {
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        callback_context(context)
            .timeout_hits
            .insert(PathBuf::from(native_text(name)?));

        Ok::<_, io::Error>(())
    }));

    callback_status(context, result, "timeout callback panicked")
}

const CALLBACKS: NativeCallbacks = NativeCallbacks {
    source: source_callback,
    configuration: configuration_callback,
    resolve: resolve_callback,
    diagnostic: diagnostic_callback,
};

struct Checker {
    handle: usize,
}

impl Checker {
    fn new(context: &mut CallbackContext<'_, '_>, options: &Options) -> io::Result<Self> {
        let frontend_options = NativeFrontendOptions {
            old_solver: u8::from(options.old_solver),
            retain_full_type_graphs: u8::from(options.annotations || options.retain_full_type_graphs),
            run_lint_checks: 1,
            ..NativeFrontendOptions::default()
        };

        let mut error = NativeString::default();

        let handle = unsafe {
            instar_checker_new(
                &CALLBACKS,
                ptr::from_mut(context).cast(),
                &raw const frontend_options,
                &raw mut error,
            )
        };

        if handle.is_null() {
            let message = owned_text(error)?;

            return Err(io::Error::other(if message.is_empty() {
                "cannot create native checker".into()
            } else {
                message
            }));
        }

        Ok(Self {
            handle: handle as usize,
        })
    }

    fn register_builtins(&self, context: &mut CallbackContext<'_, '_>) -> io::Result<()> {
        let mut error = NativeString::default();

        let code = unsafe {
            instar_checker_register_builtins(self.handle as *mut c_void, 0, &raw mut error)
        };

        status(code, error, &mut context.error)
    }

    fn call(
        &self,
        context: &mut CallbackContext<'_, '_>,
        operation: unsafe extern "C" fn(*mut c_void, *mut NativeString) -> i32,
    ) -> io::Result<()> {
        let mut error = NativeString::default();

        let code = unsafe { operation(self.handle as *mut c_void, &raw mut error) };

        status(code, error, &mut context.error)
    }

    fn call_named(
        &self,
        context: &mut CallbackContext<'_, '_>,
        name: &Path,
        operation: unsafe extern "C" fn(*mut c_void, NativeSlice, *mut NativeString) -> i32,
    ) -> io::Result<()> {
        let name = name
            .to_str()
            .ok_or_else(|| io::Error::other("module identity requires UTF-8"))?;

        let mut error = NativeString::default();

        let code = unsafe {
            operation(
                self.handle as *mut c_void,
                NativeSlice::new(name.as_bytes()),
                &raw mut error,
            )
        };

        status(code, error, &mut context.error)
    }

    fn load_definition(
        &self,
        context: &mut CallbackContext<'_, '_>,
        source: &[u8],
        package: &str,
    ) -> io::Result<()> {
        let options = NativeDefinitionOptions {
            capture_comments: 1,
            ..NativeDefinitionOptions::default()
        };

        let mut error = NativeString::default();

        let code = unsafe {
            instar_checker_load_definition(
                self.handle as *mut c_void,
                NativeSlice::new(source),
                NativeSlice::new(package.as_bytes()),
                &raw const options,
                &raw mut error,
            )
        };

        status(code, error, &mut context.error)
    }

    fn register_roblox_magic(
        &self,
        context: &mut CallbackContext<'_, '_>,
        environment: &Environment,
    ) -> io::Result<()> {
        let classes = environment
            .classes
            .iter()
            .map(|value| {
                let (name, flags) = value
                    .split_once('\0')
                    .ok_or_else(|| io::Error::other("invalid Roblox class metadata"))?;

                let flags = flags.as_bytes();

                Ok(NativeRobloxClass {
                    name: NativeSlice::new(name.as_bytes()),
                    service: u8::from(
                        *flags
                            .first()
                            .ok_or_else(|| io::Error::other("invalid Roblox class metadata"))?
                            == b'1',
                    ),
                    creatable: u8::from(
                        *flags
                            .get(1)
                            .ok_or_else(|| io::Error::other("invalid Roblox class metadata"))?
                            == b'1',
                    ),
                })
            })
            .collect::<io::Result<Vec<_>>>()?;

        let nodes = environment
            .nodes
            .iter()
            .map(|node| NativeRobloxNode {
                name: NativeSlice::new(node.name.as_bytes()),
                class_name: NativeSlice::new(node.class_name.as_bytes()),
            })
            .collect::<Vec<_>>();

        let mut error = NativeString::default();

        let code = unsafe {
            instar_checker_register_roblox_magic(
                self.handle as *mut c_void,
                classes.as_ptr(),
                classes.len(),
                nodes.as_ptr(),
                nodes.len(),
                &raw mut error,
            )
        };

        status(code, error, &mut context.error)
    }

    fn freeze(&self, context: &mut CallbackContext<'_, '_>) -> io::Result<()> {
        self.call(context, instar_checker_freeze)
    }

    fn parse(&self, context: &mut CallbackContext<'_, '_>, path: &Path) -> io::Result<()> {
        self.call_named(context, path, instar_checker_parse)
    }

    fn parse_diagnostics(
        &self,
        context: &mut CallbackContext<'_, '_>,
        path: &Path,
    ) -> io::Result<()> {
        self.call_named(context, path, instar_checker_parse_diagnostics)
    }

    fn syntax_tree(
        &self,
        context: &mut CallbackContext<'_, '_>,
        path: &Path,
    ) -> io::Result<String> {
        let name = path
            .to_str()
            .ok_or_else(|| io::Error::other("module identity requires UTF-8"))?;

        let mut output = NativeString::default();
        let mut error = NativeString::default();

        let code = unsafe {
            instar_checker_ast(
                self.handle as *mut c_void,
                NativeSlice::new(name.as_bytes()),
                &raw mut output,
                &raw mut error,
            )
        };

        status(code, error, &mut context.error)?;

        owned_text(output)
    }

    fn syntax(&self, context: &mut CallbackContext<'_, '_>, path: &Path) -> io::Result<()> {
        context.syntax = Some(self.syntax_tree(context, path)?);

        Ok(())
    }

    fn globals(&self, context: &mut CallbackContext<'_, '_>) -> io::Result<()> {
        let mut error = NativeString::default();

        let code = unsafe {
            instar_checker_globals(
                self.handle as *mut c_void,
                global_callback,
                ptr::from_mut(context).cast(),
                &raw mut error,
            )
        };

        status(code, error, &mut context.error)
    }

    fn check(&self, context: &mut CallbackContext<'_, '_>, path: &Path) -> io::Result<()> {
        self.call_named(context, path, instar_checker_check)
    }

    fn result(&self, context: &mut CallbackContext<'_, '_>, path: &Path) -> io::Result<()> {
        let name = path
            .to_str()
            .ok_or_else(|| io::Error::other("module identity requires UTF-8"))?;

        let mut error = NativeString::default();

        let code = unsafe {
            instar_checker_result(
                self.handle as *mut c_void,
                NativeSlice::new(name.as_bytes()),
                0,
                0,
                &raw mut error,
            )
        };

        status(code, error, &mut context.error)
    }

    fn editor(
        &self,
        context: &mut CallbackContext<'_, '_>,
        path: &Path,
        line: u32,
        column: u32,
        operation: &str,
    ) -> io::Result<Vec<EditorEntry>> {
        let name = path
            .to_str()
            .ok_or_else(|| io::Error::other("module identity requires UTF-8"))?;

        let mut output = NativeString::default();
        let mut error = NativeString::default();

        let code = unsafe {
            instar_checker_editor(
                self.handle as *mut c_void,
                NativeSlice::new(name.as_bytes()),
                line,
                column,
                NativeSlice::new(operation.as_bytes()),
                &raw mut output,
                &raw mut error,
            )
        };

        status(code, error, &mut context.error)?;
        let value = owned_text(output)?;

        serde_json::from_str(&value).map_err(io::Error::other)
    }

    fn timeouts(&self, context: &mut CallbackContext<'_, '_>) -> io::Result<()> {
        let mut error = NativeString::default();

        let code = unsafe {
            instar_checker_timeouts(
                self.handle as *mut c_void,
                timeout_callback,
                ptr::from_mut(context).cast(),
                &raw mut error,
            )
        };

        status(code, error, &mut context.error)
    }

    fn modules(&self, context: &mut CallbackContext<'_, '_>) -> io::Result<()> {
        let mut error = NativeString::default();

        let code = unsafe {
            instar_checker_modules(
                self.handle as *mut c_void,
                module_callback,
                ptr::from_mut(context).cast(),
                &raw mut error,
            )
        };

        status(code, error, &mut context.error)
    }

    fn annotation(&self, context: &mut CallbackContext<'_, '_>, path: &Path) -> io::Result<()> {
        if context.report.has_errors() {
            let source = context.resolver.load(path)?;

            context.report.annotations.push(Annotation {
                path: path.to_owned(),
                bytes: source.bytes().to_vec(),
            });

            return Ok(());
        }

        self.call_named(context, path, instar_checker_attach_type_data)?;
        let syntax_tree = self.syntax_tree(context, path)?;
        let source = context.resolver.load(path)?;

        context.report.annotations.push(Annotation {
            path: path.to_owned(),
            bytes: annotate_source(source.bytes(), &syntax_tree)?,
        });

        Ok(())
    }
}

fn annotate_source(source: &[u8], syntax_tree: &str) -> io::Result<Vec<u8>> {
    let tree: Value = serde_json::from_str(syntax_tree).map_err(io::Error::other)?;
    let source_tree = vermis::parse(source.into());
    let line_starts = line_starts(source);
    let mut insertions = BTreeMap::new();

    collect_annotation_insertions(&tree, source, &source_tree, &line_starts, &mut insertions);

    let mut output = source.to_vec();

    for (offset, value) in insertions.into_iter().rev() {
        output.splice(offset..offset, value.bytes());
    }

    Ok(output)
}

fn line_starts(source: &[u8]) -> Vec<usize> {
    let mut result = vec![0];

    for (index, byte) in source.iter().enumerate() {
        if *byte == b'\n' {
            result.push(index + 1);
        }
    }

    result
}

fn source_location(value: &Value) -> Option<((u32, u32), (u32, u32))> {
    let value = value.as_str()?;
    let (begin, end) = value.split_once(" - ")?;

    Some((position(begin)?, position(end)?))
}

fn position(value: &str) -> Option<(u32, u32)> {
    let (line, column) = value.split_once(',')?;

    Some((line.parse().ok()?, column.parse().ok()?))
}

fn offset(source: &[u8], line_starts: &[usize], position: (u32, u32)) -> Option<usize> {
    let line = usize::try_from(position.0).ok()?;
    let column = usize::try_from(position.1).ok()?;
    let start = *line_starts.get(line)?;
    let mut end = line_starts.get(line + 1).copied().unwrap_or(source.len());

    if end > start && source[end - 1] == b'\n' {
        end -= 1;
    }

    if end > start && source[end - 1] == b'\r' {
        end -= 1;
    }

    start.checked_add(column).filter(|value| *value <= end)
}

fn collect_annotation_insertions(
    value: &Value,
    source: &[u8],
    source_tree: &vermis::Tree<'_>,
    line_starts: &[usize],
    insertions: &mut BTreeMap<usize, String>,
) {
    if let Some(object) = value.as_object() {
        match object.get("type").and_then(Value::as_str) {
            Some("AstLocal") => {
                collect_local_annotation(value, source, source_tree, line_starts, insertions)
            }

            Some("AstStatLocalFunction") => {
                if let Some(name) = object.get("name") {
                    collect_function_generics(name, source, source_tree, line_starts, insertions);
                }
            }

            Some("AstExprFunction") => {
                if let Some(return_annotation) = object.get("returnAnnotation") {
                    collect_function_return(
                        value,
                        return_annotation,
                        source,
                        source_tree,
                        line_starts,
                        insertions,
                    );
                }
            }

            _ => {}
        }

        for child in object.values() {
            collect_annotation_insertions(child, source, source_tree, line_starts, insertions);
        }
    } else if let Some(values) = value.as_array() {
        for value in values {
            collect_annotation_insertions(value, source, source_tree, line_starts, insertions);
        }
    }
}

fn collect_local_annotation(
    value: &Value,
    source: &[u8],
    source_tree: &vermis::Tree<'_>,
    line_starts: &[usize],
    insertions: &mut BTreeMap<usize, String>,
) {
    let Some(object) = value.as_object() else {
        return;
    };

    let Some(annotation) = object.get("luauType").filter(|value| !value.is_null()) else {
        return;
    };

    let Some(location) = object.get("location").and_then(source_location) else {
        return;
    };

    let Some(start) = offset(source, line_starts, location.0) else {
        return;
    };

    let Some(end) = offset(source, line_starts, location.1) else {
        return;
    };

    if source_function_name(source_tree, start, end).is_some() {
        return;
    }

    let (end, annotated) =
        source_binding(source_tree, start, end).unwrap_or((end, has_colon(source, end)));

    if annotated {
        return;
    }

    let Some(annotation) = type_text(annotation) else {
        return;
    };

    insertions
        .entry(end)
        .or_insert_with(|| format!(": {annotation}"));
}

fn collect_function_generics(
    value: &Value,
    source: &[u8],
    source_tree: &vermis::Tree<'_>,
    line_starts: &[usize],
    insertions: &mut BTreeMap<usize, String>,
) {
    let Some(object) = value.as_object() else {
        return;
    };

    let Some(function_type) = object.get("luauType") else {
        return;
    };

    let names = generic_names(function_type);

    if names.is_empty() {
        return;
    }

    let Some(location) = object.get("location").and_then(source_location) else {
        return;
    };

    let Some(start) = offset(source, line_starts, location.0) else {
        return;
    };

    let Some(end) = offset(source, line_starts, location.1) else {
        return;
    };

    let (end, generics) = source_function_name(source_tree, start, end)
        .unwrap_or((end, has_generic_list(source, end)));

    if generics {
        return;
    }

    insertions
        .entry(end)
        .or_insert_with(|| format!("<{}>", names.join(", ")));
}

fn collect_function_return(
    value: &Value,
    return_annotation: &Value,
    source: &[u8],
    source_tree: &vermis::Tree<'_>,
    line_starts: &[usize],
    insertions: &mut BTreeMap<usize, String>,
) {
    let Some(object) = value.as_object() else {
        return;
    };

    let Some(location) = object.get("location").and_then(source_location) else {
        return;
    };

    let Some(start) = offset(source, line_starts, location.0) else {
        return;
    };

    let Some(end) = offset(source, line_starts, location.1) else {
        return;
    };

    let Some((close, returns)) = source_function_signature(source_tree, start, end) else {
        return;
    };

    if returns {
        return;
    }

    let Some(annotation) = type_pack_text(return_annotation, true) else {
        return;
    };

    insertions
        .entry(close)
        .or_insert_with(|| format!(": {annotation}"));
}

fn source_binding(tree: &vermis::Tree<'_>, start: usize, end: usize) -> Option<(usize, bool)> {
    for index in 0..tree.nodes.len() {
        let Some(view) = tree.view(index) else {
            continue;
        };

        if view.kind() != vermis::Kind::Binding {
            continue;
        }

        let Some(vermis::Parts::Binding { name, annotation }) = view.parts() else {
            continue;
        };

        if name.span().start == start && name.span().end == end {
            return Some((name.span().end, annotation.is_some()));
        }
    }

    None
}

fn source_function_name(
    tree: &vermis::Tree<'_>,
    start: usize,
    end: usize,
) -> Option<(usize, bool)> {
    for index in 0..tree.nodes.len() {
        let Some(view) = tree.view(index) else {
            continue;
        };

        if !matches!(
            view.kind(),
            vermis::Kind::Function | vermis::Kind::LocalFunction
        ) {
            continue;
        }

        let Some(vermis::Parts::Function { name, generics, .. }) = view.parts() else {
            continue;
        };

        let Some(name) = name else {
            continue;
        };

        if name.span().start == start && name.span().end == end {
            return Some((name.span().end, generics.is_some()));
        }
    }

    None
}

fn source_function_signature(
    tree: &vermis::Tree<'_>,
    start: usize,
    end: usize,
) -> Option<(usize, bool)> {
    let mut result = None;

    for index in 0..tree.nodes.len() {
        let node = &tree.nodes[index];

        if !matches!(
            node.kind,
            vermis::Kind::Function | vermis::Kind::LocalFunction
        ) || node.span.start > start
            || node.span.end < end
        {
            continue;
        }

        let Some(view) = tree.view(index) else {
            continue;
        };

        let Some(vermis::Parts::Function {
            parameters,
            returns,
            ..
        }) = view.parts()
        else {
            continue;
        };

        if result.is_none_or(|(length, _)| node.span.len() < length) {
            result = Some((node.span.len(), (parameters.span().end, returns.is_some())));
        }
    }

    result.map(|(_, signature)| signature)
}

fn has_colon(source: &[u8], mut offset: usize) -> bool {
    while matches!(source.get(offset), Some(b' ' | b'\t')) {
        offset += 1;
    }

    source.get(offset) == Some(&b':')
}

fn has_generic_list(source: &[u8], mut offset: usize) -> bool {
    while matches!(source.get(offset), Some(b' ' | b'\t')) {
        offset += 1;
    }

    source.get(offset) == Some(&b'<')
}

fn type_text(value: &Value) -> Option<String> {
    let object = value.as_object()?;

    match object.get("type").and_then(Value::as_str)? {
        "AstTypeReference" => {
            let name = object.get("name")?.as_str()?;

            let mut result = object
                .get("prefix")
                .and_then(Value::as_str)
                .map_or_else(String::new, |prefix| format!("{prefix}."));

            result.push_str(name);

            if let Some(parameters) = object.get("parameters").and_then(Value::as_array)
                && !parameters.is_empty()
            {
                result.push('<');

                result.push_str(
                    &parameters
                        .iter()
                        .filter_map(type_or_pack_text)
                        .collect::<Vec<_>>()
                        .join(", "),
                );

                result.push('>');
            }

            Some(result)
        }

        "AstTypeTable" => {
            let mut fields = object
                .get("props")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|property| {
                    let property = property.as_object()?;
                    let name = property_name(property.get("name")?.as_str()?);
                    let value = type_text(property.get("propType")?)?;

                    Some(format!("{name}: {value}"))
                })
                .collect::<Vec<_>>();

            if let Some(indexer) = object.get("indexer").filter(|value| !value.is_null()) {
                let indexer = indexer.as_object()?;

                fields.push(format!(
                    "[{}]: {}",
                    type_text(indexer.get("indexType")?)?,
                    type_text(indexer.get("resultType")?)?
                ));
            }

            Some(format!("{{{}}}", fields.join(", ")))
        }

        "AstTypeFunction" => {
            let generics = generic_names(value);
            let arguments = type_list_text(object.get("argTypes")?, false)?;
            let returns = type_pack_text(object.get("returnTypes")?, true)?;

            let prefix = if generics.is_empty() {
                String::new()
            } else {
                format!("<{}>", generics.join(", "))
            };

            Some(format!("{prefix}({arguments}) -> {returns}"))
        }

        "AstTypeGroup" => Some(format!("({})", type_text(object.get("inner")?)?)),

        "AstTypeSingletonBool" => Some(if object.get("value")?.as_bool()? {
            "true".into()
        } else {
            "false".into()
        }),

        "AstTypeSingletonString" => serde_json::to_string(object.get("value")?).ok(),
        "AstTypeTypeof" => Some(format!("typeof({})", expression_text(object.get("expr")?)?)),
        "AstTypeOptional" => Some("?".into()),

        "AstTypeUnion" => {
            let values = object.get("types")?.as_array()?;
            let mut result = Vec::new();
            let mut optional = false;

            for value in values {
                let value = type_text(value)?;

                if value == "?" {
                    optional = true;
                } else {
                    result.push(value);
                }
            }

            let mut result = result.join(" | ");

            if optional {
                result.push('?');
            }

            Some(result)
        }

        "AstTypeIntersection" => Some(
            object
                .get("types")?
                .as_array()?
                .iter()
                .filter_map(type_text)
                .collect::<Vec<_>>()
                .join(" & "),
        ),

        "AstTypeError" => Some("unknown".into()),
        "AstTypePackExplicit" => type_pack_text(value, false),
        "AstTypePackVariadic" => Some(format!("...{}", type_text(object.get("variadicType")?)?)),
        "AstTypePackGeneric" => Some(format!("{}...", object.get("genericName")?.as_str()?)),
        _ => Some("unknown".into()),
    }
}

fn type_or_pack_text(value: &Value) -> Option<String> {
    let object = value.as_object()?;
    let kind = object.get("type").and_then(Value::as_str)?;

    if kind.starts_with("AstTypePack") {
        type_pack_text(value, false)
    } else {
        type_text(value)
    }
}

fn type_list_text(value: &Value, parenthesized: bool) -> Option<String> {
    let result = type_list_values(value)?.join(", ");

    if parenthesized {
        Some(format!("({result})"))
    } else {
        Some(result)
    }
}

fn type_list_values(value: &Value) -> Option<Vec<String>> {
    let object = value.as_object()?;

    let mut values = object
        .get("types")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(type_or_pack_text)
        .collect::<Vec<_>>();

    if let Some(tail) = object.get("tailType").filter(|value| !value.is_null()) {
        values.push(type_pack_text(tail, false)?);
    }

    Some(values)
}

fn type_pack_text(value: &Value, function_return: bool) -> Option<String> {
    let object = value.as_object()?;

    match object.get("type").and_then(Value::as_str)? {
        "AstTypePackExplicit" => {
            let values = type_list_values(object.get("typeList")?)?;

            if function_return {
                match values.as_slice() {
                    [] => Some("()".into()),
                    [value] => Some(value.clone()),
                    _ => Some(format!("({})", values.join(", "))),
                }
            } else {
                Some(format!("({})", values.join(", ")))
            }
        }

        "AstTypePackVariadic" => Some(format!("...{}", type_text(object.get("variadicType")?)?)),
        "AstTypePackGeneric" => Some(format!("{}...", object.get("genericName")?.as_str()?)),
        _ => type_text(value),
    }
}

fn generic_names(value: &Value) -> Vec<String> {
    let Some(object) = value.as_object() else {
        return Vec::new();
    };

    let count = object
        .get("generics")
        .and_then(Value::as_array)
        .map_or(0, Vec::len)
        + object
            .get("genericPacks")
            .and_then(Value::as_array)
            .map_or(0, Vec::len);

    if count == 0 {
        return Vec::new();
    }

    let mut names = Vec::new();
    collect_generic_references(value, &mut names);

    for key in ["generics", "genericPacks"] {
        if let Some(values) = object.get(key).and_then(Value::as_array) {
            for value in values {
                let Some(name) = value.get("name").and_then(Value::as_str) else {
                    continue;
                };

                if !name.is_empty() && !names.iter().any(|value| value == name) {
                    names.push(name.to_owned());
                }
            }
        }
    }

    for index in names.len()..count {
        names.push(generated_generic_name(index));
    }

    names.truncate(count);

    names
}

fn collect_generic_references(value: &Value, names: &mut Vec<String>) {
    let Some(object) = value.as_object() else {
        if let Some(values) = value.as_array() {
            for value in values {
                collect_generic_references(value, names);
            }
        }

        return;
    };

    if object.get("type").and_then(Value::as_str) == Some("AstTypeReference")
        && let Some(name) = object.get("name").and_then(Value::as_str)
        && !matches!(
            name,
            "any"
                | "boolean"
                | "buffer"
                | "never"
                | "nil"
                | "number"
                | "string"
                | "thread"
                | "unknown"
                | "vector"
        )
        && !names.iter().any(|value| value == name)
    {
        names.push(name.to_owned());
    }

    for value in object.values() {
        collect_generic_references(value, names);
    }
}

fn generated_generic_name(index: usize) -> String {
    const NAMES: [&str; 8] = ["T", "U", "V", "W", "X", "Y", "Z", "A"];

    NAMES
        .get(index)
        .map_or_else(|| format!("T{index}"), |name| (*name).into())
}

fn property_name(value: &str) -> String {
    if value.bytes().enumerate().all(|(index, byte)| {
        (index == 0 && (byte == b'_' || byte.is_ascii_alphabetic()))
            || (index > 0 && (byte == b'_' || byte.is_ascii_alphanumeric()))
    }) {
        value.into()
    } else {
        serde_json::to_string(value)
            .map_or_else(|_| "[unknown]".into(), |value| format!("[{value}]"))
    }
}

fn expression_text(value: &Value) -> Option<String> {
    let object = value.as_object()?;

    match object.get("type").and_then(Value::as_str)? {
        "AstExprGlobal" => object.get("global")?.as_str().map(str::to_owned),

        "AstExprLocal" => object
            .get("local")?
            .get("name")?
            .as_str()
            .map(str::to_owned),

        "AstExprIndexName" => Some(format!(
            "{}{}{}",
            expression_text(object.get("expr")?)?,
            if object.get("op").and_then(Value::as_str) == Some("Colon") {
                ":"
            } else {
                "."
            },
            object.get("index")?.as_str()?
        )),

        "AstExprIndexExpr" => Some(format!(
            "{}[{}]",
            expression_text(object.get("expr")?)?,
            expression_text(object.get("index")?)?
        )),

        "AstExprConstantString" => serde_json::to_string(object.get("value")?).ok(),
        _ => Some("unknown".into()),
    }
}

impl Drop for Checker {
    fn drop(&mut self) {
        unsafe { instar_checker_destroy(self.handle as *mut c_void) };
    }
}

#[derive(Default)]
pub(crate) struct Session;

impl Session {
    pub(crate) fn environment(
        &mut self,
        resolver: &mut Resolver<'_>,
        settings: Option<&crate::project::configuration::RobloxConfig>,
        update: bool,
    ) -> io::Result<Arc<Environment>> {
        settings.map_or_else(
            || Ok(Arc::new(Environment::default())),
            |settings| Environment::load(resolver, settings, update).map(Arc::new),
        )
    }

    pub(crate) fn refresh(&mut self) {}

    pub(crate) fn parse(
        &mut self,
        resolver: &mut Resolver<'_>,
        path: &Path,
        load_definitions: bool,
    ) -> io::Result<Report> {
        let (mut report, syntax, globals, environment, _) =
            run(resolver, path, &Options::default(), load_definitions, false, None, None)?;

        report.editor = syntax.map(|description| {
            EditorResult::Entry(Box::new(EditorEntry {
                description: Some(description),
                parameters: Some(globals.into_iter().collect()),
                ..empty_entry()
            }))
        });

        if let Some(EditorResult::Entry(entry)) = &report.editor
            && let Some(description) = &entry.description
            && let Ok(document) = crate::syntax::decode(
                &Source::new(path.to_owned(), resolver.load(path)?.bytes().to_vec())
                    .map_err(io::Error::other)?,
                description,
            )
        {
            report.links = dependency_entries(resolver, environment.as_deref(), path, &document)?;
        }

        Ok(report)
    }

    pub(crate) fn analyze(
        &mut self,
        resolver: &mut Resolver<'_>,
        modules: &[PathBuf],
        options: &Options,
    ) -> io::Result<Report> {
        self.analyze_with_definitions(resolver, modules, options, false)
    }

    pub(crate) fn analyze_including_definitions(
        &mut self,
        resolver: &mut Resolver<'_>,
        modules: &[PathBuf],
        options: &Options,
    ) -> io::Result<Report> {
        self.analyze_with_definitions(resolver, modules, options, true)
    }

    fn analyze_with_definitions(
        &mut self,
        resolver: &mut Resolver<'_>,
        modules: &[PathBuf],
        options: &Options,
        include_definitions: bool,
    ) -> io::Result<Report> {
        let mut report = Report::default();

        for path in modules {
            if !include_definitions && !resolver.is_open(path)? && resolver.is_definition(path)? {
                continue;
            }

            let (mut current, _, _, _, _) = run(resolver, path, options, true, true, None, None)?;
            report.diagnostics.append(&mut current.diagnostics);
            report.timeout_hits.append(&mut current.timeout_hits);
            report.annotations.append(&mut current.annotations);
            report.documentation.append(&mut current.documentation);
        }

        normalize_diagnostics(&mut report.diagnostics);

        Ok(report)
    }

    pub(crate) fn query(
        &mut self,
        resolver: &mut Resolver<'_>,
        modules: &[PathBuf],
        path: &Path,
        position: line_index::LineCol,
        operation: &str,
        collect_diagnostics: bool,
    ) -> io::Result<Report> {
        let mut report = self.analyze(resolver, modules, &Options::default())?;

        if !collect_diagnostics {
            report.diagnostics.clear();
        }

        let syntax_operation = matches!(
            operation,
            "index" | "symbols" | "calls" | "links" | "colors" | "imports" | "extract" | "folds" | "selection"
        );

        if syntax_operation {
            let parsed = self.parse(resolver, path, true)?;

            let syntax = match parsed.editor {
                Some(EditorResult::Entry(entry)) => entry.description,
                _ => None,
            }
            .ok_or_else(|| io::Error::other("native syntax unavailable"))?;

            let source = resolver.load(path)?;
            let document = crate::syntax::decode(&source, &syntax).map_err(io::Error::other)?;
            let links = parsed.links;

            let entries = match operation {
                "colors" => color_entries(path, &document),
                "imports" => import_entries(path, &document, links),
                "extract" => extract_entries(path, &document, position),
                "folds" => fold_entries(path, &document),
                "selection" => selection_entries(path, &document, position),
                _ => index_entries(path, &document, links),
            };

            report.editor = Some(EditorResult::Entries(match operation {
                "index" => entries,

                "symbols" => entries
                    .into_iter()
                    .filter(|entry| entry.caller.is_none() && entry.kind != Some(3))
                    .collect(),

                "calls" => entries
                    .into_iter()
                    .filter(|entry| entry.caller.is_some())
                    .collect(),

                "links" => entries
                    .into_iter()
                    .filter(|entry| entry.kind == Some(3) && entry.caller.is_none())
                    .collect(),

                "colors" | "imports" | "extract" | "folds" | "selection" => entries,

                _ => Vec::new(),
            }));
        } else {
            let options = Options {
                retain_full_type_graphs: true,
                ..Options::default()
            };

            let (_, _, _, _, entries) = run(
                resolver,
                path,
                &options,
                true,
                true,
                Some((position.line, position.col, operation)),
                (operation == "references").then_some(modules),
            )?;

            report.editor = Some(EditorResult::Entries(entries.unwrap_or_default()));
        }

        Ok(report)
    }
}

fn run(
    resolver: &mut Resolver<'_>,
    path: &Path,
    options: &Options,
    load_definitions: bool,
    check: bool,
    editor: Option<(u32, u32, &str)>,
    editor_modules: Option<&[PathBuf]>,
) -> io::Result<(
    Report,
    Option<String>,
    BTreeSet<String>,
    Option<Arc<Environment>>,
    Option<Vec<EditorEntry>>,
)> {
    let configuration_source = matches!(
        path.file_name().and_then(|name| name.to_str()),
        Some(".config.luau" | "config.luau")
    );

    let environment = if configuration_source {
        None
    } else {
        resolver
            .discovery
            .roblox(path)?
            .as_ref()
            .map(|settings| Environment::load(resolver, settings, options.update))
            .transpose()?
            .map(Arc::new)
    };

    let definitions = if load_definitions && !configuration_source {
        resolver
            .discovery
            .definitions(path)?
            .into_iter()
            .map(|definition| {
                let source = resolver.load(&definition)?;

                Ok((definition, source))
            })
            .collect::<io::Result<Vec<_>>>()?
    } else {
        Vec::new()
    };

    let constants = (load_definitions && !configuration_source)
        .then(|| resolver.discovery.constants(path))
        .transpose()?
        .unwrap_or_default();

    let open_editor_modules = editor_modules
        .map(|modules| {
            let mut open = Vec::new();

            for module in modules {
                if resolver.is_open(module)? {
                    open.push(module.clone());
                }
            }

            Ok::<_, io::Error>(open)
        })
        .transpose()?;

    let mut documentation = BTreeMap::new();

    if let Some(environment) = &environment {
        documentation.extend(
            environment
                .documentation
                .iter()
                .map(|(name, value)| (name.clone(), value.clone())),
        );
    }

    if check {
        for documentation_path in resolver.discovery.documentation(path)? {
            let source = resolver.load(&documentation_path)?;

            let values: Documentation =
                serde_json::from_slice(source.bytes()).map_err(|error| {
                    io::Error::other(format!("{}: {error}", documentation_path.display()))
                })?;

            documentation.extend(values);
        }
    }

    let mut context = CallbackContext {
        resolver,
        options,
        environment: environment.clone(),
        configurations: BTreeMap::new(),
        source_buffer: None,
        output_buffer: Vec::new(),
        report: Report::default(),
        syntax: None,
        globals: BTreeSet::new(),
        modules: Vec::new(),
        timeout_hits: BTreeSet::new(),
        identities: BTreeMap::new(),
        error: None,
    };

    if configuration_source {
        let source = context.resolver.load(path)?;
        let mut configuration = Configuration::new()?;

        if let Err(error) = configuration.extract_luau(source.bytes()) {
            let message = error.to_string();

            let message = if message.starts_with("Unknown lint ") {
                message
            } else {
                format!("TypeError: {message}")
            };

            context.report.diagnostics.push(Diagnostic {
                path: path.to_owned(),
                line: 0,
                column: 0,
                end_line: 0,
                end_column: 0,
                message,
                is_error: true,
                related: Vec::new(),
            });
        }
    }

    let checker = Checker::new(&mut context, options)?;
    checker.register_builtins(&mut context)?;

    if load_definitions && configuration_source {
        checker.load_definition(
            &mut context,
            include_bytes!("project/configuration/configuration.d.luau"),
            "configuration.d.luau",
        )?;
    }

    if load_definitions && let Some(environment) = &environment {
        let source = context.resolver.load(&environment.definition)?;

        let package = environment
            .definition
            .to_str()
            .ok_or_else(|| io::Error::other("definition path requires UTF-8"))?;

        checker.load_definition(&mut context, source.bytes(), package)?;
        checker.register_roblox_magic(&mut context, environment)?;
    }

    for (definition, source) in definitions {
        checker.load_definition(
            &mut context,
            source.bytes(),
            definition
                .to_str()
                .ok_or_else(|| io::Error::other("definition path requires UTF-8"))?,
        )?;
    }

    if !constants.is_empty() {
        checker.load_definition(&mut context, &constants, "build constants")?;
    }

    checker.freeze(&mut context)?;

    if check {
        checker.check(&mut context, path)?;

        if let Some(modules) = &open_editor_modules {
            for module in modules {
                if module != path {
                    checker.check(&mut context, module)?;
                }
            }
        }

        checker.timeouts(&mut context)?;
        checker.result(&mut context, path)?;
        checker.modules(&mut context)?;

        let modules = std::mem::take(&mut context.modules);

        for module in modules {
            if module != path {
                checker.result(&mut context, &module)?;
                checker.timeouts(&mut context)?;
            }
        }

        if options.annotations {
            checker.annotation(&mut context, path)?;
        }
    } else {
        checker.parse(&mut context, path)?;
        checker.parse_diagnostics(&mut context, path)?;
        checker.syntax(&mut context, path)?;
        checker.globals(&mut context)?;
    }

    if !documentation.is_empty() {
        context
            .report
            .documentation
            .insert(path.to_owned(), Arc::new(documentation));
    }

    let mut editor = editor
        .map(|(line, column, operation)| checker.editor(&mut context, path, line, column, operation))
        .transpose()?;

    if let Some(environment) = &environment
        && let Some(entries) = &mut editor
    {
        for entry in entries {
            if let Some(path) = entry.path.clone() {
                entry.path = Some(environment.source(&path));
            }
        }
    }

    normalize_diagnostics(&mut context.report.diagnostics);
    context.report.timeout_hits = context.timeout_hits.into_iter().collect();

    Ok((context.report, context.syntax, context.globals, environment, editor))
}

fn normalize_diagnostics(diagnostics: &mut Vec<Diagnostic>) {
    diagnostics.sort_by(|left, right| {
        (
            &left.path,
            left.line,
            left.column,
            left.end_line,
            left.end_column,
            &left.message,
            left.is_error,
        )
            .cmp(&(
                &right.path,
                right.line,
                right.column,
                right.end_line,
                right.end_column,
                &right.message,
                right.is_error,
            ))
    });

    diagnostics.dedup_by(|left, right| {
        left.path == right.path
            && left.line == right.line
            && left.column == right.column
            && left.end_line == right.end_line
            && left.end_column == right.end_column
            && left.message == right.message
            && left.is_error == right.is_error
    });
}

fn empty_entry() -> EditorEntry {
    EditorEntry {
        name: None,
        description: None,
        documentation: None,
        documentation_text: None,
        path: None,
        range: None,
        color: None,
        imports: None,
        require: None,
        caller: None,
        container: None,
        modifiers: None,
        selection: None,
        kind: None,
        declaration: None,
        insert: None,
        deprecated: None,
        label: None,
        active: None,
        parameters: None,
        error: None,
    }
}

fn unwrap_expression(mut value: &Value) -> &Value {
    while matches!(
        crate::syntax::kind(value),
        "AstExprGroup" | "AstExprTypeAssertion" | "AstExprInstantiate"
    ) {
        value = &value["expr"];
    }

    value
}

fn string_expression(value: &Value) -> Option<&str> {
    let value = unwrap_expression(value);

    (crate::syntax::kind(value) == "AstExprConstantString")
        .then(|| crate::syntax::field(value, "value"))
}

fn static_strings(document: &Value) -> BTreeMap<String, String> {
    fn collect<'value>(value: &'value Value, bindings: &mut BTreeMap<String, &'value Value>) {
        match value {
            Value::Object(fields) => {
                if crate::syntax::kind(value) == "AstStatLocal" {
                    let variables = crate::syntax::array(&value["vars"]);
                    let values = crate::syntax::array(&value["values"]);

                    for (variable, value) in variables.iter().zip(values) {
                        if let Some(location) = variable["location"].as_str() {
                            bindings.insert(location.to_owned(), value);
                        }
                    }
                }

                for child in fields.values() {
                    collect(child, bindings);
                }
            }

            Value::Array(values) => {
                for child in values {
                    collect(child, bindings);
                }
            }

            _ => {}
        }
    }

    fn resolve<'value>(
        value: &'value Value,
        bindings: &BTreeMap<String, &'value Value>,
        seen: &mut BTreeSet<String>,
    ) -> Option<String> {
        let value = unwrap_expression(value);

        if crate::syntax::kind(value) == "AstExprConstantString" {
            return Some(crate::syntax::field(value, "value").to_owned());
        }

        if crate::syntax::kind(value) != "AstExprLocal" {
            return None;
        }

        let location = value["local"]["location"].as_str()?.to_owned();

        if !seen.insert(location.clone()) {
            return None;
        }

        bindings
            .get(&location)
            .and_then(|value| resolve(value, bindings, seen))
    }

    let mut bindings = BTreeMap::new();
    collect(&document["root"], &mut bindings);

    bindings
        .iter()
        .filter_map(|(location, value)| {
            resolve(value, &bindings, &mut BTreeSet::from([location.clone()]))
                .map(|value| (location.clone(), value))
        })
        .collect()
}

fn static_string(value: &Value, bindings: &BTreeMap<String, String>) -> Option<String> {
    let value = unwrap_expression(value);

    if crate::syntax::kind(value) == "AstExprConstantString" {
        return Some(crate::syntax::field(value, "value").to_owned());
    }

    (crate::syntax::kind(value) == "AstExprLocal")
        .then(|| value["local"]["location"].as_str())
        .flatten()
        .and_then(|location| bindings.get(location).cloned())
}

fn instance_expression(environment: &Environment, from: &Path, value: &Value) -> Option<usize> {
    let value = unwrap_expression(value);

    match crate::syntax::kind(value) {
        "AstExprGlobal" => match crate::syntax::field(value, "global") {
            "game" => (environment.nodes.first()?.class_name == "DataModel").then_some(0),
            "script" => environment.node(from),
            _ => None,
        },

        "AstExprIndexName" => {
            let parent = instance_expression(environment, from, &value["expr"])?;
            let name = crate::syntax::field(value, "index");

            if name == "Parent" {
                return environment.nodes[parent].parent;
            }

            environment.nodes[parent]
                .descendants
                .iter()
                .find(|child| environment.nodes[**child].name == name)
                .copied()
        }

        "AstExprCall" => {
            let function = unwrap_expression(&value["func"]);

            if crate::syntax::kind(function) != "AstExprIndexName" {
                return None;
            }

            let method = crate::syntax::field(function, "index");

            let argument = crate::syntax::array(&value["args"])
                .first()
                .and_then(|argument| string_expression(argument))?;

            let parent = instance_expression(environment, from, &function["expr"])?;

            match method {
                "WaitForChild" | "FindFirstChild" => environment.nodes[parent]
                    .descendants
                    .iter()
                    .find(|child| environment.nodes[**child].name == argument)
                    .copied(),

                "GetService" if parent == 0 => environment.nodes[parent]
                    .descendants
                    .iter()
                    .find(|child| environment.nodes[**child].class_name == argument)
                    .copied(),

                _ => None,
            }
        }

        _ => None,
    }
}

fn location(value: &Value) -> Option<[u32; 4]> {
    let (start, end) = crate::syntax::field(value, "location").split_once(" - ")?;
    let (line, column) = start.split_once(',')?;
    let (end_line, end_column) = end.split_once(',')?;

    Some([
        line.parse().ok()?,
        column.parse().ok()?,
        end_line.parse().ok()?,
        end_column.parse().ok()?,
    ])
}

fn dependency_entries(
    resolver: &mut Resolver<'_>,
    environment: Option<&Environment>,
    path: &Path,
    document: &Value,
) -> io::Result<Vec<EditorEntry>> {
    fn visit<'value>(value: &'value Value, output: &mut Vec<&'value Value>) {
        match value {
            Value::Object(fields) => {
                if crate::syntax::kind(value) == "AstExprCall" {
                    output.push(value);
                }

                for child in fields.values() {
                    visit(child, output);
                }
            }

            Value::Array(values) => {
                for child in values {
                    visit(child, output);
                }
            }

            _ => {}
        }
    }

    let mut calls = Vec::new();
    visit(&document["root"], &mut calls);
    let bindings = static_strings(document);
    let mut result = Vec::new();

    for call in calls {
        let function = unwrap_expression(&call["func"]);

        if crate::syntax::kind(function) != "AstExprGlobal"
            || crate::syntax::field(function, "global") != "require"
        {
            continue;
        }

        let Some(argument) = crate::syntax::array(&call["args"]).first() else {
            continue;
        };

        let resolved = if let Some(specifier) = static_string(argument, &bindings) {
            if let Some(environment) = environment {
                environment.require(resolver, path, &specifier)?
            } else {
                resolver.resolve(path, &specifier)?
            }
        } else {
            environment
                .and_then(|environment| instance_expression(environment, path, argument))
                .map(|index| environment.expect("environment").identity(index))
        };

        let Some(resolved) = resolved else {
            continue;
        };

        let resolved = environment.map_or(resolved.clone(), |environment| environment.source(&resolved));

        result.push(EditorEntry {
            name: Some(resolved.to_string_lossy().into_owned()),
            path: Some(path.to_owned()),
            range: location(argument),
            require: Some(true),
            kind: Some(3),
            ..empty_entry()
        });
    }

    Ok(result)
}

fn color_entries(path: &Path, document: &Value) -> Vec<EditorEntry> {
    fn number(value: &Value) -> Option<f64> {
        match crate::syntax::kind(value) {
            "AstExprConstantNumber" | "AstExprConstantInteger" => value["value"].as_f64(),
            "AstExprUnary" if crate::syntax::field(value, "op") == "Minus" => number(&value["expr"]).map(|value| -value),
            _ => None,
        }
    }

    fn color(value: &Value) -> Option<[f32; 4]> {
        let function = unwrap_expression(&value["func"]);

        if crate::syntax::kind(function) != "AstExprIndexName"
            || crate::syntax::kind(&function["expr"]) != "AstExprGlobal"
            || crate::syntax::field(&function["expr"], "global") != "Color3"
        {
            return None;
        }

        let arguments = crate::syntax::array(&value["args"]);
        let method = crate::syntax::field(function, "index");

        if matches!(method, "new" | "fromRGB") {
            if arguments.len() != 3 {
                return None;
            }

            let divisor = if method == "fromRGB" { 255.0 } else { 1.0 };

            let mut result = [0.0, 0.0, 0.0, 1.0];

            for (index, argument) in arguments.iter().enumerate() {
                result[index] = (number(unwrap_expression(argument))? / divisor).clamp(0.0, 1.0) as f32;
            }

            return Some(result);
        }

        if method != "fromHex" || arguments.len() != 1 {
            return None;
        }

        let mut value = string_expression(&arguments[0])?.trim_start_matches('#').to_owned();

        if value.len() == 3 {
            value = value
                .chars()
                .flat_map(|character| [character, character])
                .collect();
        }

        if value.len() != 6 {
            return None;
        }

        let mut result = [0.0, 0.0, 0.0, 1.0];

        for index in 0..3 {
            result[index] = u8::from_str_radix(&value[index * 2..index * 2 + 2], 16).ok()? as f32 / 255.0;
        }

        Some(result)
    }

    fn visit(path: &Path, value: &Value, entries: &mut Vec<EditorEntry>) {
        if crate::syntax::kind(value) == "AstExprCall"
            && let Some(color) = color(value)
        {
            entries.push(EditorEntry {
                path: Some(path.to_owned()),
                range: location(value),
                selection: location(value),
                color: Some(color),
                ..empty_entry()
            });
        }

        match value {
            Value::Object(fields) => fields.values().for_each(|child| visit(path, child, entries)),
            Value::Array(values) => values.iter().for_each(|child| visit(path, child, entries)),
            _ => {}
        }
    }

    let mut entries = Vec::new();
    visit(path, &document["root"], &mut entries);

    entries
}

fn import_entries(path: &Path, document: &Value, links: Vec<EditorEntry>) -> Vec<EditorEntry> {
    fn visit(
        path: &Path,
        value: &Value,
        links: &[EditorEntry],
        entries: &mut Vec<EditorEntry>,
    ) {
        if crate::syntax::kind(value) == "AstStatLocal" {
            let variables = crate::syntax::array(&value["vars"]);
            let values = crate::syntax::array(&value["values"]);

            for (variable, expression) in variables.iter().zip(values) {
                let expression = unwrap_expression(expression);

                let Some(call) = expression.as_object().filter(|_| crate::syntax::kind(expression) == "AstExprCall")
                else {
                    continue;
                };

                let _ = call;

                let function = unwrap_expression(&expression["func"]);

                let Some(argument) = crate::syntax::array(&expression["args"]).first() else {
                    continue;
                };

                let argument = unwrap_expression(argument);
                let argument_range = location(argument);
                let name = crate::syntax::field(variable, "name");

                if crate::syntax::kind(function) == "AstExprGlobal"
                    && crate::syntax::field(function, "global") == "require"
                {
                    let Some(link) = links.iter().find(|link| {
                        link.require == Some(true) && link.range == argument_range
                    }) else {
                        continue;
                    };

                    entries.push(EditorEntry {
                        name: Some(name.to_owned()),
                        description: link.name.as_ref().map(|name| name.replace('\\', "/")),
                        label: Some("module".into()),
                        path: Some(path.to_owned()),
                        range: location(variable),
                        selection: location(variable),
                        declaration: Some(true),
                        ..empty_entry()
                    });
                } else if crate::syntax::kind(function) == "AstExprIndexName"
                    && crate::syntax::field(function, "index") == "GetService"
                    && crate::syntax::kind(&function["expr"]) == "AstExprGlobal"
                    && crate::syntax::field(&function["expr"], "global") == "game"
                    && crate::syntax::kind(argument) == "AstExprConstantString"
                {
                    entries.push(EditorEntry {
                        name: Some(name.to_owned()),
                        description: Some(crate::syntax::field(argument, "value").to_owned()),
                        label: Some("service".into()),
                        path: Some(path.to_owned()),
                        range: location(variable),
                        selection: location(variable),
                        declaration: Some(true),
                        ..empty_entry()
                    });
                }
            }
        }

        match value {
            Value::Object(fields) => fields
                .values()
                .for_each(|child| visit(path, child, links, entries)),

            Value::Array(values) => values
                .iter()
                .for_each(|child| visit(path, child, links, entries)),

            _ => {}
        }
    }

    let mut entries = Vec::new();
    visit(path, &document["root"], &links, &mut entries);

    entries
}

fn extract_entries(path: &Path, document: &Value, position: line_index::LineCol) -> Vec<EditorEntry> {
    fn after(left: [u32; 4], right: [u32; 4]) -> bool {
        (left[2], left[3]) > (right[2], right[3])
    }

    fn visit(
        path: &Path,
        value: &Value,
        position: line_index::LineCol,
        statement: Option<[u32; 4]>,
        best: &mut Option<([u32; 4], [u32; 4])>,
    ) {
        let kind = crate::syntax::kind(value);

        let current_statement = if kind.starts_with("AstStat") && kind != "AstStatBlock" {
            location(value).or(statement)
        } else {
            statement
        };

        if kind.starts_with("AstExpr")
            && let Some(range) = location(value)
            && (range[0], range[1]) == (position.line, position.col)
            && best.is_none_or(|(current, _)| after(range, current))
            && let Some(statement) = current_statement
        {
            *best = Some((range, statement));
        }

        match value {
            Value::Object(fields) => fields.values().for_each(|child| {
                visit(path, child, position, current_statement, best)
            }),

            Value::Array(values) => values.iter().for_each(|child| {
                visit(path, child, position, current_statement, best)
            }),

            _ => {}
        }
    }

    let mut best = None;
    visit(path, &document["root"], position, None, &mut best);

    best.map(|(range, statement)| EditorEntry {
        path: Some(path.to_owned()),
        range: Some(range),
        selection: Some([statement[0], statement[1], statement[0], statement[1]]),
        ..empty_entry()
    })
    .into_iter()
    .collect()
}

fn fold_entries(path: &Path, document: &Value) -> Vec<EditorEntry> {
    fn visit(path: &Path, value: &Value, entries: &mut Vec<EditorEntry>) {
        if matches!(
            crate::syntax::kind(value),
            "AstStatIf"
                | "AstStatWhile"
                | "AstStatRepeat"
                | "AstStatFor"
                | "AstStatForIn"
                | "AstStatFunction"
                | "AstStatLocalFunction"
                | "AstStatBlock"
        ) && let Some(range) = location(value)
            && range[0] < range[2]
        {
            entries.push(EditorEntry {
                path: Some(path.to_owned()),
                range: Some(range),
                selection: Some(range),
                ..empty_entry()
            });
        }

        match value {
            Value::Object(fields) => fields.values().for_each(|child| visit(path, child, entries)),
            Value::Array(values) => values.iter().for_each(|child| visit(path, child, entries)),
            _ => {}
        }
    }

    let mut entries = Vec::new();
    visit(path, &document["root"], &mut entries);

    entries
}

fn selection_entries(path: &Path, document: &Value, position: line_index::LineCol) -> Vec<EditorEntry> {
    fn contains(range: [u32; 4], position: line_index::LineCol) -> bool {
        (range[0], range[1]) <= (position.line, position.col)
            && (position.line, position.col) <= (range[2], range[3])
    }

    fn visit(
        path: &Path,
        value: &Value,
        position: line_index::LineCol,
        entries: &mut Vec<EditorEntry>,
    ) {
        if let Some(range) = location(value).filter(|range| contains(*range, position)) {
            entries.push(EditorEntry {
                path: Some(path.to_owned()),
                range: Some(range),
                selection: Some(range),
                ..empty_entry()
            });
        }

        match value {
            Value::Object(fields) => fields
                .values()
                .for_each(|child| visit(path, child, position, entries)),

            Value::Array(values) => values
                .iter()
                .for_each(|child| visit(path, child, position, entries)),

            _ => {}
        }
    }

    let mut entries = Vec::new();
    visit(path, &document["root"], position, &mut entries);

    entries.sort_by_key(|entry| {
        entry
            .range
            .map(|range| (range[0], range[1], std::cmp::Reverse((range[2], range[3]))))
    });

    entries
}

fn index_entries(path: &Path, document: &Value, links: Vec<EditorEntry>) -> Vec<EditorEntry> {
    fn expression_name(value: &Value) -> Option<String> {
        match crate::syntax::kind(value) {
            "AstExprGlobal" => Some(crate::syntax::field(value, "global").into()),
            "AstExprIndexName" => Some(crate::syntax::field(value, "index").into()),
            "AstExprLocal" => Some(crate::syntax::field(&value["local"], "name").into()),
            "AstLocal" => Some(crate::syntax::field(value, "name").into()),
            _ => None,
        }
    }

    fn visit(
        path: &Path,
        value: &Value,
        container: Option<[u32; 4]>,
        symbols: &mut Vec<EditorEntry>,
        calls: &mut Vec<EditorEntry>,
    ) {
        let kind = crate::syntax::kind(value);

        let current_container = if matches!(kind, "AstStatFunction" | "AstStatLocalFunction") {
            location(value).or(container)
        } else {
            container
        };

        match kind {
            "AstStatLocal" => {
                for local in crate::syntax::array(&value["vars"]) {
                    symbols.push(EditorEntry {
                        name: local["name"].as_str().map(str::to_owned),
                        path: Some(path.to_owned()),
                        range: location(local),
                        selection: location(local),
                        declaration: Some(true),
                        container: current_container,
                        kind: Some(13),
                        ..empty_entry()
                    });
                }
            }

            "AstStatFunction" | "AstStatLocalFunction" => {
                let name = if kind == "AstStatLocalFunction" {
                    value["name"]
                        .as_str()
                        .map(str::to_owned)
                        .or_else(|| expression_name(&value["name"]))
                } else {
                    expression_name(&value["name"])
                };

                if let Some(name) = name {
                    symbols.push(EditorEntry {
                        name: Some(name),
                        path: Some(path.to_owned()),
                        range: location(value),
                        selection: location(&value["name"]),
                        declaration: Some(true),
                        container: current_container,
                        kind: Some(12),
                        ..empty_entry()
                    });
                }
            }

            "AstStatType" | "AstStatDeclareType" => {
                let field = if kind == "AstStatType" {
                    "name"
                } else {
                    "name"
                };

                symbols.push(EditorEntry {
                    name: crate::syntax::field(value, field).to_owned().into(),
                    path: Some(path.to_owned()),
                    range: location(value),
                    declaration: Some(true),
                    container: current_container,
                    kind: Some(13),
                    ..empty_entry()
                });
            }

            "AstExprTable" => {
                for item in crate::syntax::array(&value["items"]) {
                    if crate::syntax::field(item, "kind") == "record"
                        && crate::syntax::kind(&item["key"]) == "AstExprConstantString"
                    {
                        symbols.push(EditorEntry {
                            name: Some(crate::syntax::field(&item["key"], "value").to_owned()),
                            path: Some(path.to_owned()),
                            range: location(&item["key"]),
                            selection: location(&item["key"]),
                            declaration: Some(true),
                            container: current_container,
                            kind: Some(if crate::syntax::kind(unwrap_expression(&item["value"])) == "AstExprFunction"
                                || expression_name(unwrap_expression(&item["value"]))
                                    .is_some_and(|name| name != crate::syntax::field(&item["key"], "value"))
                            {
                                12
                            } else {
                                8
                            }),
                            ..empty_entry()
                        });
                    }
                }
            }

            "AstExprCall" => {
                let function = unwrap_expression(&value["func"]);

                if !(crate::syntax::kind(function) == "AstExprGlobal"
                    && crate::syntax::field(function, "global") == "require")
                {
                    calls.push(EditorEntry {
                        name: expression_name(function),
                        path: Some(path.to_owned()),
                        range: location(function),
                        caller: location(value),
                        container: current_container,
                        kind: Some(12),
                        ..empty_entry()
                    });
                }
            }

            _ => {}
        }

        match value {
            Value::Object(fields) => {
                for child in fields.values() {
                    visit(path, child, current_container, symbols, calls);
                }
            }

            Value::Array(values) => {
                for child in values {
                    visit(path, child, current_container, symbols, calls);
                }
            }

            _ => {}
        }
    }

    let mut symbols = Vec::new();
    let mut calls = Vec::new();
    visit(path, &document["root"], None, &mut symbols, &mut calls);
    symbols.extend(calls);
    symbols.extend(links);

    symbols
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, path::Path};

    #[test]
    fn configuration_mode_translation_accepts_definition() -> io::Result<()> {
        let configuration = Configuration::new()?;
        let mut error = NativeString::default();

        let code = unsafe {
            instar_configuration_set_mode(configuration.handle as *mut c_void, 3, &mut error)
        };

        status(code, error, &mut None)
    }

    #[test]
    fn vm_call_translation_preserves_luau_failure() {
        assert!(matches("[", "value").is_err());
        assert!(!matches("^missing$", "value").expect("valid pattern"));
        assert!(matches("^value$", "value").expect("valid pattern"));
    }

    #[test]
    fn resolution_translation_preserves_optional_and_absence() -> io::Result<()> {
        let directory = tempfile::tempdir()?;
        let source_path = directory.path().join("main.luau");
        let dependency_path = directory.path().join("dependency.luau");
        fs::write(&dependency_path, "return 1")?;

        let mut sources = crate::source::SourceStore::default();
        let mut resolver = Resolver::new(&mut sources);
        let options = Options::default();

        let mut context = CallbackContext {
            resolver: &mut resolver,
            options: &options,
            environment: None,
            configurations: BTreeMap::new(),
            source_buffer: None,
            output_buffer: Vec::new(),
            report: Report::default(),
            syntax: None,
            globals: BTreeSet::new(),
            modules: Vec::new(),
            timeout_hits: BTreeSet::new(),
            identities: BTreeMap::new(),
            error: None,
        };

        let expression = serde_json::to_vec(&serde_json::json!({
            "type": "AstExprConstantString",
            "value": "./dependency"
        }))?;

        let source_name = source_path
            .to_str()
            .ok_or_else(|| io::Error::other("source path requires UTF-8"))?;

        let mut output = NativeSlice::default();
        let mut output_optional = 0;
        let mut present = 0;

        let code = resolve_callback(
            ptr::from_mut(&mut context).cast(),
            NativeSlice::new(source_name.as_bytes()),
            1,
            NativeSlice::new(&expression),
            NativeTypeCheckLimits::default(),
            ptr::from_mut(&mut output),
            ptr::from_mut(&mut output_optional),
            ptr::from_mut(&mut present),
        );

        assert_eq!(code, 1);
        assert_eq!(present, 1);
        assert_eq!(output_optional, 1);
        assert!(Path::new(&native_text(output)?).ends_with("dependency.luau"));

        let expression = serde_json::to_vec(&serde_json::json!({
            "type": "AstExprConstantString",
            "value": "./missing"
        }))?;

        output = NativeSlice::default();
        output_optional = 0;
        present = 1;

        let code = resolve_callback(
            ptr::from_mut(&mut context).cast(),
            NativeSlice::new(source_name.as_bytes()),
            1,
            NativeSlice::new(&expression),
            NativeTypeCheckLimits::default(),
            ptr::from_mut(&mut output),
            ptr::from_mut(&mut output_optional),
            ptr::from_mut(&mut present),
        );

        assert_eq!(code, 1);
        assert_eq!(present, 0);
        assert_eq!(output_optional, 0);
        assert!(native_bytes(output)?.is_empty());

        Ok(())
    }
}
