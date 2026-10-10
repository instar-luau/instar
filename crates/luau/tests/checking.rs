//! Native checker diagnostics and interruption boundary fixtures.

use std::{
    fmt::Write,
    io, thread,
    time::{Duration, Instant},
};

use instar_luau::{
    ImportIdentity, ImportOutcome, ImportRequest, ImportSite, Native, ResolvedImport,
};

use instar_shared::{Completion, DiagnosticCategory, Reason};

use crate::support::{definition, location, module, options, request};

#[test]
fn native_type_codes_locations_and_related_data_are_retained() -> io::Result<()> {
    let mut native = Native::default();

    let input = request(vec![module(
        1,
        include_str!("fixtures/checking/native/input.luau"),
        8,
    )?])?;

    let result = native.analyze(&input, &options(Duration::from_secs(5)))?;
    assert_eq!(result.completion, Completion::Complete);

    let diagnostic = result.modules[0]
        .diagnostics
        .iter()
        .find(|diagnostic| !diagnostic.related.is_empty())
        .expect("duplicate definition with previous location");

    assert_eq!(diagnostic.category, DiagnosticCategory::Type);

    assert!(
        diagnostic
            .code
            .parse::<u32>()
            .is_ok_and(|code| code >= 1000)
    );

    assert_eq!(
        diagnostic.primary.source,
        input.modules[0].source.source.version
    );

    assert_eq!(diagnostic.primary.source.revision, 8);
    assert!(diagnostic.primary.span.start > diagnostic.related[0].location.span.start);
    assert_eq!(diagnostic.related[0].location.source.revision, 8);

    native.finish()
}

#[test]
fn lexical_loader_targets_supply_real_imported_types() -> io::Result<()> {
    let source = include_str!("fixtures/checking/lexical/input.luau");
    let mut entry = module(1, source, 1)?;

    let dependency = module(
        2,
        include_str!("fixtures/checking/lexical/dependency.luau"),
        2,
    )?;

    let call = source.find("loader(").expect("loader call");
    let argument = source.find("'./").expect("loader argument");
    let end = source[argument..].find(')').expect("call end") + argument;

    entry.imports.push(ResolvedImport {
        site: ImportSite {
            identity: ImportIdentity(1),
            call: location(&entry, [call, end + 1])?,
            argument: location(&entry, [argument, end])?,
            request: ImportRequest::Static(source[argument + 1..end - 1].to_owned()),
        },
        outcome: ImportOutcome::Module(dependency.analysis.module),
    });

    let input = request(vec![entry, dependency])?;
    let mut native = Native::default();
    let result = native.analyze(&input, &options(Duration::from_secs(5)))?;
    assert_eq!(result.completion, Completion::Complete);

    assert!(
        result.modules[0]
            .diagnostics
            .iter()
            .any(
                |diagnostic| diagnostic.primary.source == input.modules[0].source.source.version
                    && diagnostic.category == DiagnosticCategory::Type
            )
    );

    native.finish()
}

