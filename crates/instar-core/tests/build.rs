//! Build planning, artifact publication, and incremental rebuild behavior.

use instar_core::{
    build::{Classification, Session},
    graft::Graft,
};
use std::{error::Error, fs, path::Path};

type TestResult = Result<(), Box<dyn Error>>;

#[path = "support/roblox.rs"]
mod support;

fn project(configuration: &str) -> tempfile::TempDir {
    let directory = tempfile::tempdir().expect("temporary project");
    fs::write(directory.path().join("instar.toml"), configuration).expect("configuration");
    fs::create_dir(directory.path().join("source")).expect("source directory");

    directory
}

fn execute(directory: &Path, source: &str, condition: &str) -> TestResult {
    fs::write(
        directory.join("module.luau"),
        format!(
            "local result = (function()\n{source}\nend)()\nreturn table.freeze({{lint = function() if not ({condition}) then local details = {{}} if type(result) == 'table' then for name, value in result do table.insert(details, tostring(name) .. '=' .. tostring(value)) end end table.sort(details) error('unexpected bundle result: ' .. table.concat(details, '; ')) end return {{}} end}})"
        ),
    )?;

    fs::write(
        directory.join("graft.toml"),
        "name = 'example'\nversion = 1\nruntime = 'luau'\nentry = 'module.luau'\nlint = true\n",
    )?;

    Graft::load(&directory.join("graft.toml"), "example")?.lint(b"")?;

    Ok(())
}

#[test]
fn directory_plans_rewrite_aliases_copy_assets_and_publish_incrementally() -> TestResult {
    let directory = project(
        "[aliases]\nvalue = 'source/value'\n[build]\ninputs = ['source']\noutput = 'output'\n",
    );

    fs::write(
        directory.path().join("source/main.luau"),
        "return require('@value')\n",
    )?;

    fs::write(directory.path().join("source/value.luau"), "return 1\n")?;
    fs::write(directory.path().join("source/data.json"), "{\"value\":1}")?;
    let mut session = Session::default();
    let plan = session.plan(directory.path(), None)?;
    assert!(!directory.path().join("output").exists());
    assert!(!plan.has_errors(), "{}", plan.json()?);
    assert_eq!(plan.modules.len(), 2);

    assert!(
        std::str::from_utf8(
            plan.contents(Path::new("source/main.luau"))
                .ok_or("output")?
        )?
        .contains("require(\"./value\")")
    );

    assert_eq!(plan.publish()?.written, 5);

    assert_eq!(
        session.plan(directory.path(), None)?.publish()?.unchanged,
        5
    );

    fs::write(directory.path().join("source/value.luau"), "return 2\n")?;
    let incremental = session.plan(directory.path(), None)?;
    let clean = Session::default().plan(directory.path(), None)?;
    assert!(incremental.equivalent(&clean));
    let outcome = incremental.publish()?;
    assert_eq!(outcome.written, 2);
    assert_eq!(outcome.unchanged, 3);

    Ok(())
}

#[test]
fn bundles_preserve_literals_caching_hygiene_and_lazy_initialization() -> TestResult {
    let directory = project(
        "[build]\nentry = 'source/main.luau'\nshape = 'bundle'\noutput = 'output/bundle.luau'\n",
    );

    fs::write(
        directory.path().join("source/main.luau"),
        "local __instar_modules = 3\nlocal first = require('./value')\nlocal second = require('./value')\nreturn {same = first == second, text = first.text, count = __instar_modules}",
    )?;

    fs::write(
        directory.path().join("source/value.luau"),
        "return {text = [=[first\n  second\nthird]=]}",
    )?;

    fs::write(
        directory.path().join("source/unreachable.luau"),
        "error('unreachable module ran')",
    )?;

    let plan = Session::default().plan(directory.path(), None)?;
    assert!(!plan.has_errors(), "{}", plan.json()?);
    assert_eq!(plan.modules.len(), 2);
    let source = std::str::from_utf8(plan.contents(Path::new("bundle.luau")).ok_or("bundle")?)?;
    assert!(!source.contains("unreachable module"));

    execute(
        directory.path(),
        source,
        "result.same and result.count == 3 and result.text == 'first\\n  second\\nthird'",
    )?;

    let map: serde_json::Value =
        serde_json::from_slice(plan.contents(Path::new("bundle.luau.map")).ok_or("map")?)?;

    assert_eq!(map["version"], 3);
    assert_eq!(map["sources"].as_array().ok_or("sources")?.len(), 2);

    Ok(())
}

