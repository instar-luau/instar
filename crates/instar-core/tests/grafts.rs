//! Graft execution, protocol validation, and transformation behavior.

use instar_core::{
    graft::{self, Graft, Manifest},
    project::Configuration,
    project::configuration::{InstarConfig, format::Options},
};

use std::fs;

#[path = "support/graft.rs"]
mod support;

use support::{fixture, module};

#[path = "support/native.rs"]
mod native;

#[test]
fn project_configuration_validates_dependencies_and_metadata() {
    for entries in [
        "local={path='../local-project'}\nremote={repo='owner/project',version='^0.2.0'}",
        "local={path='../local-project',configuration={factory={create='fluid.create'}}}",
        "remote={repo='owner/project',version='0.2.1',configuration={factory={create='fluid.create'}}}",
    ] {
        assert!(InstarConfig::parse(&format!("[grafts]\n{entries}")).is_ok());
    }

    for entries in [
        "example='path'",
        "example={path=''}",
        "example={repo='owner/project'}",
        "example={repo='owner/project',version='invalid'}",
        "example={repo='../project',version='0.2.1'}",
        "example={repo='owner/project/extra',version='0.2.1'}",
        "example={repo='owner',version='0.2.1'}",
        "example={path='.',repo='owner/project',version='0.2.1'}",
        "example={path='.',unknown=true}",
        "example={path='.',configuration=true}",
        "'../escape'={path='.'}",
    ] {
        assert!(
            InstarConfig::parse(&format!("[grafts]\n{entries}")).is_err(),
            "{entries}"
        );
    }

    let metadata = "name='example'\nprotocol=1\nruntime='wasm'\nentry='dist/module.wasm'\nformat=true\nlint=true\ncompile=true\n";
    assert!(Manifest::parse(metadata).is_ok());
    assert!(Manifest::parse(&format!("{metadata}extensions=['custom']\n")).is_ok());

    for invalid in [
        format!("{metadata}version='0.2.1'\n"),
        metadata.replace("protocol=1\n", ""),
        metadata.replace("protocol=1", "protocol=2"),
        metadata.replace("dist/module.wasm", "../module.wasm"),
        metadata.replace("runtime='wasm'\n", ""),
        metadata.replace("true", "false"),
        format!("{metadata}extensions=['lua']\n"),
        format!("{metadata}extensions=['custom','custom']\n"),
        format!(
            "{}extensions=['custom']\n",
            metadata.replace("compile=true\n", "")
        ),
    ] {
        assert!(Manifest::parse(&invalid).is_err(), "{invalid}");
    }
}

#[test]
fn local_projects_resolve_relative_to_the_declaring_configuration() {
    let directory = native::fixture("[]");

    let consumer = directory.path().join("consumer");
    fs::create_dir(&consumer).unwrap();

    fs::write(
        consumer.join("instar.toml"),
        "[grafts]\nexample={path='..'}",
    )
    .unwrap();

    fs::create_dir(consumer.join("nested")).unwrap();

    let configuration =
        Configuration::discover(&consumer.join("nested/source.luau"), None).unwrap();

    assert_eq!(configuration.grafts.len(), 1);

    assert_eq!(
        configuration.grafts[0].manifest().canonicalize().unwrap(),
        directory.path().join("graft.toml").canonicalize().unwrap()
    );

    fs::write(
        directory.path().join("instar.toml"),
        "[grafts]\nexample={repo='owner/project',version='0.2.1'}",
    )
    .unwrap();

    assert_eq!(
        Configuration::discover(&consumer.join("nested/source.luau"), None)
            .unwrap()
            .grafts
            .len(),
        1
    );

    let mut sources = instar_core::source::SourceStore::default();

    let source = sources
        .open(&consumer.join("nested/source.luau"), 1, "return 1")
        .unwrap();

    assert_eq!(
        instar_core::lint::analyze(
            &mut instar_core::analysis::Session::default(),
            &mut sources,
            source.path()
        )
        .unwrap()
        .findings,
        []
    );

    assert_eq!(graft::install(&consumer).unwrap().len(), 1);
    assert!(graft::install(&consumer.join("missing.toml")).is_err());
    fs::write(consumer.join("instar.toml"), "[grafts]\nwrong={path='..'}").unwrap();
    assert!(Configuration::discover(&consumer.join("source.luau"), None).is_err());
}

#[test]
fn project_configuration_rejects_conflicting_frontends() {
    let first = native::fixture("[]");
    let second = native::fixture("[]");

    fs::write(
        first.path().join("graft.toml"),
        "name='first'\nprotocol=1\nruntime='native'\ncompile=true\nextensions=['custom']\n",
    )
    .unwrap();

    fs::write(
        second.path().join("graft.toml"),
        "name='second'\nprotocol=1\nruntime='native'\ncompile=true\nextensions=['custom']\n",
    )
    .unwrap();

    let directory = tempfile::tempdir().unwrap();

    fs::write(
        directory.path().join("instar.toml"),
        format!(
            "[grafts]\nfirst={{path='{}'}}\nsecond={{path='{}'}}\n",
            first.path().to_string_lossy().replace('\\', "/"),
            second.path().to_string_lossy().replace('\\', "/"),
        ),
    )
    .unwrap();

    let error = Configuration::discover(&directory.path().join("source.custom"), None)
        .err()
        .expect("conflicting frontends accepted");

    assert!(error.to_string().contains("multiple grafts own .custom"));
}

