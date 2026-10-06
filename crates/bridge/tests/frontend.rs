//! Host/native analysis agreement and invalidation fixtures.

use std::{io, time::Duration};

use instar_analysis::Options;

use instar_bridge::{
    Configuration,
    frontend::{Frontend, Site},
};

#[test]
fn host_targets_are_authoritative_for_lexical_loader_aliases() -> io::Result<()> {
    let configuration = Configuration::new()?;
    let mut frontend = Frontend::new()?;
    let source = "local loader=require\nloader('./not-on-disk')";
    let start = source.find("loader(").expect("loader call");
    let argument = source.find("'./").expect("argument");

    let site = Site {
        call: [start, source.len()],
        argument: [argument, source.len() - 1],
        static_request: true,
        target: Some("contextual-target".to_owned()),
    };

    frontend.insert(
        "entry",
        source,
        7,
        &configuration,
        std::slice::from_ref(&site),
    )?;

    frontend.insert("contextual-target", "return 1", 3, &configuration, &[])?;
    let links = frontend.parse("entry", &Options::new(Duration::from_secs(5)))?;
    assert_eq!(links.len(), 1);
    assert_eq!(links[0].module, "entry");
    assert_eq!(links[0].revision, 7);
    assert_eq!(links[0].argument, site.argument);
    assert_eq!(links[0].target, "contextual-target");
    frontend.invalidate(&["entry".to_owned(), "contextual-target".to_owned()])?;

    assert_eq!(
        frontend
            .parse("entry", &Options::new(Duration::from_secs(5)))
            .expect_err("removed source")
            .kind(),
        io::ErrorKind::NotFound
    );

    frontend.insert("entry", "return 2", 8, &configuration, &[])?;

    assert_eq!(
        frontend
            .parse("entry", &Options::new(Duration::from_secs(5)))?
            .len(),
        0
    );

    Ok(())
}

#[test]
fn static_argument_disagreement_is_an_error() -> io::Result<()> {
    let configuration = Configuration::new()?;
    let mut frontend = Frontend::new()?;
    let source = "require('./target')";

    frontend.insert(
        "entry",
        source,
        1,
        &configuration,
        &[Site {
            call: [0, source.len()],
            argument: [9, source.len() - 2],
            static_request: true,
            target: None,
        }],
    )?;

    assert!(
        frontend
            .parse("entry", &Options::new(Duration::from_secs(5)))
            .expect_err("mismatched host argument")
            .to_string()
            .contains("static require argument disagreement")
    );

    Ok(())
}

#[test]
fn missing_host_targets_and_invalid_ranges_are_not_guessed() -> io::Result<()> {
    let configuration = Configuration::new()?;
    let mut frontend = Frontend::new()?;
    let source = "require('./target')";

    let site = Site {
        call: [0, source.len()],
        argument: [8, source.len() - 1],
        static_request: true,
        target: Some("missing-context".to_owned()),
    };

    frontend.insert(
        "entry",
        source,
        1,
        &configuration,
        std::slice::from_ref(&site),
    )?;

    assert_eq!(
        frontend
            .parse("entry", &Options::new(Duration::from_secs(5)))
            .expect_err("missing host target")
            .kind(),
        io::ErrorKind::NotFound
    );

    let mut invalid = site.clone();
    invalid.argument[1] = source.len() + 1;

    assert_eq!(
        frontend
            .insert("invalid", source, 1, &configuration, &[invalid])
            .expect_err("invalid range")
            .kind(),
        io::ErrorKind::InvalidInput
    );

    let mut invalid = site.clone();
    invalid.static_request = false;

    assert_eq!(
        frontend
            .insert("invalid", source, 1, &configuration, &[invalid])
            .expect_err("dynamic site cannot have target")
            .kind(),
        io::ErrorKind::InvalidInput
    );

    assert_eq!(
        frontend
            .insert("invalid", source, 1, &configuration, &[site.clone(), site])
            .expect_err("duplicate site")
            .kind(),
        io::ErrorKind::InvalidInput
    );

    Ok(())
}