#[test]
fn failed_definitions_do_not_pollute_other_environments() -> io::Result<()> {
    let mut configured = module(1, include_str!("fixtures/checking/failed/input.luau"), 1)?;

    let first = definition(
        3,
        include_str!("fixtures/checking/failed/definition.luau"),
        1,
        "@custom",
    )?;

    configured
        .environment
        .as_mut()
        .expect("environment")
        .declarations[0]
        .definitions = vec![first.clone()];

    let mut native = Native::default();

    let warmed = native.analyze(
        &request(vec![configured.clone()])?,
        &options(Duration::from_secs(5)),
    )?;

    assert_eq!(warmed.completion, Completion::Complete);
    assert_eq!(warmed.modules[0].diagnostics.as_slice(), []);

    let broken = definition(
        4,
        include_str!("fixtures/checking/failed/invalid.luau"),
        1,
        "@custom",
    )?;

    configured.analysis.revision = 2;

    configured
        .environment
        .as_mut()
        .expect("environment")
        .declarations[0]
        .definitions
        .push(broken.clone());

    let failed = native.analyze(
        &request(vec![configured.clone()])?,
        &options(Duration::from_secs(5)),
    )?;

    assert_eq!(
        failed.completion,
        Completion::Incomplete(vec![Reason::UnavailableEnvironment])
    );

    assert!(
        failed.modules[0]
            .environment
            .as_ref()
            .expect("environment metadata")
            .diagnostics
            .iter()
            .any(
                |diagnostic| diagnostic.primary.source == broken.source.version
                    && diagnostic.category == DiagnosticCategory::Syntax
            )
    );

    let isolated = native.analyze(
        &request(vec![module(
            2,
            include_str!("fixtures/checking/failed/input.luau"),
            1,
        )?])?,
        &options(Duration::from_secs(5)),
    )?;

    assert_eq!(isolated.completion, Completion::Complete);
    assert_ne!(isolated.modules[0].diagnostics.as_slice(), []);
    configured.analysis.revision = 3;

    configured
        .environment
        .as_mut()
        .expect("environment")
        .declarations[0]
        .definitions = vec![first];

    let restored = native.analyze(
        &request(vec![configured])?,
        &options(Duration::from_secs(5)),
    )?;

    assert_eq!(restored.completion, Completion::Complete);
    assert_eq!(restored.modules[0].diagnostics.as_slice(), []);

    native.finish()
}

#[test]
fn native_interruption_does_not_cache_a_clean_answer() -> io::Result<()> {
    let input = request(vec![module(
        1,
        include_str!("fixtures/checking/interruption/input.luau"),
        4,
    )?])?;

    let mut native = Native::default();
    let cancelled = options(Duration::from_secs(5));
    cancelled.cancellation.cancel();

    assert_eq!(
        native.analyze(&input, &cancelled)?.completion,
        Completion::Cancelled
    );

    assert_eq!(
        native.analyze(&input, &options(Duration::ZERO))?.completion,
        Completion::TimedOut
    );

    let retry = native.analyze(&input, &options(Duration::from_secs(5)))?;
    assert_eq!(retry.completion, Completion::Complete);
    assert_ne!(retry.modules[0].diagnostics.as_slice(), []);

    native.finish()
}

#[test]
fn interrupted_requests_return_promptly_and_allow_retry() -> io::Result<()> {
    let source = "local value = 1\n".repeat(500_000);
    let mut input = request(vec![module(1, &source, 1)?])?;
    input.modules[0].inference.required = false;
    input.modules[0].inference.retained.clear();
    input.modules[0].reporting.ordinary_type_diagnostics = false;
    input.modules[0].environment = None;
    let mut native = Native::default();
    let limited = options(Duration::from_millis(20));
    let started = Instant::now();

    assert_eq!(
        native.analyze(&input, &limited)?.completion,
        Completion::TimedOut
    );

    assert!(started.elapsed() < Duration::from_secs(2));
    let mut entry = module(1, "return 1", 2)?;

    let declarations = (0..100_000).fold(String::new(), |mut text, number| {
        writeln!(text, "declare value{number}: number").expect("string write");

        text
    });

    entry
        .environment
        .as_mut()
        .expect("environment")
        .declarations[0]
        .definitions = vec![definition(2, &declarations, 1, "@custom")?];

    let limited = options(Duration::from_secs(5));
    let cancellation = limited.cancellation.clone();

    let monitor = thread::spawn(move || {
        thread::sleep(Duration::from_millis(20));
        cancellation.cancel();
    });

    let started = Instant::now();
    let interrupted = native.analyze(&request(vec![entry.clone()])?, &limited)?;
    monitor.join().expect("cancellation thread");
    assert_eq!(interrupted.completion, Completion::Cancelled);
    assert!(started.elapsed() < Duration::from_secs(2));
    entry.analysis.revision = 3;

    entry
        .environment
        .as_mut()
        .expect("environment")
        .declarations[0]
        .definitions
        .clear();

    let retry = native.analyze(&request(vec![entry])?, &options(Duration::from_secs(5)))?;
    assert_eq!(retry.completion, Completion::Complete);
    assert_eq!(retry.modules[0].diagnostics.as_slice(), []);

    native.finish()
}
