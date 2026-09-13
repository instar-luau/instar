//! Lint findings, configured levels, and safe source fixes.

use instar_core::{analysis, lint, source::SourceStore};
use std::{error::Error, fmt::Write, fs};

#[path = "support/roblox.rs"]
mod support;

type TestResult = Result<(), Box<dyn Error>>;

const CASES: &[(&str, &str)] = &[
    (
        "incomplete_swap",
        "local first, second = 1, 2\nfirst = second\nsecond = first\nreturn first, second",
    ),
    ("not_a_number_comparison", "return 1 == 0/0"),
    ("table_identity_comparison", "return {} == {}"),
    ("division_by_zero", "return 1/0"),
    ("duplicate_table_key", "return {value = 1, value = 2}"),
    (
        "repeated_condition",
        "if unknown then print(1) elseif unknown then print(2) end",
    ),
    (
        "identical_branches",
        "if unknown then print(1) else print(1) end",
    ),
    (
        "invalid_reverse_loop",
        "for index = 10, 1 do print(index) end",
    ),
    (
        "misplaced_type_comparison",
        "return type(unknown == 'string')",
    ),
    (
        "unbalanced_assignment",
        "local first, second = 1\nreturn first, second",
    ),
    ("zero_loop_step", "for index = 1, 10, 0 do print(index) end"),
    ("invalid_string_escape", "return '\\q'"),
    (
        "argument_count",
        "local function work(value: number) return value end\nreturn work(1, 2)",
    ),
    ("discarded_return", "math.abs(1)"),
    ("invalid_directive", "--!unknown\nreturn 1"),
    ("builtin_assignment", "print = 1"),
    ("comparison_precedence", "return not unknown == 1"),
    (
        "duplicate_function",
        "local function work() end\nlocal function work() end\nreturn work",
    ),
    (
        "duplicate_binding",
        "local value, value = 1, 2\nreturn value",
    ),
    ("invalid_format_string", "return string.format('%J', 1)"),
    (
        "inconsistent_return",
        "local function work(condition) if condition then return 1 end end\nreturn work",
    ),
    ("misleading_conditional", "return unknown and false or 1"),
    ("number_literal_overflow", "return 0xfffffffffffffffff"),
    ("placeholder_read", "local _ = 1\nreturn _"),
    ("invalid_table_operation", "table.insert({}, 0, 1)"),
    ("untyped_local", "local value\nreturn value"),
    (
        "untyped_parameter",
        "local function work(value) return value end\nreturn work",
    ),
    ("uninitialized_local", "local value\nreturn value"),
    ("invalid_type_name", "return type(1) == 'integer'"),
    ("undefined_variable", "return unknown"),
    ("global_assignment", "unknown = 1"),
    ("unused_function", "local function work() end"),
    ("unused_variable", "local value = 1"),
    (
        "unused_import",
        "local dependency = require('./dependency')",
    ),
    (
        "shadowed_binding",
        "local value = 1\ndo local value = 2 print(value) end\nreturn value",
    ),
    ("global_environment", "return _G"),
    ("deprecated_function", "return table.getn({})"),
    ("restricted_import", "return require('./dependency')"),
    (
        "function_complexity",
        "local function work(condition) if condition then return 1 else return 2 end end\nreturn work",
    ),
    (
        "manual_table_clone",
        "local source = {}\nlocal target = {}\nfor key, value in pairs(source) do target[key] = value end\nreturn target",
    ),
    ("constant_binding", "local value = 1\nreturn value"),
    ("restricted_global", "print(1)"),
    (
        "constant_import",
        "local dependency = require('./dependency')\nreturn dependency",
    ),
    (
        "unreachable_code",
        "if unknown then return 1 else return 2 end\nprint(3)",
    ),
    (
        "self_assignment",
        "local value = 1\nvalue = value\nreturn value",
    ),
    (
        "loop_string_concatenation",
        "local value = ''\nfor index = 1, 3 do value ..= tostring(index) end\nreturn value",
    ),
    (
        "loop_invariant_call",
        "for index = 1, 3 do print(index, require('./dependency')) end",
    ),
    ("length_condition", "if #{} then print(1) end"),
    ("shadowed_builtin", "local print = 1\nreturn print"),
    ("ignored_protected_call", "pcall(print)"),
    ("empty_branch", "if unknown then end"),
    ("empty_loop", "while unknown do end"),
    ("mixed_table", "return {1, value = 2}"),
    ("multiple_statements", "print(1) print(2)"),
    ("parenthesized_condition", "if (unknown) then print(1) end"),
    ("constant_condition", "if true then print(1) end"),
    (
        "redundant_else",
        "if unknown then return 1 else return 2 end",
    ),
    (
        "nested_condition",
        "if first then if second then print(1) end end",
    ),
    (
        "negated_condition",
        "if not unknown then print(1) else print(2) end",
    ),
    ("logical_conditional", "return unknown and 1 or 2"),
    ("conditional_expression", "return if unknown then 1 else 2"),
];

