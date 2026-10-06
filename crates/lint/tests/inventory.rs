//! Positive and negative fixtures for every configured Instar rule.

use std::{collections::BTreeSet, fmt::Write as _, io, time::Duration};

use instar_lint::{
    Completion, Configuration, Inference, Kind, Level, Options, Reason, Require, Rule, Source,
};

struct Fixture {
    rule: Rule,
    positive: &'static str,
    negative: &'static str,
}

const FIXTURES: &[Fixture] = &[
    Fixture {
        rule: Rule::AlmostSwapped,
        positive: "local a,b=1,2\na=b\nb=a\nreturn a+b",
        negative: "local a,b=1,2\na,b=b,a\nreturn a+b",
    },
    Fixture {
        rule: Rule::BadStringEscape,
        positive: r#"return "\q""#,
        negative: r#"return "\n""#,
    },
    Fixture {
        rule: Rule::CompareNan,
        positive: "return 0/0 == 1",
        negative: "return 0/1 == 1",
    },
    Fixture {
        rule: Rule::ConstantCondition,
        positive: "if true then print(1) end",
        negative: "if flag then print(1) end",
    },
    Fixture {
        rule: Rule::ConstantTableComparison,
        positive: "return {} == value",
        negative: "return value == other",
    },
    Fixture {
        rule: Rule::LengthAsCondition,
        positive: "if #list then print(1) end",
        negative: "if #list > 0 then print(1) end",
    },
    Fixture {
        rule: Rule::MismatchedArgumentCount,
        positive: "local function f(a,b) return a,b end\nreturn f(1)",
        negative: "local function f(a,b) return a,b end\nreturn f(1,2)",
    },
    Fixture {
        rule: Rule::MustUse,
        positive: "math.abs(-1)",
        negative: "return math.abs(-1)",
    },
    Fixture {
        rule: Rule::ZeroStepLoop,
        positive: "for i=1,10,0 do print(i) end",
        negative: "for i=1,10,1 do print(i) end",
    },
    Fixture {
        rule: Rule::DivideByZero,
        positive: "return 1/0.00",
        negative: "return 1/1",
    },
    Fixture {
        rule: Rule::SelfAssignment,
        positive: "local value=1\nvalue=value\nreturn value",
        negative: "local value=1\nvalue=2\nreturn value",
    },
    Fixture {
        rule: Rule::EmptyIf,
        positive: "if flag then end",
        negative: "if flag then print(1) end",
    },
    Fixture {
        rule: Rule::EmptyLoop,
        positive: "while flag do end",
        negative: "while flag do break end",
    },
    Fixture {
        rule: Rule::IfSameThenElse,
        positive: "if flag then print(1) else print(1) end",
        negative: "if flag then print(1) else print(2) end",
    },
    Fixture {
        rule: Rule::IgnoredPcallResult,
        positive: "pcall(print,1)",
        negative: "local ok=pcall(print,1)\nreturn ok",
    },
    Fixture {
        rule: Rule::MixedTable,
        positive: "return {1,key=2}",
        negative: "return {key=1,other=2}",
    },
    Fixture {
        rule: Rule::UnscopedVariables,
        positive: "created=1",
        negative: "local created=1\ncreated=2\nreturn created",
    },
    Fixture {
        rule: Rule::RobloxIncorrectColor3NewBounds,
        positive: "return Color3.new(255,0,0)",
        negative: "return Color3.new(1,0,0)",
    },
    Fixture {
        rule: Rule::RobloxManualFromscaleOrFromoffset,
        positive: "return UDim2.new(0.5,0,0.2,0)",
        negative: "return UDim2.new(0.5,10,0.2,5)",
    },
    Fixture {
        rule: Rule::RobloxPreferGetPlayers,
        positive: "local Players=game:GetService('Players')\nreturn Players:GetChildren()",
        negative: "local Folder=game:GetService('ReplicatedStorage')\nreturn Folder:GetChildren()",
    },
    Fixture {
        rule: Rule::RobloxSuspiciousUdim2New,
        positive: "return UDim2.new(1,2)",
        negative: "return UDim2.new(1,2,3,4)",
    },
    Fixture {
        rule: Rule::AndOrConditional,
        positive: "return flag and 1 or 2",
        negative: "return flag and false or 2",
    },
    Fixture {
        rule: Rule::CollapsibleIf,
        positive: "if a then if b then print(1) end end",
        negative: "if a then print(1) end",
    },
    Fixture {
        rule: Rule::ElseAfterReturn,
        positive: "local function f(x) if x then return 1 else return 2 end end\nreturn f",
        negative: "local function f(x) if x then print(1) else return 2 end end\nreturn f",
    },
    Fixture {
        rule: Rule::IfExpressionAssignment,
        positive: "local v\nif flag then v=1 else v=2 end\nreturn v",
        negative: "local v=if flag then 1 else 2\nreturn v",
    },
    Fixture {
        rule: Rule::NegatedCondition,
        positive: "if not flag then print(1) else print(2) end",
        negative: "if flag then print(1) else print(2) end",
    },
    Fixture {
        rule: Rule::ParenthesizedConditions,
        positive: "if (flag) then print(1) end",
        negative: "if flag then print(1) end",
    },
    Fixture {
        rule: Rule::GlobalUsage,
        positive: "return external",
        negative: "local external=1\nreturn external",
    },
    Fixture {
        rule: Rule::TypeCheckInsideCall,
        positive: "return type(1 == 2)",
        negative: "return type(1) == 'number'",
    },
    Fixture {
        rule: Rule::LoopInvariantCall,
        positive: "while running do game:GetService('Players') end",
        negative: "local Players=game:GetService('Players')\nwhile running do print(Players) end",
    },
    Fixture {
        rule: Rule::ManualTableClone,
        positive: "local dest={}\nfor k,v in pairs(src) do dest[k]=v end\nreturn dest",
        negative: "local dest={}\nfor k,v in pairs(src) do dest[k]=v print(k) end\nreturn dest",
    },
    Fixture {
        rule: Rule::StringConcatInLoop,
        positive: "local s=''\nwhile running do s=s..'x' end\nreturn s",
        negative: "local s=''\nwhile running do local t=s..'x' print(t) end",
    },
    Fixture {
        rule: Rule::UnusedVariable,
        positive: "local unused=1\nreturn 2",
        negative: "local used=1\nreturn used",
    },
    Fixture {
        rule: Rule::HighCyclomaticComplexity,
        positive: "local function f(x) if x then return 1 end while x do break end end\nreturn f",
        negative: "local function f(x) if x then return 1 end end\nreturn f",
    },
    Fixture {
        rule: Rule::PreferConst,
        positive: "local value=1\nreturn value",
        negative: "local value=1\nvalue=2\nreturn value",
    },
    Fixture {
        rule: Rule::Deprecated,
        positive: "old.api()",
        negative: "new.api()",
    },
    Fixture {
        rule: Rule::RestrictedGlobals,
        positive: "print(1)",
        negative: "local print=function() end\nprint(1)",
    },
];

