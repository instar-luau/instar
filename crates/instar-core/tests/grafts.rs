use instar_core::{
    format::{Options, configuration::Configuration},
    graft::Graft,
};
use std::fs;

#[path = "support/graft.rs"]
mod support;
use support::{fixture, module};


#[test]
fn formatting_uses_the_host_renderer_and_configuration_pipeline() {
    let reply = r#"{"version":1,"document":{"sequence":[{"text":"local value ="},{"indent":{"sequence":["hard",{"host":{"start":14,"end":15,"parse":"expression"}}]}}]}}"#;
    let directory = fixture(reply, "instar_format");

    fs::write(
        directory.path().join("instar.toml"),
        "[grafts]\nexample = 'graft.toml'",
    )
    .unwrap();

    let configuration =
        Configuration::discover(&directory.path().join("source.luau"), None).unwrap();

    let output = configuration.format(b"local value=1").unwrap();
    assert_eq!(output, b"local value =\n\t1\n");
    assert_eq!(configuration.format(&output).unwrap(), output);
}

#[test]
fn malformed_layouts_and_syntax_changes_are_rejected() {
    for reply in [
        "not json",
        r#"{"version":2,"document":"empty"}"#,
        r#"{"version":1,"document":{"source":[0,999]}}"#,
        r#"{"version":1,"document":{"text":"return 2"}}"#,
    ] {
        let directory = fixture(reply, "instar_format");
        let graft = Graft::load(&directory.path().join("graft.toml"), "example").unwrap();

        assert!(
            graft.format(b"return 1", &Options::default()).is_err(),
            "{reply}"
        );
    }
}

#[test]
fn lint_findings_validate_source_ranges() {
    let directory = fixture(
        r#"[{"rule":"example","message":"reported","start":0,"end":6}]"#,
        "instar_lint",
    );

    let graft = Graft::load(&directory.path().join("graft.toml"), "example").unwrap();
    assert_eq!(graft.lint(b"return 1").unwrap()[0].rule, "example");
    assert!(graft.lint(b"x").is_err());
}

#[test]
fn traps_and_out_of_bounds_guest_responses_are_errors() {
    let reply = r#"{"version":1,"document":{"source":[0,8]}}"#;
    let directory = fixture(reply, "instar_format");
    let artifact = directory.path().join("module.wasm");
    let manifest = directory.path().join("graft.toml");
    let mut trapped = module(reply, "instar_format");

    let position = trapped
        .windows(6)
        .position(|bytes| bytes == [5, 0, 0x41, 0x80, 0x20, 0x0b])
        .unwrap();

    trapped[position..position + 6].copy_from_slice(&[5, 0, 0, 1, 1, 0x0b]);
    fs::write(&artifact, trapped).unwrap();

    assert!(
        Graft::load(&manifest, "example")
            .unwrap()
            .format(b"return 1", &Options::default())
            .is_err()
    );

    let mut invalid = module(reply, "instar_format");

    let header = [
        12u32.to_le_bytes(),
        u32::try_from(reply.len()).unwrap().to_le_bytes(),
        1u32.to_le_bytes(),
    ]
    .concat();

    let position = invalid
        .windows(12)
        .position(|bytes| bytes == header)
        .unwrap();

    invalid[position..position + 4].copy_from_slice(&u32::MAX.to_le_bytes());
    fs::write(&artifact, invalid).unwrap();

    assert!(
        Graft::load(&manifest, "example")
            .unwrap()
            .format(b"return 1", &Options::default())
            .is_err()
    );
}

#[test]
fn graft_layouts_preserve_suppression_and_comments() {
    let directory = fixture(
        r#"{"version":1,"document":{"text":"-- instar: format off\nlocal x = 1\n-- instar: format on"}}"#,
        "instar_format",
    );

    let graft = Graft::load(&directory.path().join("graft.toml"), "example").unwrap();

    let error = graft
        .format(
            b"-- instar: format off\nlocal  x=1\n-- instar: format on",
            &Options::default(),
        )
        .unwrap_err();

    assert!(error.to_string().contains("suppressed"), "{error}");
}

#[test]
fn manifest_identity_exports_and_entry_boundaries_are_checked() {
    let directory = fixture(r#"{"version":1,"document":"empty"}"#, "instar_format");
    let path = directory.path().join("graft.toml");
    assert!(Graft::load(&path, "another").is_err());

    for manifest in [
        "name='example'\nversion=2\nruntime='wasm'\nentry='module.wasm'\nformat=true",
        "name='example'\nversion=1\nruntime='wasm'\nentry='../module.wasm'\nformat=true",
        "name='example'\nversion=1\nruntime='wasm'\nentry='module.wasm'\nlint=true",
    ] {
        fs::write(&path, manifest).unwrap();
        assert!(Graft::load(&path, "example").is_err());
    }
}

#[test]
fn runtime_is_required_and_validated() {
    let directory = fixture("[]", "instar_lint");
    let path = directory.path().join("graft.toml");

    for runtime in ["", "runtime='unknown'\n"] {
        fs::write(
            &path,
            format!("name='example'\nversion=1\n{runtime}entry='module.wasm'\nlint=true"),
        )
        .unwrap();

        let error = Graft::load(&path, "example").unwrap_err();

        assert!(
            error.to_string().contains("runtime") || error.to_string().contains("unknown"),
            "{error}"
        );
    }

    {
        let runtime = "wasm";
        fs::write(
            &path,
            format!("name='example'\nversion=1\nruntime='{runtime}'\nentry='../module'\nlint=true"),
        )
        .unwrap();

        assert!(
            Graft::load(&path, "example")
                .unwrap_err()
                .to_string()
                .contains("inside")
        );
    }
}