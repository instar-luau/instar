//! Semantic declaration and type navigation contracts.

use std::{io, time::Duration};

use instar_luau::{Native, PositionInput};
use instar_shared::{Knowledge, Location, Span};
use serde::Deserialize;

use crate::support::{module, options, request};

#[derive(Deserialize)]
struct Case {
    query: String,
    offset: usize,
    type_definition: bool,
    target: String,
    target_offset: usize,
    length: usize,
}

#[test]
fn definitions_preserve_lexical_identity_and_type_locations() -> io::Result<()> {
    let source = include_str!("fixtures/navigation/input.luau");
    let cases: Vec<Case> = serde_json::from_str(include_str!("fixtures/navigation/expected.json"))?;
    let input = request(vec![module(1, source, 11)?])?;
    let mut native = Native::default();
    let established = native.analyze(&input, &options(Duration::from_secs(5)))?;

    let reference = established.modules[0]
        .reference
        .clone()
        .expect("established reference");

    for case in cases {
        let offset = source.find(&case.query).expect("query") + case.offset;
        let target = source.find(&case.target).expect("target") + case.target_offset;

        let occurrence = native
            .inspect(
                &PositionInput {
                    analysis: reference.clone(),
                    offset,
                },
                &options(Duration::from_secs(5)),
            )?
            .occurrence
            .expect("navigation occurrence");

        let locations = if case.type_definition {
            occurrence.type_definitions
        } else {
            occurrence.definitions
        };

        assert_eq!(
            locations,
            Knowledge::Known(vec![Location::new(
                &input.modules[0].source.source,
                Span {
                    start: target,
                    end: target + case.length
                }
            )?]),
            "{}",
            case.query
        );
    }

    native.finish()
}
