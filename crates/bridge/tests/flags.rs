//! Typed native flags and worker lifetime contracts.
use std::{collections::BTreeMap, io, time::Duration};

use instar_analysis::{Kind, Options};

use instar_bridge::{
    Configuration,
    flags::{self, Value},
    frontend::{Frontend, Site},
};

#[test]
fn flags_validate_registry_names_types_and_aliases() {
    for (name, value) in [
        ("MissingInstarFlag", Value::Boolean(true)),
        ("LuauRecursionLimit", Value::Boolean(true)),
        ("LuauExportValueSyntax", Value::Integer(1)),
    ] {
        assert_eq!(
            flags::validate(&BTreeMap::from([(name.to_owned(), value)]))
                .expect_err("invalid flag")
                .kind(),
            io::ErrorKind::InvalidData
        );
    }

    assert!(
        flags::validate(&BTreeMap::from([
            ("LuauExportValueSyntax".to_owned(), Value::Boolean(true)),
            ("LuauExportValueSyntax5".to_owned(), Value::Boolean(true)),
        ]))
        .is_err()
    );
}

#[test]
fn integer_changes_restart_cached_analysis_and_defaults_are_restored() -> io::Result<()> {
    let mut frontend = Frontend::new()?;
    let source = format!("return {}1{}", "(".repeat(40), ")".repeat(40));
    frontend.insert("entry", &source, 1, &Configuration::new()?, &[])?;
    let options = Options::new(Duration::from_secs(5));
    let roots = ["entry".to_owned()];
    assert_eq!(frontend.check(&roots, &options)?.diagnostics, Vec::new());

    frontend.flags(
        "entry",
        &flags::validate(&BTreeMap::from([(
            "LuauRecursionLimit".to_owned(),
            Value::Integer(20),
        )]))?,
    )?;

    assert!(
        frontend
            .check(&roots, &options)?
            .diagnostics
            .iter()
            .any(|diagnostic| matches!(diagnostic.kind, Kind::Syntax { .. }))
    );

    frontend.flags("entry", &flags::Overrides::default())?;
    assert_eq!(frontend.check(&roots, &options)?.diagnostics, Vec::new());

    Ok(())
}

#[test]
fn reachable_modules_require_consistent_effective_flags() -> io::Result<()> {
    let mut frontend = Frontend::new()?;
    let configuration = Configuration::new()?;
    let source = "return require('dependency')";

    frontend.insert(
        "entry",
        source,
        1,
        &configuration,
        &[Site {
            call: [7, source.len()],
            argument: [15, source.len() - 1],
            static_request: true,
            target: Some("dependency".to_owned()),
        }],
    )?;

    frontend.insert("dependency", "return 1", 1, &configuration, &[])?;
    frontend.insert("unreachable", "return 1", 1, &configuration, &[])?;
    let options = Options::new(Duration::from_secs(5));
    let roots = ["entry".to_owned()];

    let changed = flags::validate(&BTreeMap::from([(
        "LuauRecursionLimit".to_owned(),
        Value::Integer(20),
    )]))?;

    frontend.flags("unreachable", &changed)?;

    frontend.flags(
        "entry",
        &flags::validate(&BTreeMap::from([(
            "LuauRecursionLimit".to_owned(),
            Value::Integer(1000),
        )]))?,
    )?;

    assert_eq!(frontend.check(&roots, &options)?.diagnostics, Vec::new());
    frontend.flags("dependency", &changed)?;

    assert!(
        frontend
            .check(&roots, &options)
            .expect_err("conflicting reachable flags")
            .to_string()
            .contains("conflicting")
    );

    frontend.flags("entry", &changed)?;
    assert_eq!(frontend.check(&roots, &options)?.diagnostics, Vec::new());

    frontend.flags(
        "entry",
        &flags::validate(&BTreeMap::from([(
            "LuauExportValueSyntax".to_owned(),
            Value::Boolean(true),
        )]))?,
    )?;

    frontend.flags(
        "dependency",
        &flags::validate(&BTreeMap::from([(
            "LuauExportValueSyntax5".to_owned(),
            Value::Boolean(true),
        )]))?,
    )?;

    assert_eq!(frontend.check(&roots, &options)?.diagnostics, Vec::new());

    Ok(())
}
