//! Semantic declaration and type navigation contracts.

use std::{io, time::Duration};

use instar_analysis::{Location, Options};
use instar_bridge::{Configuration, frontend::Frontend};
use serde::Deserialize;

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
    let mut frontend = Frontend::default();
    frontend.insert("entry", source, 11, &Configuration::new()?, &[])?;
    let options = Options::new(Duration::from_secs(5));

    for case in cases {
        let offset = source.find(&case.query).expect("query") + case.offset;
        let target = source.find(&case.target).expect("target") + case.target_offset;

        assert_eq!(
            frontend.definition("entry", offset, case.type_definition, &options)?,
            vec![Location {
                module: "entry".to_owned(),
                revision: 11,
                range: [target, target + case.length]
            }],
            "{}",
            case.query
        );
    }

    Ok(())
}