const QUIET: &[(&str, &str)] = &[
    (
        "incomplete_swap",
        "local first, second = 1, 2\nfirst, second = second, first\nreturn first, second",
    ),
    ("not_a_number_comparison", "return 1 == 1"),
    (
        "table_identity_comparison",
        "local value = {}\nreturn value == value",
    ),
    ("division_by_zero", "return 1/2"),
    ("duplicate_table_key", "return {first = 1, second = 2}"),
    (
        "repeated_condition",
        "if first then print(1) elseif second then print(2) end",
    ),
    (
        "identical_branches",
        "if unknown then print(1) else print(2) end",
    ),
    (
        "invalid_reverse_loop",
        "for index = 10, 1, -1 do print(index) end",
    ),
    (
        "misplaced_type_comparison",
        "return type(unknown) == 'string'",
    ),
    (
        "unbalanced_assignment",
        "local first, second = pcall(print)\nreturn first, second",
    ),
    ("zero_loop_step", "for index = 1, 10 do print(index) end"),
    ("invalid_string_escape", "return '\\n'"),
    (
        "argument_count",
        "local function work(value: number) return value end\nreturn work(1)",
    ),
    ("discarded_return", "return math.abs(1)"),
    ("invalid_directive", "--!strict\nreturn 1"),
    (
        "builtin_assignment",
        "local value = 1\nvalue = 2\nreturn value",
    ),
    ("comparison_precedence", "return not (unknown == 1)"),
    (
        "duplicate_function",
        "local function first() end\nlocal function second() end\nreturn first, second",
    ),
    (
        "duplicate_binding",
        "local first, second = 1, 2\nreturn first, second",
    ),
    (
        "invalid_format_string",
        "return string.format('%s', 'value')",
    ),
    (
        "inconsistent_return",
        "local function work(condition) if condition then return 1 end return 2 end\nreturn work",
    ),
    ("misleading_conditional", "return unknown and 1 or 2"),
    ("number_literal_overflow", "return 0xff"),
    ("placeholder_read", "local _ = 1\nreturn 2"),
    ("invalid_table_operation", "table.insert({}, 1)"),
    ("untyped_local", "local value: number\nreturn value"),
    (
        "untyped_parameter",
        "local function work(value: number) return value end\nreturn work",
    ),
    ("uninitialized_local", "local value = 1\nreturn value"),
    ("invalid_type_name", "return type(1) == 'number'"),
    ("undefined_variable", "return math.pi"),
    (
        "global_assignment",
        "local value = 1\nvalue = 2\nreturn value",
    ),
    ("unused_function", "local function work() end\nreturn work"),
    ("unused_variable", "local value = 1\nreturn value"),
    (
        "unused_import",
        "local dependency = require('./dependency')\nreturn dependency",
    ),
    (
        "shadowed_binding",
        "do local value = 1 print(value) end\ndo local value = 2 print(value) end",
    ),
    ("global_environment", "local _G = {}\nreturn _G"),
    (
        "deprecated_function",
        "local table = {getn = function() return 1 end}\nreturn table.getn()",
    ),
    (
        "restricted_import",
        "local require = function(value) return value end\nreturn require('./dependency')",
    ),
    (
        "function_complexity",
        "local function work() return 1 end\nreturn work",
    ),
    (
        "manual_table_clone",
        "local source, target = {}, {}\nfor key, value in pairs(source) do target[key] = value + 1 end\nreturn target",
    ),
    (
        "constant_binding",
        "local value = 1\nvalue = 2\nreturn value",
    ),
    ("restricted_global", "local print = function() end\nprint()"),
    (
        "constant_import",
        "const dependency = require('./dependency')\nreturn dependency",
    ),
    ("unreachable_code", "if unknown then print(1) end\nprint(2)"),
    (
        "self_assignment",
        "local first, second = 1, 2\nfirst = second\nreturn first",
    ),
    (
        "loop_string_concatenation",
        "for index = 1, 3 do local value = tostring(index) .. 'suffix' print(value) end",
    ),
    (
        "loop_invariant_call",
        "local dependency = require('./dependency')\nfor index = 1, 3 do print(index, dependency) end",
    ),
    ("length_condition", "if #{} > 0 then print(1) end"),
    ("shadowed_builtin", "local value = 1\nreturn value"),
    ("ignored_protected_call", "return pcall(print)"),
    ("empty_branch", "if unknown then print(1) end"),
    ("empty_loop", "while unknown do print(1) end"),
    ("mixed_table", "return {1, 2}"),
    ("multiple_statements", "print(1)\nprint(2)"),
    ("parenthesized_condition", "if unknown then print(1) end"),
    ("constant_condition", "if unknown then print(1) end"),
    (
        "redundant_else",
        "if unknown then print(1) else print(2) end",
    ),
    ("nested_condition", "if first and second then print(1) end"),
    (
        "negated_condition",
        "if unknown then print(1) else print(2) end",
    ),
    ("logical_conditional", "return if unknown then 1 else 2"),
    (
        "conditional_expression",
        "if unknown then return 1 else return 2 end",
    ),
];

