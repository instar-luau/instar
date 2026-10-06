//! Native checker diagnostics and interruption boundary fixtures.

use instar_bridge::{
    Configuration,
    frontend::{Definition, Frontend, Site},
};

use instar_check::{Completion, Kind, Options, Reason};
use std::{io, time::Duration};

#[test]
fn native_type_codes_locations_and_related_data_are_retained() -> io::Result<()> {
    let configuration = Configuration::new()?;
    let mut frontend = Frontend::new()?;
    let source = "--!strict\ntype Value = number\ntype Value = string\nreturn 1";
    frontend.insert("entry", source, 8, &configuration, &[])?;
    let result = frontend.check(&["entry".to_owned()], &Options::new(Duration::from_secs(5)))?;
    assert_eq!(result.completion, Completion::Complete);

    let diagnostic = result
        .diagnostics
        .iter()
        .find(|diagnostic| !diagnostic.related.is_empty())
        .expect("duplicate definition with previous location");

    assert!(matches!(diagnostic.kind, Kind::Type { .. }));

    assert!(
        diagnostic
            .kind
            .native_code()
            .is_some_and(|code| code >= 1000)
    );

    assert_eq!(diagnostic.location.module, "entry");
    assert_eq!(diagnostic.location.revision, 8);
    assert!(diagnostic.location.range[0] > diagnostic.related[0].location.range[0]);
    assert_eq!(diagnostic.related[0].location.revision, 8);

    Ok(())
}

#[test]
fn lexical_loader_targets_supply_real_imported_types() -> io::Result<()> {
    let configuration = Configuration::new()?;
    let mut frontend = Frontend::new()?;
    let source = "--!strict\nlocal loader = require\nlocal imported = loader('./not-on-disk')\nlocal number: number = imported\nreturn number";
    let call = source.find("loader(").expect("loader call");
    let argument = source.find("'./").expect("loader argument");
    let end = source[argument..].find(')').expect("call end") + argument;

    frontend.insert(
        "entry",
        source,
        1,
        &configuration,
        &[Site {
            call: [call, end + 1],
            argument: [argument, end],
            static_request: true,
            target: Some("dependency".to_owned()),
        }],
    )?;

    frontend.insert("dependency", "return 'string'", 2, &configuration, &[])?;
    let result = frontend.check(&["entry".to_owned()], &Options::new(Duration::from_secs(5)))?;
    assert_eq!(result.completion, Completion::Complete);

    assert!(
        result
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.location.module == "entry"
                && matches!(diagnostic.kind, Kind::Type { .. }))
    );

    Ok(())
}

#[test]
fn failed_definitions_do_not_pollute_other_environments() -> io::Result<()> {
    let configuration = Configuration::new()?;
    let mut frontend = Frontend::new()?;

    frontend.insert(
        "configured",
        "--!strict\nreturn configured",
        1,
        &configuration,
        &[],
    )?;

    frontend.definitions(
        "configured",
        &[
            Definition {
                name: "first".to_owned(),
                revision: 1,
                text: "declare configured: number".to_owned(),
            },
            Definition {
                name: "broken".to_owned(),
                revision: 1,
                text: "declare broken:".to_owned(),
            },
        ],
    )?;

    let failed = frontend.check(
        &["configured".to_owned()],
        &Options::new(Duration::from_secs(5)),
    )?;

    assert_eq!(
        failed.completion,
        Completion::Incomplete(Reason::Environment)
    );

    assert!(
        failed
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.location.module == "broken"
                && matches!(diagnostic.kind, Kind::Syntax { .. }))
    );

    frontend.insert(
        "isolated",
        "--!strict\nreturn configured",
        1,
        &configuration,
        &[],
    )?;

    let isolated = frontend.check(
        &["isolated".to_owned()],
        &Options::new(Duration::from_secs(5)),
    )?;

    assert_eq!(isolated.completion, Completion::Complete);
    assert_ne!(isolated.diagnostics, Vec::new());

    frontend.definitions(
        "configured",
        &[Definition {
            name: "first".to_owned(),
            revision: 1,
            text: "declare configured: number".to_owned(),
        }],
    )?;

    assert_eq!(
        frontend
            .check(
                &["configured".to_owned()],
                &Options::new(Duration::from_secs(5))
            )?
            .diagnostics,
        Vec::new()
    );

    Ok(())
}

#[test]
fn native_interruption_does_not_cache_a_clean_answer() -> io::Result<()> {
    let configuration = Configuration::new()?;
    let mut frontend = Frontend::new()?;

    frontend.insert(
        "entry",
        "--!strict\nlocal number: number = 'wrong'\nreturn number",
        4,
        &configuration,
        &[],
    )?;

    let options = Options::new(Duration::from_secs(5));
    options.cancellation.cancel();

    assert_eq!(
        frontend.check(&["entry".to_owned()], &options)?.completion,
        Completion::Incomplete(Reason::Cancelled)
    );

    assert_eq!(
        frontend
            .check(&["entry".to_owned()], &Options::new(Duration::ZERO))?
            .completion,
        Completion::Incomplete(Reason::Timeout)
    );

    let retry = frontend.check(&["entry".to_owned()], &Options::new(Duration::from_secs(5)))?;
    assert_eq!(retry.completion, Completion::Complete);
    assert_ne!(retry.diagnostics, Vec::new());

    Ok(())
}
