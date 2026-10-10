//! Captured host/native agreement and guarded invalidation fixtures.

use std::{io, slice, time::Duration};

use instar_luau::{
    FactsInput, ImportIdentity, ImportOutcome, ImportRequest, ImportSite, Native, ResolvedImport,
    SemanticCategory,
};

use instar_shared::{Completion, Reason, Span};

use crate::support::{location, module, options, request};

#[test]
fn host_targets_are_authoritative_for_lexical_loader_aliases() -> io::Result<()> {
    let mut native = Native::default();
    let source = include_str!("fixtures/analysis/host/input.luau");
    let mut entry = module(1, source, 7)?;
    let dependency = module(2, "return 1", 3)?;
    let start = source.find("loader(").expect("loader call");
    let argument = source.find("'./").expect("argument");

    let site = ImportSite {
        identity: ImportIdentity(1),
        call: location(&entry, [start, source.len()])?,
        argument: location(&entry, [argument, source.len() - 1])?,
        request: ImportRequest::Static("./not-on-disk".to_owned()),
    };

    entry.imports.push(ResolvedImport {
        site: site.clone(),
        outcome: ImportOutcome::Module(dependency.analysis.module),
    });

    let input = request(vec![entry, dependency])?;
    let established = native.analyze(&input, &options(Duration::from_secs(5)))?;
    assert_eq!(established.completion, Completion::Complete);

    assert_eq!(
        established.modules[0].parsed.imports.as_slice(),
        slice::from_ref(&site)
    );

    assert_eq!(
        established.modules[0].parsed.input.source.version.revision,
        7
    );

    assert_eq!(
        established.modules[0].parsed.imports[0].argument,
        site.argument
    );

    let reference = established.modules[0]
        .reference
        .clone()
        .expect("established reference");

    let replacement = request(vec![module(1, "return 2", 8)?])?;
    let replaced = native.analyze(&replacement, &options(Duration::from_secs(5)))?;
    assert_eq!(replaced.modules[0].parsed.imports.as_slice(), []);

    let stale = native
        .facts(
            &FactsInput {
                analysis: reference,
                categories: vec![SemanticCategory::Bindings],
            },
            &options(Duration::from_secs(5)),
        )
        .expect_err("removed captured revision");

    assert_eq!(
        stale
            .get_ref()
            .and_then(|error| error.downcast_ref::<Reason>()),
        Some(&Reason::StaleQuery)
    );

    native.finish()
}

#[test]
fn static_argument_disagreement_is_an_error() -> io::Result<()> {
    let mut native = Native::default();
    let source = include_str!("fixtures/analysis/static/input.luau");
    let mut entry = module(1, source, 1)?;

    entry.imports.push(ResolvedImport {
        site: ImportSite {
            identity: ImportIdentity(1),
            call: location(&entry, [0, source.len()])?,
            argument: location(&entry, [9, source.len() - 2])?,
            request: ImportRequest::Static("dependency".to_owned()),
        },
        outcome: ImportOutcome::Unresolved,
    });

    assert!(
        native
            .analyze(&request(vec![entry])?, &options(Duration::from_secs(5)))
            .expect_err("mismatched host argument")
            .to_string()
            .contains("static require argument disagreement")
    );

    native.finish()
}

#[test]
fn missing_host_targets_and_invalid_ranges_are_not_guessed() -> io::Result<()> {
    let source = include_str!("fixtures/analysis/static/input.luau");
    let mut entry = module(1, source, 1)?;
    let missing = module(2, "return 1", 1)?.analysis.module;

    let site = ImportSite {
        identity: ImportIdentity(1),
        call: location(&entry, [0, source.len()])?,
        argument: location(&entry, [8, source.len() - 1])?,
        request: ImportRequest::Static("dependency".to_owned()),
    };

    entry.imports.push(ResolvedImport {
        site: site.clone(),
        outcome: ImportOutcome::Module(missing),
    });

    let mut native = Native::default();

    assert_eq!(
        native
            .analyze(
                &request(vec![entry.clone()])?,
                &options(Duration::from_secs(5))
            )
            .expect_err("missing captured target")
            .kind(),
        io::ErrorKind::InvalidData
    );

    entry.imports[0].site.argument.span = Span {
        start: 8,
        end: source.len() + 1,
    };

    assert_eq!(
        native
            .analyze(
                &request(vec![entry.clone()])?,
                &options(Duration::from_secs(5))
            )
            .expect_err("invalid range")
            .kind(),
        io::ErrorKind::InvalidInput
    );

    entry.imports[0].site = site.clone();
    entry.imports[0].site.request = ImportRequest::Nonconstant;

    let invalid = native
        .analyze(
            &request(vec![entry.clone()])?,
            &options(Duration::from_secs(5)),
        )
        .expect_err("nonconstant target");

    assert_eq!(invalid.kind(), io::ErrorKind::InvalidData);
    assert!(invalid.to_string().contains("outcome"));
    entry.imports[0].site = site;
    entry.imports[0].outcome = ImportOutcome::Unresolved;
    entry.imports.push(entry.imports[0].clone());

    let invalid = native
        .analyze(&request(vec![entry])?, &options(Duration::from_secs(5)))
        .expect_err("duplicate site");

    assert_eq!(invalid.kind(), io::ErrorKind::InvalidData);
    assert!(invalid.to_string().contains("duplicated"));

    native.finish()
}