#[test]
fn registered_rules_report_their_triggering_cases() -> TestResult {
    let directory = tempfile::tempdir()?;
    let mut settings = String::from("[lint.rules]\n");

    for rule in lint::registry::RULES {
        writeln!(settings, "{} = 'warn'", rule.name)?;
    }

    settings.push_str("[lint.options.function_complexity]\nmaximum_complexity = 1\n[lint.options.restricted_import.paths]\n'./dependency' = 'restricted'\n[lint.options.restricted_global]\nprint = 'restricted'\n");
    fs::write(directory.path().join("instar.toml"), settings)?;
    fs::write(directory.path().join("dependency.luau"), "return 1")?;
    assert_eq!(CASES.len() + 3, lint::registry::RULES.len());

    assert_eq!(
        CASES.iter().map(|(name, _)| name).collect::<Vec<_>>(),
        QUIET.iter().map(|(name, _)| name).collect::<Vec<_>>()
    );

    for &(rule, text) in CASES {
        let mut sources = SourceStore::default();
        let source = sources.open(&directory.path().join("source.luau"), 1, text)?;

        let report = lint::analyze(
            &mut analysis::Session::default(),
            &mut sources,
            source.path(),
        )?;

        assert!(
            report.findings.iter().any(|finding| finding.rule == rule),
            "missing {rule}: {:?}",
            report.findings
        );
    }

    Ok(())
}

#[test]
fn registered_rules_accept_their_nearby_cases() -> TestResult {
    let directory = tempfile::tempdir()?;
    let mut settings = String::from("[lint.rules]\n");

    for rule in lint::registry::RULES {
        writeln!(settings, "{} = 'warn'", rule.name)?;
    }

    settings.push_str("[lint.options.function_complexity]\nmaximum_complexity = 1\n[lint.options.restricted_import.paths]\n'./dependency' = 'restricted'\n[lint.options.restricted_global]\nprint = 'restricted'\n");
    fs::write(directory.path().join("instar.toml"), settings)?;
    fs::write(directory.path().join("dependency.luau"), "return 1")?;

    for &(rule, text) in QUIET {
        let mut sources = SourceStore::default();
        let source = sources.open(&directory.path().join("source.luau"), 1, text)?;

        let report = lint::analyze(
            &mut analysis::Session::default(),
            &mut sources,
            source.path(),
        )?;

        assert!(
            report.findings.iter().all(|finding| finding.rule != rule),
            "unexpected {rule}: {:?}",
            report.findings
        );
    }

    Ok(())
}

