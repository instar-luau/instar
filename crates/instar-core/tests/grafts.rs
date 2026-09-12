use instar_core::{configuration::format::Options, graft::Graft, project::Configuration};
use std::fs;

#[path = "support/graft.rs"]
mod support;
use support::{fixture, module};

#[path = "support/native.rs"]
mod native;

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

    fs::write(
        directory.path().join("instar.toml"),
        "[grafts]\nexample = 'graft.toml'\n[lint.rules]\n'example/example' = 'deny'\n",
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

    for runtime in ["native", "luau", "wasm"] {
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

#[test]
fn luau_receives_settings_and_uses_host_layouts() {
    let directory = support::luau(
        r#"
local function format(request: {read settings: {read indentation: {read width: number}}, read configuration: {read setting: string}})
    if request.settings.indentation.width ~= 2 or request.configuration.setting ~= "value" then
        error("missing settings or configuration")
    end
    return {
        version = 1,
        document = {sequence = {
            {text = "local value ="},
            {indent = {sequence = {"hard", {host = {start = 14, ["end"] = 15, parse = "expression"}}}}},
        }},
    }
end
return table.freeze({format = format})
"#,
        "format",
    );

    let manifest = directory.path().join("graft.toml");

    fs::write(
        &manifest,
        fs::read_to_string(&manifest).unwrap() + "[configuration]\nsetting='value'\n",
    )
    .unwrap();

    fs::write(
        directory.path().join("instar.toml"),
        "[format]\nindentation.style='spaces'\nindentation.width=2\n[grafts]\nexample='graft.toml'",
    )
    .unwrap();

    let configuration =
        Configuration::discover(&directory.path().join("source.luau"), None).unwrap();

    let output = configuration.format(b"local value=1").unwrap();
    assert_eq!(output, b"local value =\n  1\n");
    assert_eq!(configuration.format(&output).unwrap(), output);
}

#[test]
fn luau_preserves_request_bytes_and_validates_lint_ranges() {
    let directory = support::luau(
        r#"
local function lint(request: {read source: string, read configuration: {read message: string}})
    return {{rule = "example", message = request.source .. request.configuration.message, start = 0, ["end"] = #request.source}}
end
return table.freeze({lint = lint})
"#,
        "lint",
    );

    let manifest = directory.path().join("graft.toml");

    fs::write(
        &manifest,
        fs::read_to_string(&manifest).unwrap() + "[configuration]\nmessage='雪\"\\123'\n",
    )
    .unwrap();

    let graft = Graft::load(&manifest, "example").unwrap();
    let source = "雪\"\\123\0\nreturn [=[value]=]";
    let findings = graft.lint(source.as_bytes()).unwrap();
    assert_eq!(findings[0].message, format!("{source}雪\"\\123"));
    assert_eq!(findings[0].end, source.len());

    assert_eq!(
        graft
            .format(source.as_bytes(), &Options::default())
            .unwrap(),
        source.as_bytes()
    );

    fs::write(directory.path().join("module.luau"), "return {lint=function() return {{rule='example', message='reported', start=1, ['end']=2}} end}").unwrap();
    let graft = Graft::load(&manifest, "example").unwrap();
    assert!(graft.lint("雪".as_bytes()).is_err());
}

#[test]
fn luau_modules_and_responses_are_checked() {
    for source in ["not luau", "return 1", "return {}", "return {format=1}"] {
        let directory = support::luau(source, "format");

        assert!(
            Graft::load(&directory.path().join("graft.toml"), "example").is_err(),
            "{source}"
        );
    }

    for body in [
        "error('failure')",
        "error({})",
        "local value = {}; value.self = value; return value",
        "return function() end",
        "return setmetatable({}, {})",
        "return {[1]='first', [3]='third'}",
        "return {[1]='first', name='mixed'}",
        "return {version=0/0, document='empty'}",
        "local value = {}; for index=1,128 do value={value} end; return value",
        "return {version=1, document={text='return 2'}}",
        "return {version=1, document={source={0,999}}}",
        "return {version=1, document={text=string.char(255)}}",
    ] {
        let directory = support::luau(
            &format!("return {{format=function() {body} end}}"),
            "format",
        );

        let graft = Graft::load(&directory.path().join("graft.toml"), "example").unwrap();

        assert!(
            graft.format(b"return 1", &Options::default()).is_err(),
            "{body}"
        );
    }
}

#[test]
fn luau_instances_are_isolated_and_have_no_host_io() {
    let directory = support::luau(
        r#"
if io ~= nil or require ~= nil or print ~= nil or os.execute ~= nil then
    error("host capabilities exposed")
end
local count = 0
local function lint()
    count += 1
    if count ~= 1 then error("instance reused") end
    return {}
end
return table.freeze({lint=lint})
"#,
        "lint",
    );

    let graft = Graft::load(&directory.path().join("graft.toml"), "example").unwrap();
    assert!(graft.lint(b"return 1").unwrap().is_empty());
    assert!(graft.lint(b"return 1").unwrap().is_empty());
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
