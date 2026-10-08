//! Positive and negative fixtures for every configured Instar rule.

use std::{
    cell::Cell,
    collections::{BTreeMap, BTreeSet},
    fs, io,
    path::Path,
    time::Duration,
};

use instar_analysis::{Cancellation, Completion, Kind as AnalysisKind, Options, Reason};
use instar_lint::{Configuration, Inference, Kind, Level, Require, Result as Output, Rule, Source};
use serde::Deserialize;
use toml::{Table, Value};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Fixture {
    rule: String,
    range: [usize; 2],
}

fn fixtures() -> BTreeMap<String, Fixture> {
    toml::from_str(include_str!("fixtures/inventory.toml")).expect("fixture inventory")
}

fn configured(rule: Rule) -> Configuration {
    let mut configuration: Table =
        toml::from_str(include_str!("fixtures/configuration.toml")).expect("fixture configuration");

    for (identity, _) in Configuration::default().rules() {
        configuration
            .entry(identity.name())
            .or_insert_with(|| Value::Table(Table::new()))
            .as_table_mut()
            .expect("rule configuration")
            .insert(
                "level".to_owned(),
                Value::String(if identity == rule { "warn" } else { "allow" }.to_owned()),
            );
    }

    configuration.try_into().expect("fixture configuration")
}

fn run(
    text: &str,
    configuration: &Configuration,
    requires: &[Require],
    inferred: Option<&[Inference]>,
) -> io::Result<Output<&'static str>> {
    instar_lint::lint(
        "fixture",
        &Source {
            text,
            revision: 7,
            configuration,
            globals: &[],
            roblox: true,
            requires,
            inferred,
        },
        &Options::new(Duration::from_secs(5)),
    )
}

fn assert_findings(result: &Output<&str>, rule: Rule, ranges: &[[usize; 2]]) {
    assert_eq!(result.completion, Completion::Complete, "{rule}");
    assert_eq!(result.modules, ["fixture"]);

    assert_eq!(
        result.diagnostics.len(),
        ranges.len(),
        "{rule}: {:?}",
        result.diagnostics
    );

    for (diagnostic, range) in result.diagnostics.iter().zip(ranges) {
        assert_eq!(diagnostic.kind, Kind::Rule(rule));
        assert_eq!(diagnostic.level, Level::Warn);
        assert_eq!(diagnostic.location.module, "fixture");
        assert_eq!(diagnostic.location.revision, 7);
        assert_eq!(&diagnostic.location.range, range);
        assert_eq!(diagnostic.related, Vec::new());
    }
}

#[test]
fn every_syntax_rule_has_positive_and_negative_fixtures() -> io::Result<()> {
    for (name, fixture) in fixtures() {
        let rule = Configuration::default()
            .rules()
            .into_iter()
            .find(|(rule, _)| rule.name() == fixture.rule)
            .expect("known fixture rule")
            .0;

        let configuration = configured(rule);

        let directory = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(name);

        let positive = fs::read_to_string(directory.join("positive.luau"))?;
        let negative = fs::read_to_string(directory.join("negative.luau"))?;
        let result = run(&positive, &configuration, &[], None)?;
        assert_findings(&result, rule, &[fixture.range]);
        assert_findings(&run(&negative, &configuration, &[], None)?, rule, &[]);
        assert!(result.diagnostics[0].location.range[1] <= positive.len());
    }

    Ok(())
}

#[test]
fn empty_conditional_branches_highlight_opening_headers() -> io::Result<()> {
    let configuration = configured(Rule::EmptyIf);

    for contents in ["", "-- pending", "--[[pending]]"] {
        let text = format!(
            "if flag then\n{contents}\nelseif other then\n{contents}\nelse\n{contents}\nend"
        );

        let ranges = ["if flag then", "elseif other then", "else"].map(|header| {
            let start = text.rfind(header).expect("branch header");

            [start, start + header.len()]
        });

        let result = run(&text, &configuration, &[], None)?;
        assert_findings(&result, Rule::EmptyIf, &ranges);

        for diagnostic in &result.diagnostics {
            assert_eq!(diagnostic.message, "empty conditional branch");
        }
    }

    let text = "if flag then return 1 elseif other then else return 2 end";
    let header = "elseif other then";
    let start = text.find(header).expect("branch header");

    assert_findings(
        &run(text, &configuration, &[], None)?,
        Rule::EmptyIf,
        &[[start, start + header.len()]],
    );

    Ok(())
}

