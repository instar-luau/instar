#![expect(
    unsafe_code,
    reason = "This module owns the native Luau FFI and its callback lifetimes"
)]

use std::{
    collections::BTreeMap,
    ffi::c_void,
    io,
    panic::{AssertUnwindSafe, catch_unwind},
    path::{Path, PathBuf},
    ptr, slice,
};

use crate::{
    analysis::{Annotation, Diagnostic, EditorEntry, EditorResult, Options, Report},
    project::resolution::Resolver,
};

const BUILD_CONSTANT_DEFINITIONS: &str = "@instar/build/constants.d.luau";
const ROBLOX_DEFINITIONS: &str = "@instar/roblox.d.luau";

#[repr(C)]
#[derive(Clone, Copy)]
pub(crate) struct Bytes {
    pub(crate) data: *const u8,
    pub(crate) size: usize,
}

impl Bytes {
    fn new(bytes: &[u8]) -> Self {
        Self {
            data: bytes.as_ptr(),
            size: bytes.len(),
        }
    }

    fn absent() -> Self {
        Self {
            data: ptr::null(),
            size: 0,
        }
    }

    unsafe fn slice<'value>(self) -> io::Result<&'value [u8]> {
        if self.size == 0 {
            Ok(&[])
        } else if self.data.is_null() {
            Err(io::Error::other("native byte range is invalid"))
        } else {
            Ok(unsafe { slice::from_raw_parts(self.data, self.size) })
        }
    }