#[test]
fn project_configuration_overrides_graft_defaults() {
    let directory =
        native::fixture(r#"[{"rule":"example","message":"table:fluid.create","start":0,"end":8}]"#);

    let manifest = directory.path().join("graft.toml");

    fs::write(
        &manifest,
        fs::read_to_string(&manifest).unwrap()
            + "[configuration.factory]\nbackend='table'\ncreate='default.create'\n",
    )
    .unwrap();

    fs::write(
        directory.path().join("instar.toml"),
        "[grafts.example]\npath='.'\n[grafts.example.configuration.factory]\ncreate='fluid.create'\n",
    )
    .unwrap();

    fs::write(
        directory.path().join("request.json"),
        r#"{"version":1,"hook":"lint","source":"return 1","configuration":{"factory":{"backend":"table","create":"fluid.create"}},"settings":null}"#,
    )
    .unwrap();

    let configuration =
        Configuration::discover(&directory.path().join("source.luau"), None).unwrap();

    let findings = configuration.grafts[0].lint(b"return 1").unwrap();

    assert_eq!(findings[0].message, "table:fluid.create");
}

#[test]
fn formatting_uses_the_host_renderer_and_configuration_pipeline() {
    let reply = r#"{"version":1,"document":{"sequence":[{"text":"local value ="},{"indent":{"sequence":["hard",{"host":{"start":14,"end":15,"parse":"expression"}}]}}]}}"#;
    let directory = fixture(reply, "instar_format");

    fs::write(
        directory.path().join("instar.toml"),
        "[grafts]\nexample = { path = '.' }",
    )
    .unwrap();

    let configuration =
        Configuration::discover(&directory.path().join("source.luau"), None).unwrap();

    let path = directory.path().join("source.luau");
    let output = configuration.format(&path, b"local value=1").unwrap();
    assert_eq!(output, b"local value =\n\t1\n");
    assert_eq!(configuration.format(&path, &output).unwrap(), output);
}

#[test]
fn frontend_formatting_bypasses_luau_parsing() {
    let reply = serde_json::json!({
        "version": 1,
        "document": {
            "template": {
                "source": "const function Example()\nreturn __markup__\nend",
                "replacements": [{
                    "marker": "__markup__",
                    "document": {"source": [32, 41]},
                }],
            },
        },
    });

    let directory = native::fixture(&reply.to_string());

    fs::write(
        directory.path().join("graft.toml"),
        "name='example'\nprotocol=1\nruntime='native'\nformat=true\ncompile=true\nextensions=['custom']\n",
    )
    .unwrap();

    fs::write(
        directory.path().join("instar.toml"),
        "[grafts]\nexample={path='.'}\n[format.indentation]\nstyle='spaces'",
    )
    .unwrap();

    let path = directory.path().join("source.custom");
    let configuration = Configuration::discover(&path, None).unwrap();

    assert_eq!(
        configuration
            .format(&path, b"const function Example()\nreturn <frame />\nend")
            .unwrap(),
        b"const function Example()\n    return <frame />\nend\n"
    );
}

#[test]
fn frontend_analysis_lowers_source_and_maps_diagnostics() {
    let original = "<frame />";
    let generated = "local value: string = 1";

    let reply = serde_json::json!({
        "version": 1,
        "source": generated,
        "dependencies": [],
        "mappings": [{
            "start": 0,
            "end": generated.len(),
            "original_start": 0,
            "original_end": original.len(),
        }],
    });

    let directory = native::fixture(&reply.to_string());

    fs::write(
        directory.path().join("graft.toml"),
        "name='example'\nprotocol=1\nruntime='native'\ncompile=true\nextensions=['custom']\n",
    )
    .unwrap();

    fs::write(
        directory.path().join("instar.toml"),
        "[grafts]\nexample={path='.'}",
    )
    .unwrap();

    let path = directory.path().join("source.custom");
    let mut sources = instar_core::source::SourceStore::default();
    sources.open(&path, 1, original).unwrap();

    let report = instar_core::analysis::Session::default()
        .analyze(
            &mut sources,
            std::slice::from_ref(&path),
            &instar_core::analysis::Options::default(),
        )
        .unwrap();

    assert_eq!(report.diagnostics.len(), 1);
    assert!(!report.diagnostics[0].message.starts_with("SyntaxError:"));
    assert_eq!(report.diagnostics[0].line, 0);
    assert_eq!(report.diagnostics[0].end_line, 0);
    assert!(report.diagnostics[0].column <= u32::try_from(original.len()).unwrap());
    assert!(report.diagnostics[0].end_column <= u32::try_from(original.len()).unwrap());
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

    fs::write(
        directory.path().join("instar.toml"),
        "[grafts]\nexample = { path = '.' }\n[lint.rules]\n'example/example' = 'deny'\n",
    )
    .unwrap();

    let mut sources = instar_core::source::SourceStore::default();

    let source = sources
        .open(&directory.path().join("source.luau"), 1, "return 1")
        .unwrap();

    let report = instar_core::lint::analyze(
        &mut instar_core::analysis::Session::default(),
        &mut sources,
        source.path(),
    )
    .unwrap();

    assert_eq!(report.findings.len(), 1);
    assert_eq!(report.findings[0].rule, "example/example");

    assert_eq!(
        report.findings[0].level,
        instar_core::lint::configuration::Level::Deny
    );

    let source = sources
        .update(&source, 2, "-- instar: allow(example/example)\nreturn 1")
        .unwrap();

    let report = instar_core::lint::analyze(
        &mut instar_core::analysis::Session::default(),
        &mut sources,
        source.path(),
    )
    .unwrap();

    assert_eq!(report.findings, []);
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
        "name='example'\nprotocol=2\nruntime='wasm'\nentry='module.wasm'\nformat=true",
        "name='example'\nprotocol=1\nruntime='wasm'\nentry='../module.wasm'\nformat=true",
        "name='example'\nprotocol=1\nruntime='wasm'\nentry='module.wasm'\nlint=true",
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
            format!("name='example'\nprotocol=1\n{runtime}entry='module.wasm'\nlint=true"),
        )
        .unwrap();

        let error = Graft::load(&path, "example").unwrap_err();

        assert!(
            error.to_string().contains("runtime") || error.to_string().contains("unknown"),
            "{error}"
        );
    }

    for runtime in ["wasm"] {
        fs::write(
            &path,
            format!(
                "name='example'\nprotocol=1\nruntime='{runtime}'\nentry='../module'\nlint=true"
            ),
        )
        .unwrap();

        assert!(
            Graft::load(&path, "example")
                .unwrap_err()
                .to_string()
                .contains("inside")
        );
    }

    fs::write(
        &path,
        "name='example'\nprotocol=1\nruntime='native'\nentry='module'\nlint=true",
    )
    .unwrap();

    assert!(
        Graft::load(&path, "example")
            .unwrap_err()
            .to_string()
            .contains("must not define an entry")
    );

    for runtime in ["wasm"] {
        fs::write(
            &path,
            format!("name='example'\nprotocol=1\nruntime='{runtime}'\nlint=true"),
        )
        .unwrap();

        assert!(
            Graft::load(&path, "example")
                .unwrap_err()
                .to_string()
                .contains("require an entry")
        );
    }
}