#[test]
fn empty_loops_highlight_opening_headers() -> io::Result<()> {
    let configuration = configured(Rule::EmptyLoop);

    for (header, closing) in [
        ("while flag do", "end"),
        ("repeat", "until flag"),
        ("for index = 1, 4, 2 do", "end"),
        ("for key, value in pairs(values) do", "end"),
    ] {
        for contents in ["", "-- pending", "--[[pending]]"] {
            let text = format!("{header}\n{contents}\n{closing}");
            let result = run(&text, &configuration, &[], None)?;
            assert_findings(&result, Rule::EmptyLoop, &[[0, header.len()]]);
            assert_eq!(result.diagnostics[0].message, "empty loop body");
        }

        assert_findings(
            &run(
                &format!("{header}\n-- pending\nbreak\n{closing}"),
                &configuration,
                &[],
                None,
            )?,
            Rule::EmptyLoop,
            &[],
        );
    }

    Ok(())
}

#[test]
fn typed_require_fixtures() -> io::Result<()> {
    let configuration = configured(Rule::NonConstRequire);

    let dynamic = Require {
        call: [0, 13],
        argument: [8, 12],
        constant: false,
        path: None,
    };

    let constant = Require {
        call: [0, 18],
        argument: [8, 17],
        constant: true,
        path: Some("./clear".to_owned()),
    };

    assert_findings(
        &run(
            include_str!("fixtures/requires/dynamic.luau"),
            &configuration,
            &[dynamic],
            None,
        )?,
        Rule::NonConstRequire,
        &[[8, 12]],
    );

    assert_findings(
        &run(
            include_str!("fixtures/requires/clear.luau"),
            &configuration,
            &[constant],
            None,
        )?,
        Rule::NonConstRequire,
        &[],
    );

    let configuration = configured(Rule::RestrictedModulePaths);

    let blocked = Require {
        call: [0, 20],
        argument: [8, 19],
        constant: true,
        path: Some("./blocked".to_owned()),
    };

    let clear = Require {
        call: [0, 18],
        argument: [8, 17],
        constant: true,
        path: Some("./clear".to_owned()),
    };

    assert_findings(
        &run(
            include_str!("fixtures/requires/blocked.luau"),
            &configuration,
            &[blocked],
            None,
        )?,
        Rule::RestrictedModulePaths,
        &[[8, 19]],
    );

    assert_findings(
        &run(
            include_str!("fixtures/requires/clear.luau"),
            &configuration,
            &[clear],
            None,
        )?,
        Rule::RestrictedModulePaths,
        &[],
    );

    Ok(())
}

#[test]
fn native_inference_fixtures_and_unavailable_capability() -> io::Result<()> {
    for (rule, text, parameter) in [
        (
            Rule::ImplicitAnyLocal,
            include_str!("fixtures/inference/local.luau"),
            false,
        ),
        (
            Rule::ImplicitAnyParameter,
            include_str!("fixtures/inference/parameter.luau"),
            true,
        ),
    ] {
        let configuration = configured(rule);
        let start = text.find("value").expect("binding name");

        let inference = Inference {
            range: [start, start + 5],
            parameter,
        };

        assert_findings(
            &run(text, &configuration, &[], Some(&[inference]))?,
            rule,
            &[[start, start + 5]],
        );

        assert_findings(&run(text, &configuration, &[], Some(&[]))?, rule, &[]);
        let unsupported = run(text, &configuration, &[], None)?;

        assert_eq!(
            unsupported.completion,
            Completion::Incomplete(Reason::Unsupported)
        );

        assert_eq!(unsupported.modules, ["fixture"]);
        assert_eq!(unsupported.diagnostics.len(), 1);
        let diagnostic = &unsupported.diagnostics[0];

        assert_eq!(diagnostic.kind, Kind::Analysis(AnalysisKind::Unsupported));

        assert_eq!(diagnostic.level, Level::Deny);
        assert_eq!(diagnostic.location.module, "fixture");
        assert_eq!(diagnostic.location.revision, 7);
        assert_eq!(diagnostic.location.range, [0, 0]);
        assert_eq!(diagnostic.related, Vec::new());
    }

    Ok(())
}