#[test]
fn bundles_handle_false_nil_failed_loads_cycles_and_yields() -> TestResult {
    let directory = project(
        "[build]\nentry = 'source/main.luau'\nshape = 'bundle'\noutput = 'output/bundle.luau'\n",
    );

    fs::write(
        directory.path().join("source/main.luau"),
        "local first, message = pcall(function() return require('./failure') end)\nlocal second, repeated = pcall(function() return require('./failure') end)\nlocal cyclic, cycle = pcall(function() return require('./cycle') end)\nlocal thread = coroutine.create(function() return require('./yielding') end)\nlocal started, waiting = coroutine.resume(thread)\nlocal finished, value = coroutine.resume(thread, 7)\nreturn {first = first, second = second, message = tostring(message), repeated = tostring(repeated), cyclic = cyclic, cycle = tostring(cycle), waiting = waiting, finished = finished, value = value, false_value = require('./false'), nil_value = require('./nil')}",
    )?;

    fs::write(
        directory.path().join("source/failure.luau"),
        "error('intentional failure')",
    )?;

    fs::write(
        directory.path().join("source/cycle.luau"),
        "return require('./cycle')",
    )?;

    fs::write(
        directory.path().join("source/yielding.luau"),
        "return coroutine.yield('waiting')",
    )?;

    fs::write(directory.path().join("source/false.luau"), "return false")?;
    fs::write(directory.path().join("source/nil.luau"), "return nil")?;
    let plan = Session::default().plan(directory.path(), None)?;
    assert!(!plan.has_errors(), "{}", plan.json()?);

    execute(
        directory.path(),
        std::str::from_utf8(plan.contents(Path::new("bundle.luau")).ok_or("bundle")?)?,
        "result.first == false and result.second == false and string.find(result.repeated, 'intentional failure', 1, true) ~= nil and result.cyclic == false and string.find(result.cycle, 'cyclic initialization', 1, true) ~= nil and result.waiting == 'waiting' and result.finished and result.value == 7 and result.false_value == false and result.nil_value == nil",
    )?;

    Ok(())
}

#[test]
fn constants_lowering_minification_and_profiles_share_mappings() -> TestResult {
    let directory = project(
        "[build]\nentry = 'source/main.luau'\nshape = 'bundle'\noutput = 'output/development.luau'\n[build.constants]\nVALUE = 7\n[build.profiles.production]\noutput = 'output/bundle.luau'\nlower = true\nminify = true\n",
    );

    fs::write(
        directory.path().join("source/main.luau"),
        "export type Value = number\nconst function work<Value>(value: number): number\nlocal VALUE = 3\nreturn value + VALUE\nend\nlocal message = '😀'\nreturn {value = work(VALUE :: number), message = message}",
    )?;

    let plan = Session::default().plan(directory.path(), Some("production"))?;
    assert!(!plan.has_errors(), "{}", plan.json()?);
    let source = std::str::from_utf8(plan.contents(Path::new("bundle.luau")).ok_or("bundle")?)?;
    assert!(!source.contains("::"));
    assert!(!source.contains("export type"));
    assert!(!source.contains("const function"));

    execute(
        directory.path(),
        source,
        "result.value == 10 and result.message == '😀'",
    )?;

    let map: serde_json::Value =
        serde_json::from_slice(plan.contents(Path::new("bundle.luau.map")).ok_or("map")?)?;

    assert!(
        map["mappings"]
            .as_str()
            .is_some_and(|value| !value.is_empty())
    );

    assert!(
        Session::default()
            .plan(directory.path(), Some("missing"))
            .is_err()
    );

    Ok(())
}

