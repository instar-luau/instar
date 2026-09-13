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
    let project = tempfile::tempdir_in(directory)?;
    let directory = project.path();

    fs::write(
        directory.join("module.luau"),
        format!(
            "local result = (function()\n{source}\nend)()\nreturn table.freeze({{lint = function() if not ({condition}) then local details = {{}} if type(result) == 'table' then for name, value in result do table.insert(details, tostring(name) .. '=' .. tostring(value)) end end table.sort(details) error('unexpected bundle result: ' .. table.concat(details, '; ')) end return {{}} end}})"
        ),
    )?;

    fs::write(
        directory.join("graft.toml"),
        "name = 'example'\nprotocol = 1\nruntime = 'luau'\nentry = 'module.luau'\nlint = true\n",
    )?;

    Graft::load(&directory.join("graft.toml"), "example")?.lint(b"")?;

    Ok(())
}

#[test]
fn directory_plans_rewrite_aliases_copy_assets_and_publish_incrementally() -> TestResult {
    let directory = project(
        "[analyze.aliases]\nvalue = 'source/value'\n[build]\ninputs = ['source']\noutput = 'output'\n",
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
fn bundles_preserve_global_loadstring() -> TestResult {
    let directory = project(
        "[build]\ninputs = ['source']\nentry = 'source/main.luau'\nshape = 'bundle'\noutput = 'output/bundle.luau'\n",
    );

    fs::write(
        directory.path().join("source/main.luau"),
        "return loadstring('return 1')()",
    )?;

    let plan = Session::default().plan(directory.path(), None)?;
    assert!(!plan.has_errors(), "{}", plan.json()?);

    assert!(
        std::str::from_utf8(plan.contents(Path::new("bundle.luau")).ok_or("bundle")?)?
            .contains("loadstring")
    );

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
fn configured_build_rules_transform_sources_before_output() -> TestResult {
    let directory = project(
        "[build]\ninputs = ['source']\noutput = 'output'\n[build.rules]\ncompute_expression = true\nconvert_function_to_assignment = true\nconvert_index_to_field = true\nconvert_luau_number = true\nconvert_square_root_call = true\nfilter_after_early_return = true\nremove_attribute = true\nremove_comments = { except = [] }\nremove_compound_assignment = true\nremove_debug_profiling = true\nremove_empty_do = true\nremove_floor_division = true\nremove_interpolated_string = true\nremove_method_call = true\nremove_method_definition = true\nremove_nil_declaration = true\nremove_types = true\nremove_unused_if_branch = true\nremove_unused_variable = true\nremove_unused_while = true\n",
    );

    fs::write(
        directory.path().join("source/main.luau"),
        "-- removed\nlocal object = {}\n@native\nfunction attributed() end\nfunction object:method(value: number): string\n\tdebug.profilebegin('method')\n\tlocal unused = 1\n\tlocal nilled = nil\n\tdo end\n\tif false then error('dead branch') else value += 0b1010 end\n\twhile false do error('dead loop') end\n\tdebug.profileend()\n\treturn `value {math.sqrt(value // 1)}`\nend\nlocal data = {[\"field\"] = 1 + 2}\nlocal function filtered()\n\tdo return end\n\terror('dead return')\nend\nfiltered()\nreturn object:method(data[\"field\"])\n",
    )?;

    let plan = Session::default().plan(directory.path(), None)?;
    assert!(!plan.has_errors(), "{}", plan.json()?);
    assert!(plan.stages.iter().any(|stage| stage == "rules"));
    assert!(plan.stages.iter().any(|stage| stage == "lower"));

    let output = std::str::from_utf8(
        plan.contents(Path::new("source/main.luau"))
            .ok_or("output")?,
    )?;

    for removed in [
        "-- removed",
        "@native",
        ":method",
        "debug.profile",
        "unused",
        "nilled",
        "do end",
        "dead branch",
        "dead loop",
        "dead return",
        "0b1_0",
        "//",
        "`value",
        "[\"field\"]",
        ": number",
        ": string",
    ] {
        assert!(!output.contains(removed), "{removed} remains in:\n{output}");
    }

    assert!(
        output.contains("object.method = function(self, value)"),
        "{output}"
    );

    assert!(output.contains("value = value + 0xA"), "{output}");
    assert!(output.contains("math.floor(value / 1)"), "{output}");
    assert!(output.contains("^ 0.5"), "{output}");
    assert!(output.contains("string.format"), "{output}");
    assert!(output.contains("field = 3"), "{output}");

    assert!(
        output.contains("object.method(object, data.field)"),
        "{output}"
    );

    Ok(())
}

#[test]
fn method_and_branch_rules_preserve_live_source() -> TestResult {
    let directory = project(
        "[build]\ninputs = ['source']\noutput = 'output'\n[build.rules]\nremove_method_definition = true\nremove_method_call = true\nremove_unused_if_branch = true\n",
    );

    fs::write(
        directory.path().join("source/main.luau"),
        "local object = {}\nfunction object:method(value) return self, value end\nobject:method{'value'}\nnested.object:method(1)\nif condition then\n\tone()\nelseif false then\n\ttwo()\nelse\n\tthree()\nend\n",
    )?;

    let plan = Session::default().plan(directory.path(), None)?;
    assert!(!plan.has_errors(), "{}", plan.json()?);

    let output = std::str::from_utf8(
        plan.contents(Path::new("source/main.luau"))
            .ok_or("output")?,
    )?;

    assert!(
        output.contains("function object.method(self, value)"),
        "{output}"
    );

    assert!(
        output.contains("object.method(object, {'value'})"),
        "{output}"
    );

    assert!(output.contains("nested.object:method(1)"), "{output}");
    assert!(output.contains("one()"), "{output}");
    assert!(!output.contains("two()"), "{output}");
    assert!(output.contains("three()"), "{output}");

    Ok(())
}

#[test]
fn function_assignment_rules_preserve_attributes_generics_and_recursion() -> TestResult {
    let directory = project(
        "[build]\ninputs = ['source']\noutput = 'output'\n[build.rules]\nconvert_function_to_assignment = true\nconvert_local_function_to_assign = true\n",
    );

    fs::write(
        directory.path().join("source/main.luau"),
        "@native\nfunction attributed() end\nfunction generic<T>(value: T): T return value end\nlocal function recursive() return recursive() end\nreturn attributed, generic, recursive\n",
    )?;

    let plan = Session::default().plan(directory.path(), None)?;
    assert!(!plan.has_errors(), "{}", plan.json()?);

    let output = std::str::from_utf8(
        plan.contents(Path::new("source/main.luau"))
            .ok_or("output")?,
    )?;

    assert!(
        output.contains("@native\nfunction attributed()"),
        "{output}"
    );

    assert!(
        output.contains("generic = function<T>(value: T)"),
        "{output}"
    );

    assert!(output.contains("local function recursive()"), "{output}");

    Ok(())
}

#[test]
fn variable_renaming_uses_local_identity_and_preserves_behavior() -> TestResult {
    let directory = project(
        "[build]\nentry = 'source/main.luau'\nshape = 'bundle'\noutput = 'output/bundle.luau'\nminify = true\n[build.rules]\nrename_variables = true\n",
    );

    fs::write(
        directory.path().join("source/main.luau"),
        "local long_name = 3\nlocal function add(value) return value + long_name end\nreturn add(4)",
    )?;

    let plan = Session::default().plan(directory.path(), None)?;
    assert!(!plan.has_errors(), "{}", plan.json()?);
    let output = std::str::from_utf8(plan.contents(Path::new("bundle.luau")).ok_or("bundle")?)?;
    assert!(!output.contains("long_name"), "{output}");
    execute(directory.path(), output, "result == 7")?;

    Ok(())
}

#[test]
fn remaining_build_rules_transform_their_supported_forms() -> TestResult {
    let directory = project(
        "[build]\ninputs = ['source']\noutput = 'output'\n[build.rules]\nconst_requires = true\nappend_text_comment = { text = 'tail', location = 'end' }\nadd_luau_directive = 'native'\nremove_if_expression = true\nmake_assignment_local = true\nremove_function_call_parens = true\nremove_continue = true\nremove_assertions = true\ngroup_local_assignment = true\nconvert_local_function_to_assign = true\nremove_calls = ['print']\ndedupe_requires = true\nfreeze_module = true\n",
    );

    fs::write(directory.path().join("source/value.luau"), "return 1")?;

    fs::write(
        directory.path().join("source/main.luau"),
        "local A = require('./value')\nlocal B = require('./value')\nconst changed = 1\nlocal first = 1\nlocal second = 2\nlocal function identity(value) return value end\nlocal selected = if true then 1 else 2\nassert(true)\nprint('drop')\nwhile running do continue end\nidentity({'value'})\nreturn {A, B, changed, first, second, selected}\n",
    )?;

    let plan = Session::default().plan(directory.path(), None)?;
    assert!(!plan.has_errors(), "{}", plan.json()?);

    let output = std::str::from_utf8(
        plan.contents(Path::new("source/main.luau"))
            .ok_or("output")?,
    )?;

    assert!(output.starts_with("--!native\n"), "{output}");
    assert!(output.contains("const A = require"), "{output}");
    assert!(output.contains("const B = A"), "{output}");
    assert!(output.contains("local changed = 1"), "{output}");
    assert!(output.contains("local first, second = 1, 2"), "{output}");
    assert!(output.contains("local identity = function"), "{output}");
    assert!(output.contains("true and 1 or 2"), "{output}");
    assert!(!output.contains("assert("), "{output}");
    assert!(!output.contains("print("), "{output}");
    assert!(output.contains("repeat"), "{output}");
    assert!(!output.contains("continue"), "{output}");
    assert!(output.contains("identity{'value'}"), "{output}");
    assert!(output.contains("return table.freeze("), "{output}");
    assert!(output.trim_end().ends_with("-- tail"), "{output}");

    Ok(())
}

#[test]
fn grouped_locals_keep_sequential_dependencies() -> TestResult {
    let directory = project(
        "[build]\ninputs = ['source']\noutput = 'output'\n[build.rules]\ngroup_local_assignment = true\n",
    );

    let source = "local first = 1\nlocal second = first\nreturn second\n";
    fs::write(directory.path().join("source/main.luau"), source)?;

    let plan = Session::default().plan(directory.path(), None)?;
    assert!(!plan.has_errors(), "{}", plan.json()?);

    assert_eq!(
        std::str::from_utf8(
            plan.contents(Path::new("source/main.luau"))
                .ok_or("output")?
        )?,
        source
    );

    Ok(())
}

#[test]
fn build_rule_options_preserve_configured_source() -> TestResult {
    let directory = project(
        "[build]\ninputs = ['source']\noutput = 'output'\n[build.rules]\nremove_comments = {}\nappend_text_comment = { file = 'notice.txt' }\nremove_attribute = { match = ['^native$'] }\nremove_interpolated_string = { strategy = 'tostring' }\nremove_assertions = { preserve_arguments_side_effects = false }\nremove_debug_profiling = { preserve_arguments_side_effects = false }\nremove_calls = { functions = ['warn'], preserve_arguments_side_effects = false }\n",
    );

    fs::write(directory.path().join("notice.txt"), "notice")?;

    fs::write(
        directory.path().join("source/main.luau"),
        "--!strict\n-- removed\n@native\nfunction removed() end\n@checked\nfunction retained() end\nassert(side())\ndebug.profilebegin(side())\nwarn(side())\nlocal nested = `nested {choose([=[}]=])}`\nreturn `value {side()}`, nested\n",
    )?;

    let plan = Session::default().plan(directory.path(), None)?;
    assert!(!plan.has_errors(), "{}", plan.json()?);

    let output = std::str::from_utf8(
        plan.contents(Path::new("source/main.luau"))
            .ok_or("output")?,
    )?;

    assert!(output.starts_with("-- notice\n--!strict\n"), "{output}");
    assert!(!output.contains("-- removed"), "{output}");
    assert!(!output.contains("@native"), "{output}");
    assert!(output.contains("@checked"), "{output}");
    assert!(!output.contains("assert("), "{output}");
    assert!(!output.contains("debug.profilebegin("), "{output}");
    assert!(!output.contains("warn("), "{output}");

    assert!(
        output.contains("string.format(\"nested %*\", choose([=[}]=]))"),
        "{output}"
    );

    assert!(
        output.contains("string.format(\"value %*\", side())"),
        "{output}"
    );

    assert!(
        plan.inputs()
            .iter()
            .any(|path| path.ends_with("notice.txt")),
        "{:?}",
        plan.inputs()
    );

    Ok(())
}

#[test]
fn build_rule_names_and_strategies_are_validated() -> TestResult {
    for rule in [
        "inject_module_path = 'if'",
        "remove_calls = ['debug.if']",
        "remove_interpolated_string = { strategy = 'invalid' }",
    ] {
        let directory = project(&format!(
            "[build]\ninputs = ['source']\noutput = 'output'\n[build.rules]\n{rule}\n"
        ));

        fs::write(directory.path().join("source/main.luau"), "return 1")?;
        assert!(Session::default().plan(directory.path(), None).is_err());
    }

    Ok(())
}

#[test]
fn roblox_native_rules_use_services_and_mounted_module_paths() -> TestResult {
    let directory = project(
        "[build]\ninputs = ['source']\noutput = 'output'\n[build.rules]\nuse_get_service = true\ninject_module_path = 'MODULE_PATH'\n[analyze.roblox]\nproject = 'default.project.json'\n",
    );

    support::configure(directory.path())?;

    fs::write(
        directory.path().join("default.project.json"),
        r#"{"name":"Game","tree":{"$className":"DataModel","ReplicatedStorage":{"Main":{"$path":"source/main.luau"}}}}"#,
    )?;

    fs::write(
        directory.path().join("source/main.luau"),
        "--!strict\n-- header\nreturn game.Players, game.AnalyticsService, game.Part, MODULE_PATH\n",
    )?;

    let plan = Session::default().plan(directory.path(), None)?;
    assert!(!plan.has_errors(), "{}", plan.json()?);

    let output = std::str::from_utf8(
        plan.contents(Path::new("source/main.luau"))
            .ok_or("output")?,
    )?;

    assert!(output.contains("game:GetService(\"Players\")"), "{output}");

    assert!(
        output.contains("game:GetService(\"AnalyticsService\")"),
        "{output}"
    );

    assert!(output.contains("game.Part"), "{output}");

    assert!(
        output.starts_with("--!strict\n-- header\nlocal MODULE_PATH"),
        "{output}"
    );

    assert!(
        output.contains("local MODULE_PATH = \"@game/ReplicatedStorage/Main\""),
        "{output}"
    );

    Ok(())
}

#[test]
fn graft_compilation_exposes_dependencies_and_validated_source_mappings() -> TestResult {
    let directory = project(
        "[grafts]\nexample = { path = '.' }\n[build]\ninputs = ['source']\nentry = 'source/main.luau'\nshape = 'bundle'\noutput = 'output/bundle.luau'\n",
    );

    fs::write(
        directory.path().join("graft.toml"),
        "name = 'example'\nprotocol = 1\nruntime = 'luau'\nentry = 'compiler.luau'\ncompile = true\nextensions = ['custom']\n",
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
        "[build]\ninputs = ['source']\noutput = 'output'\n[analyze.roblox]\nproject = 'default.project.json'\n",
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
        "[grafts]\nexample = { path = '.' }\n[build]\ninputs = ['source']\noutput = 'output'\n[analyze.roblox]\nproject = 'default.project.json'\n",
    );

    support::configure(directory.path())?;

    fs::write(
        directory.path().join("graft.toml"),
        "name = 'example'\nprotocol = 1\nruntime = 'luau'\nentry = 'compiler.luau'\ncompile = true\nextensions = ['custom']\n",
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