#[test]
fn grouped_expressions_keep_their_rule_meaning() -> TestResult {
    let directory = tempfile::tempdir()?;

    for (rule, text) in [
        ("invalid_format_string", "return (string).format('%J', 1)"),
        ("constant_condition", "if (false) then print(1) end"),
    ] {
        let mut sources = SourceStore::default();
        let source = sources.open(&directory.path().join("source.luau"), 1, text)?;

        let report = lint::analyze(
            &mut analysis::Session::default(),
            &mut sources,
            source.path(),
        )?;

        assert!(
            report.findings.iter().any(|finding| finding.rule == rule),
            "{rule}: {:?}",
            report.findings
        );
    }

    Ok(())
}

#[test]
fn settings_inherit_options_and_validate_names() -> TestResult {
    let directory = tempfile::tempdir()?;
    let nested = directory.path().join("nested");
    fs::create_dir(&nested)?;

    fs::write(
        directory.path().join("instar.toml"),
        "[lint.rules]\nunused_variable = 'deny'\n[lint.options.unused_variable]\nparameters = true\n",
    )?;

    fs::write(
        nested.join("instar.toml"),
        "[lint.rules]\nunused_function = 'allow'\n",
    )?;

    let mut sources = SourceStore::default();

    let source = sources.open(
        &nested.join("source.luau"),
        1,
        "local function work(value) return 1 end\nreturn work",
    )?;

    let report = lint::analyze(
        &mut analysis::Session::default(),
        &mut sources,
        source.path(),
    )?;

    assert!(
        report
            .findings
            .iter()
            .any(|finding| finding.rule == "unused_variable"
                && finding.level == lint::configuration::Level::Deny),
        "{:?}",
        report.findings
    );

    fs::write(
        nested.join("instar.toml"),
        "[lint.options.unused_variable]\nparameters = false\n",
    )?;

    let report = lint::analyze(
        &mut analysis::Session::default(),
        &mut sources,
        source.path(),
    )?;

    assert!(
        report
            .findings
            .iter()
            .all(|finding| finding.rule != "unused_variable"),
        "{:?}",
        report.findings
    );

    for configuration in [
        "[lint.rules]\nmissing = 'warn'",
        "[lint.groups]\nmissing = 'warn'",
        "[lint.options.unused_variable]\nignore_pattern = '['",
    ] {
        fs::write(nested.join("instar.toml"), configuration)?;

        assert!(
            lint::analyze(
                &mut analysis::Session::default(),
                &mut sources,
                source.path()
            )
            .is_err(),
            "{configuration}"
        );
    }

    Ok(())
}

#[test]
fn edits_reject_conflicts_invalid_ranges_and_stale_files() -> TestResult {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("source.luau");
    fs::write(&path, "return 1")?;
    let mut sources = SourceStore::default();
    let source = sources.read(&path)?;

    assert!(
        lint::apply(
            &source,
            &[lint::Edit {
                start: 0,
                end: 99,
                text: String::new()
            }]
        )
        .is_err()
    );

    let finding = lint::Finding {
        rule: "unused_variable".into(),
        level: lint::configuration::Level::Warn,
        message: String::new(),
        start: 0,
        end: 1,
        edits: vec![
            lint::Edit {
                start: 0,
                end: 2,
                text: String::new(),
            },
            lint::Edit {
                start: 1,
                end: 3,
                text: String::new(),
            },
        ],
    };

    assert!(lint::edits(&[finding]).is_err());
    fs::write(&path, "return 2")?;
    assert!(lint::write(&source, &sources, b"return 3").is_err());
    assert_eq!(fs::read_to_string(path)?, "return 2");

    Ok(())
}

#[test]
fn binding_fixes_preserve_global_reads_and_type_imports() -> TestResult {
    let directory = tempfile::tempdir()?;

    fs::write(
        directory.path().join("dependency.luau"),
        "export type Value = number\nreturn {}",
    )?;

    let mut sources = SourceStore::default();
    let source = sources.open(&directory.path().join("source.luau"), 1, "local dependency = require('./dependency')\nlocal value: dependency.Value = 1\nreturn value")?;

    let report = lint::analyze(
        &mut analysis::Session::default(),
        &mut sources,
        source.path(),
    )?;

    assert!(
        report
            .findings
            .iter()
            .all(|finding| finding.rule != "unused_import"),
        "{:?}",
        report.findings
    );

    let source = sources.update(&source, 2, "local unused = 1\nreturn _unused")?;

    let report = lint::analyze(
        &mut analysis::Session::default(),
        &mut sources,
        source.path(),
    )?;

    let finding = report
        .findings
        .iter()
        .find(|finding| finding.rule == "unused_variable")
        .ok_or("finding")?;

    assert_eq!(finding.edits, []);

    Ok(())
}