#[test]
fn failures_preserve_outputs_and_pruning_only_removes_owned_files() -> TestResult {
    let directory = project("[build]\ninputs = ['source']\noutput = 'output'\n");
    let source = directory.path().join("source/main.luau");
    fs::write(&source, "return 1")?;
    fs::write(directory.path().join("source/stale.txt"), "owned")?;
    let mut session = Session::default();
    session.plan(directory.path(), None)?.publish()?;
    let output = directory.path().join("output/source/main.luau");
    fs::write(directory.path().join("output/unowned.txt"), "retained")?;
    fs::write(&source, "local =")?;
    assert!(session.plan(directory.path(), None).is_err());
    assert_eq!(fs::read_to_string(&output)?, "return 1");
    fs::write(&source, "return 2")?;
    fs::remove_file(directory.path().join("source/stale.txt"))?;
    assert_eq!(session.plan(directory.path(), None)?.publish()?.removed, 1);

    assert_eq!(
        fs::read_to_string(directory.path().join("output/unowned.txt"))?,
        "retained"
    );

    fs::write(&output, "user edit")?;
    assert!(session.plan(directory.path(), None)?.publish().is_err());
    assert_eq!(fs::read_to_string(&output)?, "user edit");

    Ok(())
}

#[test]
fn graph_classifies_unresolved_dynamic_and_external_requires() -> TestResult {
    let directory =
        project("[build]\ninputs = ['source']\noutput = 'output'\nexternal = ['@provided']\n");

    fs::write(
        directory.path().join("source/main.luau"),
        "local first = require('@provided')\nlocal second = require(unknown)\nreturn first, second, require('./missing')",
    )?;

    let plan = Session::default().plan(directory.path(), None)?;
    assert!(plan.has_errors());

    assert!(
        plan.modules[0]
            .dependencies
            .iter()
            .any(|dependency| dependency.classification == Classification::External)
    );

    assert!(
        plan.modules[0]
            .dependencies
            .iter()
            .any(|dependency| dependency.classification == Classification::Dynamic)
    );

    assert!(
        plan.modules[0]
            .dependencies
            .iter()
            .any(|dependency| dependency.classification == Classification::Unresolved)
    );

    assert!(plan.publish().is_err());
    assert!(!directory.path().join("output").exists());

    Ok(())
}

#[test]
fn graft_compilation_exposes_dependencies_and_validated_source_mappings() -> TestResult {
    let directory = project(
        "[grafts]\nexample = 'graft.toml'\n[build]\ninputs = ['source']\nentry = 'source/main.luau'\nshape = 'bundle'\noutput = 'output/bundle.luau'\n[build.languages]\ncustom = 'example'\n",
    );

    fs::write(
        directory.path().join("graft.toml"),
        "name = 'example'\nversion = 1\nruntime = 'luau'\nentry = 'compiler.luau'\ncompile = true\n",
    )?;

    fs::write(
        directory.path().join("compiler.luau"),
        "return table.freeze({compile = function(request) return {version = 1, source = 'return 9', dependencies = {'data.txt'}, mappings = {{start = 0, ['end'] = 8, original_start = 0, original_end = #request.source}}} end})",
    )?;

    fs::write(directory.path().join("source/data.txt"), "dependency")?;
    fs::write(directory.path().join("source/value.custom"), "😀abcd")?;

    fs::write(
        directory.path().join("source/main.luau"),
        "return require('./value')",
    )?;

    let plan = Session::default().plan(directory.path(), None)?;
    assert!(!plan.has_errors(), "{}", plan.json()?);

    assert!(
        plan.inputs()
            .iter()
            .any(|path| path.ends_with("source/data.txt"))
    );

    execute(
        directory.path(),
        std::str::from_utf8(plan.contents(Path::new("bundle.luau")).ok_or("bundle")?)?,
        "result == 9",
    )?;

    Ok(())
}

