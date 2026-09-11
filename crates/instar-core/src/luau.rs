#![allow(unsafe_code)]

use std::{
    collections::BTreeMap,
    ffi::c_void,
    io,
    panic::{AssertUnwindSafe, catch_unwind},
    path::{Path, PathBuf},
    ptr, slice,
};

use crate::{
    analysis::{Annotation, Diagnostic, Options, Report},
    project::resolution::Resolver,
};

#[repr(C)]
#[derive(Clone, Copy)]
struct Bytes {
    data: *const u8,
    size: usize,
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
    unsafe fn slice<'value>(self) -> &'value [u8] {
        if self.size == 0 {
            &[]
        } else {
            unsafe { slice::from_raw_parts(self.data, self.size) }
        }
    }
    unsafe fn string(self) -> io::Result<String> {
        std::str::from_utf8(unsafe { self.slice() })
            .map(str::to_owned)
            .map_err(io::Error::other)
    }
}

type Read = extern "C" fn(*mut c_void, Bytes) -> Bytes;
type Resolve = extern "C" fn(*mut c_void, Bytes, Bytes) -> Bytes;
type Configuration = extern "C" fn(*mut c_void, Bytes, usize) -> Bytes;
type Emit = extern "C" fn(*mut c_void, Bytes, Bytes, u32, u32, bool);
type Annotate = extern "C" fn(*mut c_void, Bytes, Bytes);
type Alias = extern "C" fn(*mut c_void, Bytes, Bytes);

unsafe extern "C" {
    fn instar_aliases(source: Bytes, context: *mut c_void, alias: Alias, report: Emit);
    fn instar_analyze(
        context: *mut c_void,
        read: Read,
        resolve: Resolve,
        configuration: Configuration,
        report: Emit,
        annotate: Annotate,
        modules: *const Bytes,
        count: usize,
        definitions: *const Bytes,
        definition_count: usize,
        strict: bool,
        old_solver: bool,
        annotations: bool,
    );
}

struct Context<'resolver, 'store> {
    resolver: &'resolver mut Resolver<'store>,
    buffer: Vec<u8>,
    report: Report,
    error: Option<io::Error>,
}

impl Context<'_, '_> {
    fn call(&mut self, operation: impl FnOnce(&mut Self) -> io::Result<Option<Vec<u8>>>) -> Bytes {
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
}

extern "C" fn read(context: *mut c_void, name: Bytes) -> Bytes {
    let context = unsafe { &mut *context.cast::<Context<'_, '_>>() };

    context.call(|context| {
        let name = unsafe { name.string()? };

        Ok(Some(
            context.resolver.load(Path::new(&name))?.bytes().to_vec(),
        ))
    })
}

extern "C" fn resolve(context: *mut c_void, from: Bytes, specifier: Bytes) -> Bytes {
    let context = unsafe { &mut *context.cast::<Context<'_, '_>>() };

    context.call(|context| {
        let from = unsafe { from.string()? };

        let specifier = unsafe { specifier.string()? };

        context
            .resolver
            .resolve(Path::new(&from), &specifier)?
            .map(|path| module_name(&path).map(String::into_bytes))
            .transpose()
    })
}

extern "C" fn configuration(context: *mut c_void, name: Bytes, index: usize) -> Bytes {
    let context = unsafe { &mut *context.cast::<Context<'_, '_>>() };

    context.call(|context| {
        let name = unsafe { name.string()? };

        Ok(context
            .resolver
            .discovery
            .configurations(Path::new(&name))?
            .get(index)
            .map(|configuration| configuration.bytes.clone()))
    })
}

extern "C" fn emit(
    context: *mut c_void,
    name: Bytes,
    message: Bytes,
    line: u32,
    column: u32,
    is_error: bool,
) {
    let context = unsafe { &mut *context.cast::<Context<'_, '_>>() };

    context.call(|context| {
        context.report.diagnostics.push(Diagnostic {
            path: PathBuf::from(unsafe { name.string()? }),
            line,
            column,
            message: String::from_utf8_lossy(unsafe { message.slice() }).into_owned(),
            is_error,
        });

        Ok(None)
    });
}

extern "C" fn annotate(context: *mut c_void, name: Bytes, text: Bytes) {
    let context = unsafe { &mut *context.cast::<Context<'_, '_>>() };

    context.call(|context| {
        context.report.annotations.push(Annotation {
            path: PathBuf::from(unsafe { name.string()? }),
            bytes: unsafe { text.slice() }.to_vec(),
        });

        Ok(None)
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

pub(crate) fn analyze(
    resolver: &mut Resolver<'_>,
    modules: &[PathBuf],
    options: &Options,
) -> io::Result<Report> {
    let mut groups = BTreeMap::<Vec<PathBuf>, Vec<PathBuf>>::new();

    for path in modules {
        let source = resolver.load(path)?;
        let path = source.path().to_owned();
        let mut definitions = resolver.discovery.definitions(&path)?;

        if path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.ends_with(".d.luau"))
            && !definitions.contains(&path)
        {
            definitions.push(path.clone());
        }

        groups.entry(definitions).or_default().push(path);
    }

    let mut report = Report::default();

    for (definitions, modules) in groups {
        let result = analyze_group(resolver, &modules, &definitions, options)?;
        report.diagnostics.extend(result.diagnostics);
        report.annotations.extend(result.annotations);
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

fn analyze_group(
    resolver: &mut Resolver<'_>,
    modules: &[PathBuf],
    definitions: &[PathBuf],
    options: &Options,
) -> io::Result<Report> {
    let definition_names = definitions
        .iter()
        .map(|path| {
            resolver
                .load(path)
                .and_then(|source| module_name(source.path()))
        })
        .collect::<io::Result<Vec<_>>>()?;

    let definitions: Vec<_> = definition_names
        .iter()
        .map(|name| Bytes::new(name.as_bytes()))
        .collect();

    let names = modules
        .iter()
        .map(|path| {
            resolver
                .load(path)
                .and_then(|source| module_name(source.path()))
        })
        .collect::<io::Result<Vec<_>>>()?;

    let modules: Vec<_> = names
        .iter()
        .map(|name| Bytes::new(name.as_bytes()))
        .collect();

    let mut context = Context {
        resolver,
        buffer: Vec::new(),
        report: Report::default(),
        error: None,
    };

    unsafe {
        instar_analyze(
            ptr::from_mut(&mut context).cast(),
            read,
            resolve,
            configuration,
            emit,
            annotate,
            modules.as_ptr(),
            modules.len(),
            definitions.as_ptr(),
            definitions.len(),
            options.strict,
            options.old_solver,
            options.annotations,
        );
    }

    if let Some(error) = context.error {
        return Err(error);
    }

    Ok(context.report)
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

extern "C" fn alias_error(context: *mut c_void, _: Bytes, message: Bytes, _: u32, _: u32, _: bool) {
    let context = unsafe { &mut *context.cast::<Aliases>() };

    let result = catch_unwind(AssertUnwindSafe(|| {
        String::from_utf8_lossy(unsafe { message.slice() }).into_owned()
    }));

    context.error = Some(result.unwrap_or_else(|_| "configuration callback panicked".into()));
}

pub(crate) fn aliases(source: &[u8]) -> io::Result<BTreeMap<String, String>> {
    let mut aliases = Aliases::default();

    unsafe {
        instar_aliases(
            Bytes::new(source),
            ptr::from_mut(&mut aliases).cast(),
            alias,
            alias_error,
        );
    }

    if let Some(error) = aliases.error {
        return Err(io::Error::other(error));
    }

    Ok(aliases.values)
}