#[test]
fn fixes_preserve_comments_unicode_and_are_idempotent() -> TestResult {
    let directory = tempfile::tempdir()?;
    let mut sources = SourceStore::default();

    let source = sources.open(
        &directory.path().join("source.luau"),
        1,
        "-- 😀\r\nlocal unused = 1\r\nreturn 2\r\n",
    )?;

    let mut session = analysis::Session::default();
    let report = lint::analyze(&mut session, &mut sources, source.path())?;
    let output = lint::apply(&source, &lint::edits(&report.findings)?)?;

    assert_eq!(
        String::from_utf8(output.clone())?,
        "-- 😀\r\nlocal _unused = 1\r\nreturn 2\r\n"
    );

    let source = sources.update(&source, 2, std::str::from_utf8(&output)?)?;
    session.change(source.path());
    let report = lint::analyze(&mut session, &mut sources, source.path())?;
    assert_eq!(lint::edits(&report.findings)?, []);

    Ok(())
}

#[test]
fn configuration_and_suppression_apply_to_snapshots() -> TestResult {
    let directory = tempfile::tempdir()?;
    let mut sources = SourceStore::default();

    sources.open(
        &directory.path().join("instar.toml"),
        1,
        "[lint.rules]\nconstant_binding = 'warn'\n",
    )?;

    let source = sources.open(
        &directory.path().join("source.luau"),
        1,
        "-- instar: allow(constant_binding, unused_variable)\nlocal value = 1\nreturn 2",
    )?;

    let report = lint::analyze(
        &mut analysis::Session::default(),
        &mut sources,
        source.path(),
    )?;

    assert!(report.findings.is_empty(), "{:?}", report.findings);

    Ok(())
}

#[test]
fn roblox_rules_require_the_environment() -> TestResult {
    let directory = tempfile::tempdir()?;
    fs::write(directory.path().join("instar.toml"), "[analyze.roblox]\n")?;
    support::configure(directory.path())?;
    let mut sources = SourceStore::default();

    let source = sources.open(
        &directory.path().join("source.luau"),
        1,
        "return Color3.new(255, 0, 0), UDim2.new(1, 2), UDim2.new(1, 0, 2, 0)",
    )?;

    let report = lint::analyze(
        &mut analysis::Session::default(),
        &mut sources,
        source.path(),
    )?;

    for name in [
        "color_bounds",
        "dimension_arguments",
        "dimension_constructor",
    ] {
        assert!(
            report.findings.iter().any(|finding| finding.rule == name),
            "{name}: {:?}",
            report.findings
        );
    }

    let source = sources.update(
        &source,
        2,
        "return Color3.new(0, 0, 0), UDim2.new(1, 2, 3, 4)",
    )?;

    let report = lint::analyze(
        &mut analysis::Session::default(),
        &mut sources,
        source.path(),
    )?;

    assert!(
        report
            .findings
            .iter()
            .all(|finding| lint::registry::find(&finding.rule)
                .is_none_or(|rule| rule.group != "roblox")),
        "{:?}",
        report.findings
    );

    fs::write(directory.path().join("instar.toml"), "")?;

    let source = sources.update(
        &source,
        3,
        "return Color3.new(255, 0, 0), UDim2.new(1, 2), UDim2.new(1, 0, 2, 0)",
    )?;

    let report = lint::analyze(
        &mut analysis::Session::default(),
        &mut sources,
        source.path(),
    )?;

    assert!(
        report
            .findings
            .iter()
            .all(|finding| lint::registry::find(&finding.rule)
                .is_none_or(|rule| rule.group != "roblox")),
        "{:?}",
        report.findings
    );

    Ok(())
}
