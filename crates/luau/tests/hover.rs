//! Revision-anchored native inspection and position contracts.

use std::{io, sync::Arc, time::Duration};

use instar_luau::{AnalysisReference, Native, PositionInput, TypeFact};
use instar_shared::{Completion, EventPayload, Knowledge, Reason, Span};

use crate::support::{module, options, request};

fn inspect_builtin_and_positions(
    native: &mut Native,
    reference: &AnalysisReference,
    source: &str,
) -> io::Result<()> {
    let builtin = native
        .inspect(
            &PositionInput {
                analysis: reference.clone(),
                offset: source.find("math.abs").expect("builtin") + 5,
            },
            &options(Duration::from_secs(5)),
        )?
        .occurrence
        .expect("builtin signature");

    assert!(
        matches!(&builtin.type_information, Knowledge::Known(information) if information.display.contains("number"))
    );

    assert!(matches!(builtin.documentation, Knowledge::Known(Some(_))));
    let unicode = source.find('😀').expect("Unicode string");

    assert_eq!(
        native
            .inspect(
                &PositionInput {
                    analysis: reference.clone(),
                    offset: unicode + 1
                },
                &options(Duration::from_secs(5))
            )
            .expect_err("split UTF-8 position")
            .kind(),
        io::ErrorKind::InvalidInput
    );

    assert_eq!(
        native
            .inspect(
                &PositionInput {
                    analysis: reference.clone(),
                    offset: source.len() + 1
                },
                &options(Duration::from_secs(5))
            )
            .expect_err("out of bounds position")
            .kind(),
        io::ErrorKind::InvalidInput
    );

    Ok(())
}

#[test]
fn hover_retains_types_after_checks_and_tracks_revisions() -> io::Result<()> {
    let source = include_str!("fixtures/hover/input.luau");
    let mut input = request(vec![module(1, source, 7)?])?;
    let mut native = Native::default();
    let established = native.analyze(&input, &options(Duration::from_secs(5)))?;

    let reference = established.modules[0]
        .reference
        .clone()
        .expect("established reference");

    let offset = source.rfind("count").expect("count reference");

    let hover = native
        .inspect(
            &PositionInput {
                analysis: reference.clone(),
                offset,
            },
            &options(Duration::from_secs(5)),
        )?
        .occurrence
        .expect("inferred type");

    assert!(
        matches!(&hover.type_information, Knowledge::Known(TypeFact { display, .. }) if display == "number")
    );

    assert_eq!(
        hover.location.source,
        input.modules[0].source.source.version
    );

    assert_eq!(hover.location.source.revision, 7);

    assert_eq!(
        hover.location.span,
        Span {
            start: offset,
            end: offset + "count".len()
        }
    );

    assert_eq!(hover.documentation, Knowledge::Known(None));

    inspect_builtin_and_positions(&mut native, &reference, source)?;

    input.modules[0] = module(1, source, 8)?;
    let established = native.analyze(&input, &options(Duration::from_secs(5)))?;

    let stale = native
        .inspect(
            &PositionInput {
                analysis: reference,
                offset,
            },
            &options(Duration::from_secs(5)),
        )
        .expect_err("expired reference");

    assert_eq!(
        stale
            .get_ref()
            .and_then(|error| error.downcast_ref::<Reason>()),
        Some(&Reason::StaleQuery)
    );

    let reference = established.modules[0]
        .reference
        .clone()
        .expect("new revision");

    assert_eq!(
        native
            .inspect(
                &PositionInput {
                    analysis: reference.clone(),
                    offset
                },
                &options(Duration::from_secs(5))
            )?
            .occurrence
            .expect("new revision type")
            .location
            .source
            .revision,
        8
    );

    let cancelled = options(Duration::from_secs(5));
    cancelled.cancellation.cancel();

    assert_eq!(
        native
            .inspect(
                &PositionInput {
                    analysis: reference,
                    offset
                },
                &cancelled
            )?
            .completion,
        Completion::Cancelled
    );

    native.finish()
}

#[test]
fn interrupted_semantic_work_recovers_in_a_fresh_worker() -> io::Result<()> {
    let source = include_str!("fixtures/hover/input.luau");
    let input = request(vec![module(1, source, 9)?])?;
    let mut native = Native::default();
    let mut interrupted = options(Duration::from_secs(5));
    let cancellation = interrupted.cancellation.clone();

    interrupted.observer = Some(Arc::new(move |event| {
        if matches!(event.payload, EventPayload::Progress(progress) if progress.phase == "Checking")
        {
            cancellation.cancel();
        }
    }));

    assert_eq!(
        native.analyze(&input, &interrupted)?.completion,
        Completion::Cancelled
    );

    assert!(interrupted.cancellation.requested());
    let established = native.analyze(&input, &options(Duration::from_secs(5)))?;

    let recovered = native
        .inspect(
            &PositionInput {
                analysis: established.modules[0]
                    .reference
                    .clone()
                    .expect("recovered reference"),
                offset: source.rfind("count").expect("count reference"),
            },
            &options(Duration::from_secs(5)),
        )?
        .occurrence
        .expect("recovered type");

    assert!(
        matches!(&recovered.type_information, Knowledge::Known(information) if information.display == "number")
    );

    assert_eq!(recovered.location.source.revision, 9);

    native.finish()
}