#[test]
fn roblox_builds_generate_project_paths_and_reject_unsafe_realms() -> TestResult {
    let directory = project(
        "[build]\ninputs = ['source']\noutput = 'output'\n[roblox]\nproject = 'default.project.json'\n",
    );

    support::configure(directory.path())?;
    fs::create_dir(directory.path().join("source/shared"))?;
    fs::create_dir(directory.path().join("source/server"))?;
    fs::create_dir(directory.path().join("source/client"))?;

    fs::write(
        directory.path().join("default.project.json"),
        r#"{"name":"Game","tree":{"$className":"DataModel","ReplicatedStorage":{"$path":"source/shared"},"ServerScriptService":{"$path":"source/server"},"StarterPlayer":{"StarterPlayerScripts":{"$path":"source/client"}}}}"#,
    )?;

    fs::write(
        directory.path().join("source/shared/value.luau"),
        "return 1",
    )?;

    fs::write(
        directory.path().join("source/server/main.server.luau"),
        "return require('../shared/value')",
    )?;

    fs::write(
        directory.path().join("source/client/value.luau"),
        "return 2",
    )?;

    fs::write(
        directory.path().join("source/client/main.client.luau"),
        "return require('./value')",
    )?;

    let mut session = Session::default();
    let plan = session.plan(directory.path(), None)?;
    assert!(!plan.has_errors(), "{}", plan.json()?);

    assert!(
        std::str::from_utf8(
            plan.contents(Path::new("source/server/main.server.luau"))
                .ok_or("server")?
        )?
        .contains("@game/ReplicatedStorage/value")
    );

    assert!(
        std::str::from_utf8(
            plan.contents(Path::new("source/client/main.client.luau"))
                .ok_or("client")?
        )?
        .contains("@self/../value")
    );

    assert!(plan.contents(Path::new("default.project.json")).is_some());

    fs::write(
        directory.path().join("source/shared/value.luau"),
        "return require('../client/value')",
    )?;

    assert!(session.plan(directory.path(), None)?.has_errors());

    Ok(())
}

#[test]
fn output_overlap_and_destination_collisions_are_rejected() -> TestResult {
    let directory = project("[build]\ninputs = ['source']\noutput = 'source/output'\n");
    fs::write(directory.path().join("source/main.luau"), "return 1")?;
    assert!(Session::default().plan(directory.path(), None).is_err());

    fs::write(
        directory.path().join("instar.toml"),
        "[build]\ninputs = ['source']\noutput = 'output'\n",
    )?;

    fs::write(directory.path().join("source/main.luau.map"), "asset")?;
    assert!(Session::default().plan(directory.path(), None).is_err());

    Ok(())
}

#[test]
fn compiling_grafts_participate_in_roblox_instance_mapping() -> TestResult {
    let directory = project(
        "[grafts]\nexample = 'graft.toml'\n[build]\ninputs = ['source']\noutput = 'output'\n[build.languages]\ncustom = 'example'\n[roblox]\nproject = 'default.project.json'\n",
    );

    support::configure(directory.path())?;

    fs::write(
        directory.path().join("graft.toml"),
        "name = 'example'\nversion = 1\nruntime = 'luau'\nentry = 'compiler.luau'\ncompile = true\n",
    )?;

    fs::write(
        directory.path().join("compiler.luau"),
        "return table.freeze({compile = function(request) return {version = 1, source = request.source, dependencies = {}, mappings = {{start = 0, ['end'] = #request.source, original_start = 0, original_end = #request.source}}} end})",
    )?;

    fs::write(directory.path().join("source/value.custom"), "return 9")?;

    fs::write(
        directory.path().join("source/main.luau"),
        "return require('./value')",
    )?;

    fs::write(
        directory.path().join("default.project.json"),
        r#"{"name":"Game","tree":{"$className":"DataModel","ReplicatedStorage":{"Main":{"$path":"source/main.luau"},"Value":{"$path":"source/value.custom"}}}}"#,
    )?;

    let plan = Session::default().plan(directory.path(), None)?;
    assert!(!plan.has_errors(), "{}", plan.json()?);

    assert!(
        std::str::from_utf8(plan.contents(Path::new("source/main.luau")).ok_or("main")?)?
            .contains("@game/ReplicatedStorage/Value")
    );

    let translated: serde_json::Value = serde_json::from_slice(
        plan.contents(Path::new("default.project.json"))
            .ok_or("project")?,
    )?;

    assert_eq!(
        translated["tree"]["ReplicatedStorage"]["Value"]["$path"],
        "./source/value.luau"
    );

    assert!(
        fs::read_to_string(directory.path().join("default.project.json"))?.contains("value.custom")
    );

    plan.publish()?;

    Ok(())
}