    unsafe fn string(self) -> io::Result<String> {
        std::str::from_utf8(unsafe { self.slice()? })
            .map(str::to_owned)
            .map_err(io::Error::other)
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Position {
    line: u32,
    column: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Range {
    begin: Position,
    end: Position,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Module {
    name: Bytes,
    source: Bytes,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Configuration {
    module: Bytes,
    path: Bytes,
    source: Bytes,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct RobloxClass {
    name: Bytes,
    service: i32,
    creatable: i32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct RobloxNode {
    name: Bytes,
    class_name: Bytes,
    parent: usize,
    has_parent: i32,
}

type ReadCallback = extern "C" fn(*mut c_void, Bytes) -> Bytes;
type Resolve = extern "C" fn(*mut c_void, Bytes, Range, Bytes) -> Bytes;
type ReportCallback = extern "C" fn(*mut c_void, Bytes, Bytes, Range, i32);
type TypeCallback = extern "C" fn(*mut c_void, Bytes);
type CompletionCallback = extern "C" fn(*mut c_void, Bytes, Bytes, Bytes, Bytes, u32, i32);
type SignatureCallback = extern "C" fn(*mut c_void, Bytes, *const Bytes, usize, u32);
type DestinationCallback = extern "C" fn(*mut c_void, Bytes, Range, Bytes);
type CallCallback = extern "C" fn(*mut c_void, Bytes, Range, Bytes, Range, i32, Range);
type ExtractCallback = extern "C" fn(*mut c_void, Range, Range);
type SymbolCallback = extern "C" fn(*mut c_void, Bytes, Bytes, Range, Range, u32, i32, u32);
type ImportCallback = extern "C" fn(*mut c_void, Bytes, Bytes, Bytes, Range);
type ScopeCallback = extern "C" fn(*mut c_void, Bytes, i32, u32, i32);
type AnnotationCallback = extern "C" fn(*mut c_void, Bytes, Position, Bytes);
type AliasCallback = extern "C" fn(*mut c_void, Bytes, Bytes);
type FailureCallback = extern "C" fn(*mut c_void, Bytes);

unsafe extern "C" {
    fn instar_engine_create(
        modules: *const Module,
        count: usize,
        context: *mut c_void,
        reader: Option<ReadCallback>,
        resolver: Option<Resolve>,
        configurations: *const Configuration,
        configuration_count: usize,
        mode: i32,
        old_solver: i32,
    ) -> *mut c_void;

    fn instar_engine_destroy(engine: *mut c_void);

    fn instar_engine_load_definitions(
        engine: *mut c_void,
        definitions: *const Module,
        definition_count: usize,
        target_paths: *const Bytes,
        target_count: usize,
        all_targets: i32,
        context: *mut c_void,
        failure: Option<FailureCallback>,
    ) -> i32;

    fn instar_engine_prepare_roblox(
        engine: *mut c_void,
        enumerations: *const Bytes,
        enumeration_count: usize,
        classes: *const RobloxClass,
        class_count: usize,
        nodes: *const RobloxNode,
        node_count: usize,
        context: *mut c_void,
        failure: Option<FailureCallback>,
    ) -> i32;

    fn instar_engine_check(
        engine: *mut c_void,
        context: *mut c_void,
        report: Option<ReportCallback>,
        failure: Option<FailureCallback>,
    );

    fn instar_engine_type_at(
        engine: *mut c_void,
        path: Bytes,
        position: Position,
        context: *mut c_void,
        report: Option<TypeCallback>,
        failure: Option<FailureCallback>,
    );

    fn instar_engine_hover(
        engine: *mut c_void,
        path: Bytes,
        position: Position,
        context: *mut c_void,
        report: Option<TypeCallback>,
        failure: Option<FailureCallback>,
    );

    fn instar_engine_complete(
        engine: *mut c_void,
        path: Bytes,
        position: Position,
        context: *mut c_void,
        report: Option<CompletionCallback>,
        failure: Option<FailureCallback>,
    );

    fn instar_engine_signature(
        engine: *mut c_void,
        path: Bytes,
        position: Position,
        context: *mut c_void,
        report: Option<SignatureCallback>,
        failure: Option<FailureCallback>,
    );

    fn instar_engine_definition(
        engine: *mut c_void,
        path: Bytes,
        position: Position,
        context: *mut c_void,
        report: Option<DestinationCallback>,
        failure: Option<FailureCallback>,
    );

    fn instar_engine_type_definition(
        engine: *mut c_void,
        path: Bytes,
        position: Position,
        context: *mut c_void,
        report: Option<DestinationCallback>,
        failure: Option<FailureCallback>,
    );

    fn instar_engine_implementations(
        engine: *mut c_void,
        path: Bytes,
        position: Position,
        context: *mut c_void,
        report: Option<DestinationCallback>,
        failure: Option<FailureCallback>,
    );

    fn instar_engine_references(
        engine: *mut c_void,
        path: Bytes,
        position: Position,
        context: *mut c_void,
        report: Option<DestinationCallback>,
        failure: Option<FailureCallback>,
    );

    fn instar_engine_annotations(
        engine: *mut c_void,
        path: Bytes,
        context: *mut c_void,
        report: Option<AnnotationCallback>,
        failure: Option<FailureCallback>,
    );

    fn instar_engine_calls(
        engine: *mut c_void,
        path: Bytes,
        context: *mut c_void,
        report: Option<CallCallback>,
        failure: Option<FailureCallback>,
    );

    fn instar_engine_extract(
        engine: *mut c_void,
        path: Bytes,
        position: Position,
        context: *mut c_void,
        report: Option<ExtractCallback>,
        failure: Option<FailureCallback>,
    );

    fn instar_engine_tokens(
        engine: *mut c_void,
        path: Bytes,
        context: *mut c_void,
        report: Option<SymbolCallback>,
        failure: Option<FailureCallback>,
    );

    fn instar_engine_index(
        engine: *mut c_void,
        path: Bytes,
        context: *mut c_void,
        symbol: Option<SymbolCallback>,
        call: Option<CallCallback>,
        failure: Option<FailureCallback>,
    );

    fn instar_engine_imports(
        engine: *mut c_void,
        path: Bytes,
        position: Position,
        context: *mut c_void,
        report: Option<ImportCallback>,
        failure: Option<FailureCallback>,
    );

    fn instar_engine_scope(
        engine: *mut c_void,
        path: Bytes,
        position: Position,
        context: *mut c_void,
        report: Option<ScopeCallback>,
        failure: Option<FailureCallback>,
    );

    fn instar_parse_aliases(
        source: Bytes,
        executable: i32,
        context: *mut c_void,
        alias: Option<AliasCallback>,
        failure: Option<FailureCallback>,
    );

    fn instar_matches(pattern: Bytes, source: Bytes) -> i32;
}

pub(crate) fn matches(pattern: &str, source: &str) -> io::Result<bool> {
    match unsafe {
        instar_matches(
            Bytes::new(pattern.as_bytes()),
            Bytes::new(source.as_bytes()),
        )
    } {
        0 => Ok(false),
        1 => Ok(true),
        _ => Err(io::Error::other("invalid lint ignore pattern")),
    }
}

struct Context<'resolver, 'store> {
    resolver: &'resolver mut Resolver<'store>,
    environment: std::sync::Arc<crate::roblox::Environment>,
    constants: Vec<u8>,
    buffer: Vec<u8>,
    report: Report,
    error: Option<io::Error>,
    annotations: Vec<NativeAnnotation>,
    declaration: bool,
}

#[derive(Clone)]
struct NativeAnnotation {
    path: String,
    position: Position,
    text: Vec<u8>,
}

impl Context<'_, '_> {
    fn call_bytes(
        &mut self,
        operation: impl FnOnce(&mut Self) -> io::Result<Option<Vec<u8>>>,
    ) -> Bytes {
        if self.error.is_some() {
            return Bytes::absent();
        }

        match catch_unwind(AssertUnwindSafe(|| operation(self))) {
            Ok(Ok(Some(bytes))) => {
                self.buffer = bytes;

                Bytes::new(&self.buffer)
            }

            Ok(Ok(None)) => Bytes::absent(),

            Ok(Err(error)) => {
                self.error = Some(error);

                Bytes::absent()
            }

            Err(_) => {
                self.error = Some(io::Error::other("native callback panicked"));

                Bytes::absent()
            }
        }
    }

    fn call(&mut self, operation: impl FnOnce(&mut Self) -> io::Result<()>) {
        if self.error.is_some() {
            return;
        }

        match catch_unwind(AssertUnwindSafe(|| operation(self))) {
            Ok(Ok(())) => {}
            Ok(Err(error)) => self.error = Some(error),
            Err(_) => self.error = Some(io::Error::other("native callback panicked")),
        }
    }
}

extern "C" fn read(context: *mut c_void, name: Bytes) -> Bytes {
    let context = unsafe { &mut *context.cast::<Context<'_, '_>>() };

    context.call_bytes(|context| {
        let name = unsafe { name.string()? };

        if name == BUILD_CONSTANT_DEFINITIONS {
            return Ok(Some(context.constants.clone()));
        }

        let path = Path::new(&name);

        if !context.environment.readable(path) {
            return Ok(None);
        }

        Ok(Some(
            context
                .resolver
                .load(&context.environment.source(path))?
                .bytes()
                .to_vec(),
        ))
    })
}

extern "C" fn resolve(context: *mut c_void, from: Bytes, range: Range, specifier: Bytes) -> Bytes {
    let context = unsafe { &mut *context.cast::<Context<'_, '_>>() };

    context.call_bytes(|context| {
        let from = unsafe { from.string()? };

        let specifier = unsafe { specifier.string()? };

        let path = context
            .environment
            .require(context.resolver, Path::new(&from), &specifier)?;

        let _ = range;

        path.map(|path| module_name(&path).map(String::into_bytes))
            .transpose()
    })
}

extern "C" fn failure(context: *mut c_void, message: Bytes) {
    let context = unsafe { &mut *context.cast::<Context<'_, '_>>() };

    context.call(|context| {
        context.error = Some(io::Error::other(unsafe { message.string()? }));

        Ok(())
    });
}

extern "C" fn report(
    context: *mut c_void,
    path: Bytes,
    message: Bytes,
    range: Range,
    is_error: i32,
) {
    let context = unsafe { &mut *context.cast::<Context<'_, '_>>() };

    context.call(|context| {
        let path = unsafe { path.string()? };

        context.report.diagnostics.push(Diagnostic {
            path: context.environment.source(Path::new(&path)),
            line: range.begin.line,
            column: range.begin.column,
            end_line: range.end.line,
            end_column: range.end.column,
            message: String::from_utf8_lossy(unsafe { message.slice()? }).into_owned(),
            is_error: is_error != 0,
            related: Vec::new(),
        });

        Ok(())
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

extern "C" fn type_result(context: *mut c_void, description: Bytes) {
    let context = unsafe { &mut *context.cast::<Context<'_, '_>>() };

    context.call(|context| {
        let mut entry = empty_entry();

        entry.description = Some(unsafe { description.string()? });

        context.report.editor = Some(EditorResult::Entry(Box::new(entry)));

        Ok(())
    });
}

extern "C" fn completion_result(
    context: *mut c_void,
    label: Bytes,
    description: Bytes,
    documentation: Bytes,
    insert: Bytes,
    kind: u32,
    deprecated: i32,
) {
    let context = unsafe { &mut *context.cast::<Context<'_, '_>>() };

    context.call(|context| {
        let mut entry = empty_entry();

        entry.label = Some(unsafe { label.string()? });

        entry.description = Some(unsafe { description.string()? });

        entry.documentation = Some(unsafe { documentation.string()? });

        entry.insert = Some(unsafe { insert.string()? });

        entry.kind = Some(kind);
        entry.deprecated = Some(deprecated != 0);

        match &mut context.report.editor {
            Some(EditorResult::Entries(entries)) => entries.push(entry),
            _ => context.report.editor = Some(EditorResult::Entries(vec![entry])),
        }

        Ok(())
    });
}

extern "C" fn signature_result(
    context: *mut c_void,
    description: Bytes,
    parameters: *const Bytes,
    count: usize,
    active: u32,
) {
    let context = unsafe { &mut *context.cast::<Context<'_, '_>>() };

    context.call(|context| {
        if count != 0 && parameters.is_null() {
            return Err(io::Error::other("native signature parameters are invalid"));
        }

        let mut entry = empty_entry();

        entry.description = Some(unsafe { description.string()? });

        entry.parameters = Some(
            (0..count)
                .map(|index| unsafe { (*parameters.add(index)).string() })
                .collect::<io::Result<Vec<_>>>()?,
        );

        entry.active = Some(active);
        context.report.editor = Some(EditorResult::Entry(Box::new(entry)));

        Ok(())
    });
}

fn range_values(range: Range) -> [u32; 4] {
    [
        range.begin.line,
        range.begin.column,
        range.end.line,
        range.end.column,
    ]
}

extern "C" fn destination_result(context: *mut c_void, path: Bytes, range: Range, name: Bytes) {
    let context = unsafe { &mut *context.cast::<Context<'_, '_>>() };

    context.call(|context| {
        let mut entry = empty_entry();

        entry.path = Some(PathBuf::from(unsafe { path.string()? }));

        entry.range = Some(range_values(range));

        entry.name = Some(unsafe { name.string()? });

        entry.declaration = context.declaration.then_some(true);

        match &mut context.report.editor {
            Some(EditorResult::Entries(entries)) => entries.push(entry),
            _ => context.report.editor = Some(EditorResult::Entries(vec![entry])),
        }

        Ok(())
    });
}

extern "C" fn extract_result(context: *mut c_void, range: Range, selection: Range) {
    let context = unsafe { &mut *context.cast::<Context<'_, '_>>() };

    context.call(|context| {
        let mut entry = empty_entry();
        entry.range = Some(range_values(range));
        entry.selection = Some(range_values(selection));

        match &mut context.report.editor {
            Some(EditorResult::Entries(entries)) => entries.push(entry),
            _ => context.report.editor = Some(EditorResult::Entries(vec![entry])),
        }

        Ok(())
    });
}

extern "C" fn symbol_result(
    context: *mut c_void,
    name: Bytes,
    path: Bytes,
    range: Range,
    selection: Range,
    kind: u32,
    declaration: i32,
    modifiers: u32,
) {
    let context = unsafe { &mut *context.cast::<Context<'_, '_>>() };

    context.call(|context| {
        let mut entry = empty_entry();

        entry.name = Some(unsafe { name.string()? });

        entry.path = Some(PathBuf::from(unsafe { path.string()? }));

        entry.range = Some(range_values(range));
        entry.selection = Some(range_values(selection));
        entry.kind = Some(kind);
        entry.declaration = Some(declaration != 0);
        entry.modifiers = Some(modifiers);

        match &mut context.report.editor {
            Some(EditorResult::Entries(entries)) => entries.push(entry),
            _ => context.report.editor = Some(EditorResult::Entries(vec![entry])),
        }

        Ok(())
    });
}

extern "C" fn import_result(
    context: *mut c_void,
    name: Bytes,
    label: Bytes,
    target: Bytes,
    range: Range,
) {
    let context = unsafe { &mut *context.cast::<Context<'_, '_>>() };

    context.call(|context| {
        let mut entry = empty_entry();

        entry.name = Some(unsafe { name.string()? });

        entry.label = Some(unsafe { label.string()? });

        entry.description = Some(unsafe { target.string()? });

        entry.range = Some(range_values(range));
        entry.imports = Some(true);

        match &mut context.report.editor {
            Some(EditorResult::Entries(entries)) => entries.push(entry),
            _ => context.report.editor = Some(EditorResult::Entries(vec![entry])),
        }

        Ok(())
    });
}

extern "C" fn scope_result(
    context: *mut c_void,
    name: Bytes,
    has_name: i32,
    kind: u32,
    has_kind: i32,
) {
    let context = unsafe { &mut *context.cast::<Context<'_, '_>>() };

    context.call(|context| {
        let mut entry = empty_entry();

        if has_name != 0 {
            entry.name = Some(unsafe { name.string()? });
        }

        if has_kind != 0 {
            entry.kind = Some(kind);
        }

        context.report.editor = Some(EditorResult::Entry(Box::new(entry)));

        Ok(())
    });
}

extern "C" fn call_result(
    context: *mut c_void,
    path: Bytes,
    range: Range,
    name: Bytes,
    caller: Range,
    has_container: i32,
    container: Range,
) {
    let context = unsafe { &mut *context.cast::<Context<'_, '_>>() };

    context.call(|context| {
        let mut entry = empty_entry();

        entry.path = Some(PathBuf::from(unsafe { path.string()? }));

        entry.range = Some(range_values(range));

        entry.name = Some(unsafe { name.string()? });

        entry.caller = Some(range_values(caller));
        entry.container = (has_container != 0).then(|| range_values(container));

        match &mut context.report.editor {
            Some(EditorResult::Entries(entries)) => entries.push(entry),
            _ => context.report.editor = Some(EditorResult::Entries(vec![entry])),
        }

        Ok(())
    });
}

extern "C" fn annotation_result(
    context: *mut c_void,
    path: Bytes,
    position: Position,
    text: Bytes,
) {
    let context = unsafe { &mut *context.cast::<Context<'_, '_>>() };

    context.call(|context| {
        context.annotations.push(NativeAnnotation {
            path: unsafe { path.string()? },
            position,
            text: unsafe { text.slice()?.to_vec() },
        });

        Ok(())
    });
}

fn module_name(path: &Path) -> io::Result<String> {
    path.to_str()
        .map(|path| {
            #[cfg(windows)]
            {
                path.replace('\\', "/")
            }

            #[cfg(not(windows))]
            {
                path.to_owned()
            }
        })
        .ok_or_else(|| {
            io::Error::other(format!(
                "{}: module identity requires UTF-8",
                path.display()
            ))
        })
}

struct OwnedModule {
    name: Vec<u8>,
    source: Vec<u8>,
}

impl OwnedModule {
    fn ffi(&self) -> Module {
        Module {
            name: Bytes::new(&self.name),
            source: Bytes::new(&self.source),
        }
    }
}

struct OwnedConfiguration {
    module: Vec<u8>,
    path: Vec<u8>,
    source: Vec<u8>,
}

impl OwnedConfiguration {
    fn ffi(&self) -> Configuration {
        Configuration {
            module: Bytes::new(&self.module),
            path: Bytes::new(&self.path),
            source: Bytes::new(&self.source),
        }
    }
}

struct NativeEngine(*mut c_void);

impl Drop for NativeEngine {
    fn drop(&mut self) {
        unsafe { instar_engine_destroy(self.0) };
    }
}

#[derive(Default)]
pub(crate) struct Session {
    changed: Vec<PathBuf>,
    refresh: bool,
    query: Option<(PathBuf, line_index::LineCol, String)>,

    environments:
        BTreeMap<crate::configuration::RobloxConfig, std::sync::Arc<crate::roblox::Environment>>,
}

impl Session {
    pub(crate) fn query(
        &mut self,
        resolver: &mut Resolver<'_>,
        modules: &[PathBuf],
        path: &Path,
        position: line_index::LineCol,
        operation: &str,
    ) -> io::Result<Report> {
        self.query = Some((path.to_owned(), position, operation.to_owned()));
        let result = self.analyze(resolver, modules, &Options::default());
        self.query = None;

        result
    }

    pub(crate) fn refresh(&mut self) {
        self.refresh = true;
    }

    pub(crate) fn change(&mut self, path: &Path) {
        self.changed.push(path.to_owned());
    }

    pub(crate) fn analyze(
        &mut self,
        resolver: &mut Resolver<'_>,
        modules: &[PathBuf],
        options: &Options,
    ) -> io::Result<Report> {
        let changed = std::mem::take(&mut self.changed);
        let refresh = std::mem::take(&mut self.refresh);

        if refresh || options.update {
            self.environments.clear();
        }

        let mut groups: Vec<(
            Vec<PathBuf>,
            Vec<PathBuf>,
            Vec<u8>,
            Option<std::sync::Arc<crate::roblox::Environment>>,
            Vec<PathBuf>,
        )> = Vec::new();

        for path in modules {
            let source = resolver.load(path)?;
            let path = source.path().to_owned();

            let configuration = crate::project::ConfigKind::from_path(&path)
                == Some(crate::project::ConfigKind::Luau);

            let mut definitions = if configuration {
                Vec::new()
            } else {
                resolver.discovery.definitions(&path)?
            };

            if path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.ends_with(".d.luau"))
                && !definitions.contains(&path)
            {
                definitions.push(path.clone());
            }

            let documentation = if configuration {
                Vec::new()
            } else {
                resolver.discovery.documentation(&path)?
            };

            let constants = if configuration {
                Vec::new()
            } else {
                resolver.discovery.constants(&path)?
            };

            let environment = if configuration {
                None
            } else {
                let settings = resolver.discovery.roblox(&path)?;

                Some(self.environment(resolver, settings.as_ref(), options.update)?)
            };

            let group = groups.iter_mut().find(|group| {
                group.0 == definitions
                    && group.1 == documentation
                    && group.2 == constants
                    && match (&group.3, &environment) {
                        (None, None) => true,
                        (Some(left), Some(right)) => std::sync::Arc::ptr_eq(left, right),
                        _ => false,
                    }
            });

            if let Some(group) = group {
                group.4.push(path);
            } else {
                groups.push((
                    definitions,
                    documentation,
                    constants,
                    environment,
                    vec![path],
                ));
            }
        }

        let mut report = Report::default();

        for (definitions, documentation, constants, environment, modules) in groups {
            let result = self.analyze_group(
                resolver,
                &modules,
                (&definitions, &documentation, &constants),
                environment.as_ref(),
                options,
                &changed,
            )?;

            report.diagnostics.extend(result.diagnostics);
            report.annotations.extend(result.annotations);
            report.documentation.extend(result.documentation);

            if result.editor.is_some() {
                report.editor = result.editor;
            }
        }

        let mut seen = std::collections::BTreeSet::new();

        report.diagnostics.retain(|diagnostic| {
            seen.insert((
                diagnostic.path.clone(),
                diagnostic.line,
                diagnostic.column,
                diagnostic.message.clone(),
                diagnostic.is_error,
            ))
        });

        Ok(report)
    }

    pub(crate) fn environment(
        &mut self,
        resolver: &mut Resolver<'_>,
        settings: Option<&crate::configuration::RobloxConfig>,
        update: bool,
    ) -> io::Result<std::sync::Arc<crate::roblox::Environment>> {
        let Some(settings) = settings else {
            return Ok(std::sync::Arc::default());
        };

        if let Some(environment) = self.environments.get(settings) {
            return Ok(std::sync::Arc::clone(environment));
        }

        let environment = std::sync::Arc::new(crate::roblox::Environment::load(
            resolver, settings, update,
        )?);

        self.environments
            .insert(settings.clone(), std::sync::Arc::clone(&environment));

        Ok(environment)
    }

    fn analyze_group(
        &mut self,
        resolver: &mut Resolver<'_>,
        modules: &[PathBuf],
        assets: (&[PathBuf], &[PathBuf], &[u8]),
        environment: Option<&std::sync::Arc<crate::roblox::Environment>>,
        options: &Options,
        _changed: &[PathBuf],
    ) -> io::Result<Report> {
        let (definitions, documentation, constants) = assets;
        let environment = environment.cloned().unwrap_or_default();
        let mut module_values = Vec::new();
        let mut module_names = Vec::new();

        for path in modules {
            let source = resolver.load(path)?;
            let name = module_name(source.path())?;
            module_names.push(name.clone());

            module_values.push(OwnedModule {
                name: name.into_bytes(),
                source: source.bytes().to_vec(),
            });
        }

        let mut definition_values = Vec::new();
        let mut loaded_definitions = std::collections::BTreeSet::new();

        for path in definitions {
            let source = resolver.load(path)?;
            let name = module_name(source.path())?;

            if loaded_definitions.insert(name.clone()) {
                definition_values.push(OwnedModule {
                    name: name.into_bytes(),
                    source: source.bytes().to_vec(),
                });
            }
        }

        if !constants.is_empty() && loaded_definitions.insert(BUILD_CONSTANT_DEFINITIONS.to_owned())
        {
            definition_values.push(OwnedModule {
                name: BUILD_CONSTANT_DEFINITIONS.as_bytes().to_vec(),
                source: constants.to_vec(),
            });
        }

        if environment.enabled && loaded_definitions.insert(ROBLOX_DEFINITIONS.to_owned()) {
            definition_values.push(OwnedModule {
                name: ROBLOX_DEFINITIONS.as_bytes().to_vec(),
                source: environment.definitions.as_bytes().to_vec(),
            });
        }

        let mut configuration_values = Vec::new();

        for path in modules {
            for configuration in resolver.discovery.configurations(path)? {
                let name = module_name(&configuration.path)?;

                if configuration_values
                    .iter()
                    .all(|value: &OwnedConfiguration| value.path != name.as_bytes())
                {
                    configuration_values.push(OwnedConfiguration {
                        module: Vec::new(),
                        path: name.into_bytes(),
                        source: configuration.bytes.clone(),
                    });
                }
            }
        }

        let mut context = Context {
            resolver,
            environment,
            constants: constants.to_vec(),
            buffer: Vec::new(),
            report: Report::default(),
            error: None,
            annotations: Vec::new(),
            declaration: false,
        };

        let native_modules: Vec<_> = module_values.iter().map(OwnedModule::ffi).collect();

        let native_configurations: Vec<_> = configuration_values
            .iter()
            .map(OwnedConfiguration::ffi)
            .collect();

        let context_pointer = ptr::from_mut(&mut context).cast();

        let mode = match options.mode {
            None => 0,
            Some(crate::analysis::Mode::Strict) => 1,
            Some(crate::analysis::Mode::Nonstrict) => 2,
            Some(crate::analysis::Mode::Nocheck) => 3,
        };

        let engine = unsafe {
            instar_engine_create(
                native_modules.as_ptr(),
                native_modules.len(),
                context_pointer,
                Some(read),
                Some(resolve),
                native_configurations.as_ptr(),
                native_configurations.len(),
                mode,
                i32::from(options.old_solver),
            )
        };

        if engine.is_null() {
            return Err(io::Error::other("native engine initialization failed"));
        }

        let engine = NativeEngine(engine);
        let native_definitions: Vec<_> = definition_values.iter().map(OwnedModule::ffi).collect();

        let definitions_result = unsafe {
            instar_engine_load_definitions(
                engine.0,
                native_definitions.as_ptr(),
                native_definitions.len(),
                ptr::null(),
                0,
                1,
                context_pointer,
                Some(failure),
            )
        };

        if definitions_result == 0 {
            return Err(context
                .error
                .take()
                .unwrap_or_else(|| io::Error::other("native definitions failed")));
        }

        let configuration_targets: Vec<_> = module_values
            .iter()
            .zip(modules)
            .filter_map(|(module, path)| {
                (crate::project::ConfigKind::from_path(path)
                    == Some(crate::project::ConfigKind::Luau))
                .then_some(Bytes::new(&module.name))
            })
            .collect();

        if !configuration_targets.is_empty() {
            let configuration_definition = OwnedModule {
                name: b"configuration.d.luau".to_vec(),
                source: include_bytes!("../bridge/configuration.d.luau").to_vec(),
            };

            let native_configuration_definition = configuration_definition.ffi();

            let result = unsafe {
                instar_engine_load_definitions(
                    engine.0,
                    &native_configuration_definition,
                    1,
                    configuration_targets.as_ptr(),
                    configuration_targets.len(),
                    0,
                    context_pointer,
                    Some(failure),
                )
            };

            if result == 0 {
                return Err(context
                    .error
                    .take()
                    .unwrap_or_else(|| io::Error::other("configuration definitions failed")));
            }
        }

        if context.environment.enabled {
            let enumerations: Vec<_> = context
                .environment
                .enumerations
                .iter()
                .map(|value| Bytes::new(value.as_bytes()))
                .collect();

            let mut classes = Vec::new();

            for value in &context.environment.classes {
                let mut fields = value.split('\0');
                let name = fields.next().unwrap_or_default().as_bytes().to_vec();
                let flags = fields.next().unwrap_or_default().as_bytes();

                classes.push((
                    name,
                    flags.first() == Some(&b'1'),
                    flags.get(1) == Some(&b'1'),
                ));
            }

            let native_classes: Vec<_> = classes
                .iter()
                .map(|(name, service, creatable)| RobloxClass {
                    name: Bytes::new(name),
                    service: i32::from(*service),
                    creatable: i32::from(*creatable),
                })
                .collect();

            let native_nodes: Vec<_> = context
                .environment
                .nodes
                .iter()
                .map(|node| RobloxNode {
                    name: Bytes::new(node.name.as_bytes()),
                    class_name: Bytes::new(node.class_name.as_bytes()),
                    parent: node.parent.unwrap_or_default(),
                    has_parent: i32::from(node.parent.is_some()),
                })
                .collect();

            let result = unsafe {
                instar_engine_prepare_roblox(
                    engine.0,
                    enumerations.as_ptr(),
                    enumerations.len(),
                    native_classes.as_ptr(),
                    native_classes.len(),
                    native_nodes.as_ptr(),
                    native_nodes.len(),
                    context_pointer,
                    Some(failure),
                )
            };

            if result == 0 {
                return Err(context
                    .error
                    .take()
                    .unwrap_or_else(|| io::Error::other("Roblox metadata failed")));
            }
        }

        unsafe {
            instar_engine_check(engine.0, context_pointer, Some(report), Some(failure));
        }

        if let Some(error) = context.error.take() {
            return Err(error);
        }

        if options.annotations {
            for name in &module_names {
                unsafe {
                    instar_engine_annotations(
                        engine.0,
                        Bytes::new(name.as_bytes()),
                        context_pointer,
                        Some(annotation_result),
                        Some(failure),
                    );
                }
            }

            if let Some(error) = context.error.take() {
                return Err(error);
            }

            for module in &module_values {
                let insertions: Vec<_> = context
                    .annotations
                    .iter()
                    .filter(|annotation| annotation.path.as_bytes() == module.name.as_slice())
                    .cloned()
                    .collect();

                if insertions.is_empty() {
                    continue;
                }

                let mut bytes = module.source.clone();

                let mut offsets = insertions
                    .iter()
                    .map(|annotation| {
                        line_column_offset(&bytes, annotation.position)
                            .map(|offset| (offset, annotation.text.clone()))
                    })
                    .collect::<io::Result<Vec<_>>>()?;

                offsets.sort_by(|left, right| right.0.cmp(&left.0));

                for (offset, text) in offsets {
                    bytes.splice(offset..offset, text);
                }

                context.report.annotations.push(Annotation {
                    path: context.environment.source(Path::new(
                        std::str::from_utf8(&module.name).map_err(io::Error::other)?,
                    )),
                    bytes,
                });
            }
        }

        if let Some((path, position, operation)) = self.query.take()
            && module_names.iter().any(|name| Path::new(name) == path)
        {
            query_engine(&engine, &mut context, &path, position, &operation)?;
        }

        if let Some(error) = context.error.take() {
            return Err(error);
        }

        let mut external_documentation = context.environment.documentation.as_ref().clone();

        for path in documentation {
            let source = context.resolver.load(path)?;

            let values: crate::analysis::Documentation = serde_json::from_slice(source.bytes())
                .map_err(|error| io::Error::other(format!("{}: {error}", path.display())))?;

            external_documentation.extend(values);
        }

        if !external_documentation.is_empty() {
            let documentation = std::sync::Arc::new(external_documentation);

            for name in module_names {
                context
                    .report
                    .documentation
                    .insert(PathBuf::from(name), std::sync::Arc::clone(&documentation));
            }
        }

        Ok(context.report)
    }
}

fn line_column_offset(source: &[u8], position: Position) -> io::Result<usize> {
    let mut offset = 0;

    for _ in 0..position.line {
        let Some(relative) = source
            .get(offset..)
            .and_then(|value| value.iter().position(|byte| *byte == b'\n'))
        else {
            return Err(io::Error::other(
                "native annotation position is outside the source",
            ));
        };

        offset += relative + 1;
    }

    let line_end = source
        .get(offset..)
        .and_then(|value| value.iter().position(|byte| *byte == b'\n'))
        .map_or(source.len(), |relative| offset + relative);

    let column = usize::try_from(position.column).map_err(io::Error::other)?;

    if offset + column > line_end {
        return Err(io::Error::other(
            "native annotation position is outside the source",
        ));
    }

    Ok(offset + column)
}

fn query_engine(
    engine: &NativeEngine,
    context: &mut Context<'_, '_>,
    path: &Path,
    position: line_index::LineCol,
    operation: &str,
) -> io::Result<()> {
    let path = module_name(path)?;

    let position = Position {
        line: position.line,
        column: position.col,
    };

    let context_pointer = ptr::from_mut(context).cast();
    let path_bytes = Bytes::new(path.as_bytes());

    match operation {
        "type" => unsafe {
            instar_engine_type_at(
                engine.0,
                path_bytes,
                position,
                context_pointer,
                Some(type_result),
                Some(failure),
            )
        },

        "hover" => unsafe {
            instar_engine_hover(
                engine.0,
                path_bytes,
                position,
                context_pointer,
                Some(type_result),
                Some(failure),
            )
        },

        "complete" | "completion" | "completionResolve" => unsafe {
            instar_engine_complete(
                engine.0,
                path_bytes,
                position,
                context_pointer,
                Some(completion_result),
                Some(failure),
            )
        },

        "signature" => unsafe {
            instar_engine_signature(
                engine.0,
                path_bytes,
                position,
                context_pointer,
                Some(signature_result),
                Some(failure),
            )
        },

        "definition" => unsafe {
            instar_engine_definition(
                engine.0,
                path_bytes,
                position,
                context_pointer,
                Some(destination_result),
                Some(failure),
            )
        },

        "prepare" => {
            context.declaration = true;

            unsafe {
                instar_engine_definition(
                    engine.0,
                    path_bytes,
                    position,
                    context_pointer,
                    Some(destination_result),
                    Some(failure),
                )
            }

            context.declaration = false;
        }

        "type_definition" | "typeDefinition" => unsafe {
            instar_engine_type_definition(
                engine.0,
                path_bytes,
                position,
                context_pointer,
                Some(destination_result),
                Some(failure),
            )
        },

        "implementation" | "implementations" => unsafe {
            instar_engine_implementations(
                engine.0,
                path_bytes,
                position,
                context_pointer,
                Some(destination_result),
                Some(failure),
            )
        },

        "references" | "localReferences" => unsafe {
            instar_engine_references(
                engine.0,
                path_bytes,
                position,
                context_pointer,
                Some(destination_result),
                Some(failure),
            )
        },

        "scope" => unsafe {
            instar_engine_scope(
                engine.0,
                path_bytes,
                position,
                context_pointer,
                Some(scope_result),
                Some(failure),
            )
        },

        "annotations" => unsafe {
            instar_engine_annotations(
                engine.0,
                path_bytes,
                context_pointer,
                Some(annotation_result),
                Some(failure),
            )
        },

        "calls" => unsafe {
            instar_engine_calls(
                engine.0,
                path_bytes,
                context_pointer,
                Some(call_result),
                Some(failure),
            )
        },

        "extract" => unsafe {
            instar_engine_extract(
                engine.0,
                path_bytes,
                position,
                context_pointer,
                Some(extract_result),
                Some(failure),
            )
        },

        "tokens" => unsafe {
            instar_engine_tokens(
                engine.0,
                path_bytes,
                context_pointer,
                Some(symbol_result),
                Some(failure),
            )
        },

        "index" => unsafe {
            instar_engine_index(
                engine.0,
                path_bytes,
                context_pointer,
                Some(symbol_result),
                Some(call_result),
                Some(failure),
            )
        },

        "imports" => unsafe {
            instar_engine_imports(
                engine.0,
                path_bytes,
                position,
                context_pointer,
                Some(import_result),
                Some(failure),
            )
        },

        _ => {
            return Err(io::Error::other(format!(
                "unsupported native query: {operation}"
            )));
        }
    }

    if let Some(error) = context.error.take() {
        return Err(error);
    }

    if operation == "annotations" {
        let entries = context
            .annotations
            .drain(..)
            .map(|annotation| {
                let mut entry = empty_entry();
                entry.path = Some(PathBuf::from(annotation.path));

                entry.range = Some([
                    annotation.position.line,
                    annotation.position.column,
                    annotation.position.line,
                    annotation.position.column,
                ]);

                entry.description = Some(String::from_utf8_lossy(&annotation.text).into_owned());

                entry
            })
            .collect();

        context.report.editor = Some(EditorResult::Entries(entries));
    }

    Ok(())
}

#[derive(Default)]
struct Aliases {
    values: BTreeMap<String, String>,
    error: Option<String>,
}

extern "C" fn alias(context: *mut c_void, name: Bytes, target: Bytes) {
    let context = unsafe { &mut *context.cast::<Aliases>() };

    let result = catch_unwind(AssertUnwindSafe(|| -> io::Result<()> {
        context
            .values
            .insert(unsafe { name.string()? }, unsafe { target.string()? });

        Ok(())
    }));

    match result {
        Ok(Ok(())) => {}
        Ok(Err(error)) => context.error = Some(error.to_string()),
        Err(_) => context.error = Some("alias callback panicked".into()),
    }
}

extern "C" fn alias_failure(context: *mut c_void, message: Bytes) {
    let context = unsafe { &mut *context.cast::<Aliases>() };

    context.error = Some(match unsafe { message.string() } {
        Ok(message) => message,
        Err(error) => error.to_string(),
    });
}

pub(crate) fn aliases(source: &[u8], executable: bool) -> io::Result<BTreeMap<String, String>> {
    let mut aliases = Aliases::default();

    unsafe {
        instar_parse_aliases(
            Bytes::new(source),
            i32::from(executable),
            ptr::from_mut(&mut aliases).cast(),
            Some(alias),
            Some(alias_failure),
        );
    }

    aliases
        .error
        .map_or(Ok(aliases.values), |error| Err(io::Error::other(error)))
}