#[test]
fn fixture_inventory_matches_configuration() {
    let fixtures = fixtures();

    let covered = fixtures
        .values()
        .map(|fixture| fixture.rule.as_str())
        .chain(
            [
                Rule::NonConstRequire,
                Rule::RestrictedModulePaths,
                Rule::ImplicitAnyLocal,
                Rule::ImplicitAnyParameter,
            ]
            .map(Rule::name),
        )
        .collect::<BTreeSet<_>>();

    let configured = Configuration::default()
        .rules()
        .into_iter()
        .map(|(rule, _)| rule.name())
        .collect::<BTreeSet<_>>();

    assert_eq!(covered, configured);
    assert_eq!(configured.len(), 41);
}

#[test]
fn rule_severity_allow_and_cooperative_limits() -> io::Result<()> {
    let mut configuration = configured(Rule::DivideByZero);

    for level in [Level::Info, Level::Warn, Level::Deny] {
        configuration.divide_by_zero.level = level;

        assert_eq!(
            run("return 1/0", &configuration, &[], None)?.diagnostics[0].level,
            level
        );
    }

    configuration.divide_by_zero.level = Level::Allow;

    assert_findings(
        &run("return 1/0", &configuration, &[], None)?,
        Rule::DivideByZero,
        &[],
    );

    let source = Source {
        text: "return 1",
        revision: 7,
        configuration: &configuration,
        globals: &[],
        roblox: false,
        requires: &[],
        inferred: None,
    };

    let options = Options::new(Duration::ZERO);

    assert_eq!(
        instar_lint::lint("fixture", &source, &options)?.completion,
        Completion::Incomplete(Reason::Timeout)
    );

    let options = Options::new(Duration::from_secs(5));
    options.cancellation.cancel();

    assert_eq!(
        instar_lint::lint("fixture", &source, &options)?.completion,
        Completion::Incomplete(Reason::Cancelled)
    );

    Ok(())
}

#[test]
fn lexical_binding_options_and_shadowing() -> io::Result<()> {
    let mut configuration = configured(Rule::UnusedVariable);
    configuration.unused_variable.parameters = true;
    configuration.unused_variable.loop_variables = true;
    configuration.unused_variable.ignore_pattern = "^skip".to_owned();

    let result = run(
        include_str!("fixtures/shadowing/input.luau"),
        &configuration,
        &[],
        None,
    )?;

    let source = include_str!("fixtures/shadowing/input.luau");
    let parameter = source.find("value").expect("parameter");
    let binding = source.find("kept").expect("loop binding");

    assert_findings(
        &result,
        Rule::UnusedVariable,
        &[[parameter, parameter + 5], [binding, binding + 4]],
    );

    let mut configuration = configured(Rule::PreferConst);
    configuration.prefer_const.mutated_tables_stay_local = true;

    assert_findings(
        &run(
            include_str!("fixtures/shadowing/mutation.luau"),
            &configuration,
            &[],
            None,
        )?,
        Rule::PreferConst,
        &[],
    );

    assert_findings(
        &run(
            include_str!("fixtures/shadowing/table.luau"),
            &configuration,
            &[],
            None,
        )?,
        Rule::PreferConst,
        &[[6, 7]],
    );

    assert_findings(
        &run(
            include_str!("fixtures/shadowing/function.luau"),
            &configured(Rule::MismatchedArgumentCount),
            &[],
            None,
        )?,
        Rule::MismatchedArgumentCount,
        &[],
    );

    assert_findings(
        &run(
            include_str!("fixtures/shadowing/protection.luau"),
            &configured(Rule::IgnoredPcallResult),
            &[],
            None,
        )?,
        Rule::IgnoredPcallResult,
        &[],
    );

    assert_findings(
        &run(
            include_str!("fixtures/shadowing/color.luau"),
            &configured(Rule::RobloxIncorrectColor3NewBounds),
            &[],
            None,
        )?,
        Rule::RobloxIncorrectColor3NewBounds,
        &[],
    );

    Ok(())
}