fn configured(rule: Rule) -> Configuration {
    let mut source = String::new();

    for (name, _) in Configuration::default().rules() {
        writeln!(source, "[{name}]\nlevel='allow'").expect("fixture configuration string");
    }

    source = source.replace(
        &format!("[{rule}]\nlevel='allow'"),
        &format!("[{rule}]\nlevel='warn'"),
    );

    let mut configuration: Configuration = toml::from_str(&source).expect("fixture configuration");

    configuration.high_cyclomatic_complexity.maximum_complexity =
        std::num::NonZeroUsize::new(2).expect("nonzero fixture maximum");

    configuration
        .deprecated
        .paths
        .insert("old.api".to_owned(), "new.api".to_owned());

    configuration
        .restricted_globals
        .names
        .insert("print".to_owned(), "use logging".to_owned());

    configuration
        .restricted_module_paths
        .paths
        .insert("./blocked".to_owned(), "blocked".to_owned());

    configuration
}

fn run(
    text: &str,
    configuration: &Configuration,
    requires: &[Require],
    inferred: Option<&[Inference]>,
) -> io::Result<instar_lint::Result<&'static str>> {
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

fn contains(result: &instar_lint::Result<&str>, rule: Rule) -> bool {
    result
        .diagnostics
        .iter()
        .any(|diagnostic| diagnostic.kind == Kind::Rule(rule))
}