#[test]
fn native_receives_json_settings_and_returns_shared_layouts() {
    let directory = native::fixture(r#"{"version":1,"document":{"source":[0,8]}}"#);
    let options = Options::default();

    let expected = serde_json::json!({
        "version":1, "hook":"format", "source":"return 1",
        "configuration":{}, "settings":options,
    });

    let request = format!(
        "{{\"version\":1,\"hook\":\"format\",\"source\":\"return 1\",\"configuration\":{{}},\"settings\":{}}}",
        serde_json::to_string(&options).unwrap()
    );

    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&request).unwrap(),
        expected
    );

    fs::write(directory.path().join("request.json"), request).unwrap();
    let graft = Graft::load(&directory.path().join("graft.toml"), "example").unwrap();
    assert_eq!(graft.format(b"return 1", &options).unwrap(), b"return 1\n");
}

#[test]
fn native_lint_and_failure_paths_are_checked() {
    let directory =
        native::fixture(r#"[{"rule":"example","message":"reported","start":0,"end":6}]"#);

    let graft = Graft::load(&directory.path().join("graft.toml"), "example").unwrap();
    assert_eq!(graft.lint(b"return 1").unwrap()[0].end, 6);
    assert!(graft.lint(b"x").is_err());
    fs::write(directory.path().join("response.json"), "not JSON").unwrap();
    assert!(graft.lint(b"return 1").is_err());
    fs::write(directory.path().join("mode"), "failure").unwrap();

    assert!(
        graft
            .lint(b"return 1")
            .unwrap_err()
            .to_string()
            .contains("exited")
    );

    fs::write(directory.path().join("mode"), "oversize").unwrap();

    assert!(
        graft
            .lint(b"return 1")
            .unwrap_err()
            .to_string()
            .contains("payload limit")
    );
}

#[test]
fn native_drains_output_while_sending_input() {
    let source = "x".repeat(256 * 1024);

    let response =
        serde_json::json!([{"rule":"example", "message":source, "start":0, "end":source.len()}]);

    let directory = native::fixture(&response.to_string());
    fs::write(directory.path().join("mode"), "early").unwrap();
    let graft = Graft::load(&directory.path().join("graft.toml"), "example").unwrap();
    assert_eq!(graft.lint(source.as_bytes()).unwrap()[0].message, source);
}