#[test]
fn syntax_errors_are_explicit() -> io::Result<()> {
    let result = run(
        include_str!("fixtures/syntax/input.luau"),
        &Configuration::default(),
        &[],
        None,
    )?;

    let tree = vermis::parse(include_bytes!("fixtures/syntax/input.luau"));
    assert_eq!(result.completion, Completion::Complete);
    assert_eq!(result.modules, ["fixture"]);
    assert_ne!(result.diagnostics, Vec::new());
    assert_eq!(result.diagnostics.len(), tree.diagnostics.len());

    for (diagnostic, expected) in result.diagnostics.iter().zip(&tree.diagnostics) {
        assert_eq!(
            diagnostic.kind,
            Kind::Analysis(AnalysisKind::Syntax { code: None })
        );

        assert_eq!(diagnostic.level, Level::Deny);
        assert_eq!(diagnostic.location.module, "fixture");
        assert_eq!(diagnostic.location.revision, 7);

        assert_eq!(
            diagnostic.location.range,
            [expected.span.start, expected.span.end]
        );

        assert_eq!(diagnostic.message, expected.message);
        assert_eq!(diagnostic.related, Vec::new());
    }

    Ok(())
}

#[test]
fn service_rules_use_literal_values_and_byte_escapes_remain_valid() -> io::Result<()> {
    let configuration = configured(Rule::RobloxPreferGetPlayers);

    for text in [
        include_str!("fixtures/services/long.luau"),
        include_str!("fixtures/services/escaped.luau"),
    ] {
        assert_findings(
            &run(text, &configuration, &[], None)?,
            Rule::RobloxPreferGetPlayers,
            &[[7, text.len()]],
        );
    }

    assert_findings(
        &run(
            include_str!("fixtures/services/other.luau"),
            &configuration,
            &[],
            None,
        )?,
        Rule::RobloxPreferGetPlayers,
        &[],
    );

    let configuration = configured(Rule::BadStringEscape);

    assert_findings(
        &run(
            include_str!("fixtures/escapes/byte.luau"),
            &configuration,
            &[],
            None,
        )?,
        Rule::BadStringEscape,
        &[],
    );

    Ok(())
}

#[test]
fn cancellation_during_diagnostic_construction_updates_completion() -> io::Result<()> {
    struct Module<'signal> {
        copies: &'signal Cell<usize>,
        cancellation: &'signal Cancellation,
    }

    impl Clone for Module<'_> {
        fn clone(&self) -> Self {
            self.copies.set(self.copies.get() + 1);

            if self.copies.get() == 2 {
                self.cancellation.cancel();
            }

            Self {
                copies: self.copies,
                cancellation: self.cancellation,
            }
        }
    }

    let options = Options::new(Duration::from_secs(5));
    let copies = Cell::new(0);
    let configuration = configured(Rule::DivideByZero);

    let result = instar_lint::lint(
        Module {
            copies: &copies,
            cancellation: &options.cancellation,
        },
        &Source {
            text: "return 1/0",
            revision: 1,
            configuration: &configuration,
            globals: &[],
            roblox: false,
            requires: &[],
            inferred: None,
        },
        &options,
    )?;

    assert!(copies.get() >= 2);
    assert_eq!(result.completion, Completion::Incomplete(Reason::Cancelled));

    Ok(())
}