#[test]
fn every_syntax_rule_has_positive_and_negative_fixtures() -> io::Result<()> {
    for fixture in FIXTURES {
        let configuration = configured(fixture.rule);
        let positive = run(fixture.positive, &configuration, &[], None)?;
        let negative = run(fixture.negative, &configuration, &[], None)?;

        assert_eq!(
            positive.completion,
            Completion::Complete,
            "{}",
            fixture.rule
        );

        assert_eq!(
            negative.completion,
            Completion::Complete,
            "{}",
            fixture.rule
        );

        assert!(
            contains(&positive, fixture.rule),
            "{}: {:?}",
            fixture.rule,
            positive.diagnostics
        );

        assert!(
            !contains(&negative, fixture.rule),
            "{}: {:?}",
            fixture.rule,
            negative.diagnostics
        );

        assert!(
            positive
                .diagnostics
                .iter()
                .all(|diagnostic| diagnostic.location.revision == 7
                    && diagnostic.location.range[1] <= fixture.positive.len())
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

    assert!(contains(
        &run("require(path)", &configuration, &[dynamic], None)?,
        Rule::NonConstRequire
    ));

    assert!(!contains(
        &run("require('./clear')", &configuration, &[constant], None)?,
        Rule::NonConstRequire
    ));

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

    assert!(contains(
        &run("require('./blocked')", &configuration, &[blocked], None)?,
        Rule::RestrictedModulePaths
    ));

    assert!(!contains(
        &run("require('./clear')", &configuration, &[clear], None)?,
        Rule::RestrictedModulePaths
    ));

    Ok(())
}

#[test]
fn native_inference_fixtures_and_unavailable_capability() -> io::Result<()> {
    for (rule, text, parameter) in [
        (
            Rule::ImplicitAnyLocal,
            "local value=external\nreturn value",
            false,
        ),
        (
            Rule::ImplicitAnyParameter,
            "local function f(value) return value end\nreturn f",
            true,
        ),
    ] {
        let configuration = configured(rule);
        let start = text.find("value").expect("binding name");

        let inference = Inference {
            range: [start, start + 5],
            parameter,
        };

        assert!(contains(
            &run(text, &configuration, &[], Some(&[inference]))?,
            rule
        ));

        assert!(!contains(&run(text, &configuration, &[], Some(&[]))?, rule));
        let unsupported = run(text, &configuration, &[], None)?;

        assert_eq!(
            unsupported.completion,
            Completion::Incomplete(Reason::Unsupported)
        );

        assert!(
            unsupported
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.kind == Kind::Unsupported)
        );
    }

    Ok(())
}

#[test]
fn fixture_inventory_matches_configuration() {
    let covered = FIXTURES
        .iter()
        .map(|fixture| fixture.rule)
        .chain([
            Rule::NonConstRequire,
            Rule::RestrictedModulePaths,
            Rule::ImplicitAnyLocal,
            Rule::ImplicitAnyParameter,
        ])
        .collect::<BTreeSet<_>>();

    let configured = Configuration::default()
        .rules()
        .into_iter()
        .map(|(name, _)| name)
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

    assert!(!contains(
        &run("return 1/0", &configuration, &[], None)?,
        Rule::DivideByZero
    ));

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
        "local function f(value) do local value=1 print(value) end end\nfor skip,kept in iterator do print(1) end\nreturn f",
        &configuration,
        &[],
        None,
    )?;

    assert_eq!(result.diagnostics.len(), 2);
    let mut configuration = configured(Rule::PreferConst);
    configuration.prefer_const.mutated_tables_stay_local = true;

    assert!(!contains(
        &run("local t={}\nt.field=1\nreturn t", &configuration, &[], None)?,
        Rule::PreferConst
    ));

    assert!(contains(
        &run(
            "local t={}\nlocal function f(t) t.field=1 end\nreturn t,f",
            &configuration,
            &[],
            None
        )?,
        Rule::PreferConst
    ));

    assert!(!contains(
        &run(
            "local function f(a,b) return a,b end\nlocal function g(f) return f(1) end\nreturn g",
            &configured(Rule::MismatchedArgumentCount),
            &[],
            None
        )?,
        Rule::MismatchedArgumentCount
    ));

    assert!(!contains(
        &run(
            "local pcall=function() end\npcall()",
            &configured(Rule::IgnoredPcallResult),
            &[],
            None
        )?,
        Rule::IgnoredPcallResult
    ));

    assert!(!contains(
        &run(
            "local Color3={new=function() end}\nColor3.new(255)",
            &configured(Rule::RobloxIncorrectColor3NewBounds),
            &[],
            None
        )?,
        Rule::RobloxIncorrectColor3NewBounds
    ));

    Ok(())
}

#[test]
fn syntax_errors_are_explicit() -> io::Result<()> {
    let result = run("local =", &Configuration::default(), &[], None)?;

    assert!(
        result
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.kind == Kind::Syntax && diagnostic.level == Level::Deny)
    );

    Ok(())
}

#[test]
fn service_rules_use_literal_values_and_byte_escapes_remain_valid() -> io::Result<()> {
    let configuration = configured(Rule::RobloxPreferGetPlayers);

    for text in [
        "return game:GetService([=[Players]=]):GetChildren()",
        r#"return game:GetService("Pla\121ers"):GetChildren()"#,
    ] {
        assert!(contains(
            &run(text, &configuration, &[], None)?,
            Rule::RobloxPreferGetPlayers
        ));
    }

    assert!(!contains(
        &run(
            r#"return game:GetService("Other"):GetChildren()"#,
            &configuration,
            &[],
            None
        )?,
        Rule::RobloxPreferGetPlayers
    ));

    let configuration = configured(Rule::BadStringEscape);

    assert!(!contains(
        &run(r#"return "\255""#, &configuration, &[], None)?,
        Rule::BadStringEscape
    ));

    Ok(())
}
