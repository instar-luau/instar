//! Revision-anchored native hover and position contracts.

use std::{io, sync::Arc, time::Duration};

use instar_analysis::Options;
use instar_bridge::{Configuration, frontend::Frontend};

#[test]
fn hover_retains_types_after_checks_and_tracks_revisions() -> io::Result<()> {
    let source = include_str!("fixtures/hover/input.luau");
    let configuration = Configuration::new()?;
    let options = Options::new(Duration::from_secs(5));
    let mut frontend = Frontend::default();
    frontend.insert("entry", source, 7, &configuration, &[])?;
    frontend.check(&["entry".to_owned()], &options)?;
    let offset = source.rfind("count").expect("count reference");

    let hover = frontend
        .hover("entry", offset, &options)?
        .expect("inferred type");

    assert_eq!(hover.text, "number");
    assert_eq!(hover.location.module, "entry");
    assert_eq!(hover.location.revision, 7);
    assert_eq!(hover.location.range, [offset, offset + "count".len()]);
    assert!(hover.documentation.is_none());

    let builtin = frontend
        .hover(
            "entry",
            source.find("math.abs").expect("builtin") + 5,
            &options,
        )?
        .expect("builtin signature");

    assert!(builtin.text.contains("number"), "{}", builtin.text);
    assert!(builtin.documentation.is_some());
    let unicode = source.find('😀').expect("Unicode string");

    assert_eq!(
        frontend
            .hover("entry", unicode + 1, &options)
            .expect_err("split UTF-8 position")
            .kind(),
        io::ErrorKind::InvalidInput
    );

    assert_eq!(
        frontend
            .hover("entry", source.len() + 1, &options)
            .expect_err("out of bounds position")
            .kind(),
        io::ErrorKind::InvalidInput
    );

    frontend.insert("entry", source, 8, &configuration, &[])?;

    assert_eq!(
        frontend
            .hover("entry", offset, &options)?
            .expect("new revision")
            .location
            .revision,
        8
    );

    options.cancellation.cancel();

    assert_eq!(
        frontend
            .hover("entry", offset, &options)
            .expect_err("cancelled query")
            .kind(),
        io::ErrorKind::Interrupted
    );

    Ok(())
}

#[test]
fn interrupted_semantic_work_recovers_in_a_fresh_worker() -> io::Result<()> {
    let source = include_str!("fixtures/hover/input.luau");
    let mut frontend = Frontend::default();
    frontend.insert("entry", source, 9, &Configuration::new()?, &[])?;
    let mut options = Options::new(Duration::from_secs(5));
    let cancellation = options.cancellation.clone();

    options.progress = Some(Arc::new(move |progress| {
        if progress.phase == "Checking" {
            cancellation.cancel();
        }
    }));

    let offset = source.rfind("count").expect("count reference");

    assert_eq!(
        frontend
            .hover("entry", offset, &options)
            .expect_err("interrupted native work")
            .kind(),
        io::ErrorKind::Interrupted
    );

    assert!(options.cancellation.requested());

    let recovered = frontend
        .hover("entry", offset, &Options::new(Duration::from_secs(5)))?
        .expect("recovered type");

    assert_eq!(recovered.text, "number");
    assert_eq!(recovered.location.revision, 9);

    Ok(())
}