#[test]
fn lowering_preserves_contextual_names_and_source_locations() -> TestResult {
    let directory =
        project("[build]\ninputs = ['source']\noutput = 'output'\nlower = true\nminify = true\n");

    fs::write(
        directory.path().join("source/main.luau"),
        "export type Value = number\nconst value = {const = 7}\nreturn value.const",
    )?;

    let plan = Session::default().plan(directory.path(), None)?;

    execute(
        directory.path(),
        std::str::from_utf8(
            plan.contents(Path::new("source/main.luau"))
                .ok_or("source")?,
        )?,
        "result == 7",
    )?;

    let map: serde_json::Value = serde_json::from_slice(
        plan.contents(Path::new("source/main.luau.map"))
            .ok_or("mapping")?,
    )?;

    assert_eq!(map["sourceRoot"], "../../");
    assert_eq!(map["sources"][0], "source/main.luau");

    Ok(())
}

#[test]
fn publication_rejects_new_inputs_and_unowned_output() -> TestResult {
    let directory = project("[build]\ninputs = ['source']\noutput = 'output'\n");
    fs::write(directory.path().join("source/main.luau"), "return 1")?;
    let mut session = Session::default();
    let plan = session.plan(directory.path(), None)?;
    fs::write(directory.path().join("source/added.luau"), "return 2")?;
    assert!(plan.publish().is_err());
    assert!(!directory.path().join("output").exists());
    let plan = session.plan(directory.path(), None)?;
    fs::create_dir_all(directory.path().join("output/source"))?;
    fs::write(directory.path().join("output/source/main.luau"), "unowned")?;
    assert!(plan.publish().is_err());

    assert_eq!(
        fs::read_to_string(directory.path().join("output/source/main.luau"))?,
        "unowned"
    );

    Ok(())
}

#[test]
fn bundles_prune_invalid_unreachable_modules() -> TestResult {
    let directory = project(
        "[build]\ninputs = ['source']\nentry = 'source/main.luau'\nshape = 'bundle'\noutput = 'output/bundle.luau'\n",
    );

    fs::write(directory.path().join("source/main.luau"), "return 1")?;
    fs::write(directory.path().join("source/unreachable.luau"), "local =")?;
    let plan = Session::default().plan(directory.path(), None)?;
    assert_eq!(plan.modules.len(), 1);

    execute(
        directory.path(),
        std::str::from_utf8(plan.contents(Path::new("bundle.luau")).ok_or("bundle")?)?,
        "result == 1",
    )?;

    Ok(())
}

#[cfg(windows)]
#[test]
fn metadata_publication_failure_rolls_back_artifacts_and_snapshots() -> TestResult {
    let directory = project("[build]\ninputs = ['source']\noutput = 'output'\n");
    let source = directory.path().join("source/main.luau");
    fs::write(&source, "return 1")?;
    let mut session = Session::default();
    session.plan(directory.path(), None)?.publish()?;
    let manifest = directory.path().join("output/.instar/manifest.json");
    let permissions = fs::metadata(&manifest)?.permissions();
    let mut readonly = permissions.clone();
    readonly.set_readonly(true);
    fs::set_permissions(&manifest, readonly)?;
    fs::write(&source, "return 2")?;
    let result = session.plan(directory.path(), None)?.publish();
    fs::set_permissions(&manifest, permissions)?;
    assert!(result.is_err());

    assert_eq!(
        fs::read_to_string(directory.path().join("output/source/main.luau"))?,
        "return 1"
    );

    session.plan(directory.path(), None)?.publish()?;

    assert_eq!(
        fs::read_to_string(directory.path().join("output/source/main.luau"))?,
        "return 2"
    );

    Ok(())
}
