//! Positive and negative fixtures for every configured Instar rule.
use std::{collections::BTreeSet, fmt::Write as _, io, time::Duration};

use instar_lint::{
    Completion, Configuration, Inference, Kind, Level, Options, Reason, Require, Rule, Source,
};

const FIXTURES: &[Fixture] = &[
    Fixture {
        rule: Rule::AlmostSwapped,
        positive: "local a,b=1,2\na=b\nb=a\nreturn a+b",
        range: [18, 21],
        negative: "local a,b=1,2\na,b=b,a\nreturn a+b",
    },
    Fixture {
        rule: Rule::BadStringEscape,
        positive: r#"return "\q""#,
        range: [7, 11],
        negative: r#"return "\n""#,
    },
    Fixture {
        rule: Rule::CompareNan,
        positive: "return 0/0 == 1",
        range: [7, 15],
        negative: "return 0/1 == 1",
    },
    Fixture {
        rule: Rule::ConstantCondition,
        positive: "if true then print(1) end",
        range: [3, 7],
        negative: "if flag then print(1) end",
    },
    Fixture {
        rule: Rule::ConstantTableComparison,
        positive: "return {} == value",
        range: [7, 18],
        negative: "return value == other",
    },
    Fixture {
        rule: Rule::LengthAsCondition,
        positive: "if #list then print(1) end",
        range: [3, 8],
        negative: "if #list > 0 then print(1) end",
    },
    Fixture {
        rule: Rule::MismatchedArgumentCount,
        positive: "local function f(a,b) return a,b end\nreturn f(1)",
        range: [44, 48],
        negative: "local function f(a,b) return a,b end\nreturn f(1,2)",
    },
    Fixture {
        rule: Rule::MustUse,
        positive: "math.abs(-1)",
        range: [0, 12],
        negative: "return math.abs(-1)",
    },
    Fixture {
        rule: Rule::ZeroStepLoop,
        positive: "for i=1,10,0 do print(i) end",
        range: [11, 12],
        negative: "for i=1,10,1 do print(i) end",
    },
    Fixture {
        rule: Rule::DivideByZero,
        positive: "return 1/0.00",
        range: [9, 13],
        negative: "return 1/1",
    },
    Fixture {
        rule: Rule::SelfAssignment,
        positive: "local value=1\nvalue=value\nreturn value",
        range: [14, 19],
        negative: "local value=1\nvalue=2\nreturn value",
    },
    Fixture {
        rule: Rule::EmptyIf,
        positive: "if flag then end",
        range: [13, 13],
        negative: "if flag then print(1) end",
    },
    Fixture {
        rule: Rule::EmptyLoop,
        positive: "while flag do end",
        range: [14, 14],
        negative: "while flag do break end",
    },
    Fixture {
        rule: Rule::IfSameThenElse,
        positive: "if flag then print(1) else print(1) end",
        range: [0, 39],
        negative: "if flag then print(1) else print(2) end",
    },
    Fixture {
        rule: Rule::IgnoredPcallResult,
        positive: "pcall(print,1)",
        range: [0, 14],
        negative: "local ok=pcall(print,1)\nreturn ok",
    },
    Fixture {
        rule: Rule::MixedTable,
        positive: "return {1,key=2}",
        range: [7, 16],
        negative: "return {key=1,other=2}",
    },
    Fixture {
        rule: Rule::UnscopedVariables,
        positive: "created=1",
        range: [0, 7],
        negative: "local created=1\ncreated=2\nreturn created",
    },
    Fixture {
        rule: Rule::RobloxIncorrectColor3NewBounds,
        positive: "return Color3.new(255,0,0)",
        range: [7, 26],
        negative: "return Color3.new(1,0,0)",
    },
    Fixture {
        rule: Rule::RobloxManualFromscaleOrFromoffset,
        positive: "return UDim2.new(0.5,0,0.2,0)",
        range: [7, 29],
        negative: "return UDim2.new(0.5,10,0.2,5)",
    },
    Fixture {
        rule: Rule::RobloxPreferGetPlayers,
        positive: "local Players=game:GetService('Players')\nreturn Players:GetChildren()",
        range: [48, 69],
        negative: "local Folder=game:GetService('ReplicatedStorage')\nreturn Folder:GetChildren()",
    },
    Fixture {
        rule: Rule::RobloxSuspiciousUdim2New,
        positive: "return UDim2.new(1,2)",
        range: [7, 21],
        negative: "return UDim2.new(1,2,3,4)",
    },
    Fixture {
        rule: Rule::AndOrConditional,
        positive: "return flag and 1 or 2",
        range: [7, 22],
        negative: "return flag and false or 2",
    },
    Fixture {
        rule: Rule::CollapsibleIf,
        positive: "if a then if b then print(1) end end",
        range: [0, 36],
        negative: "if a then print(1) end",
    },
    Fixture {
        rule: Rule::ElseAfterReturn,
        positive: "local function f(x) if x then return 1 else return 2 end end\nreturn f",
        range: [39, 52],
        negative: "local function f(x) if x then print(1) else return 2 end end\nreturn f",
    },
    Fixture {
        rule: Rule::IfExpressionAssignment,
        positive: "local v\nif flag then v=1 else v=2 end\nreturn v",
        range: [8, 37],
        negative: "local v=if flag then 1 else 2\nreturn v",
    },
    Fixture {
        rule: Rule::NegatedCondition,
        positive: "if not flag then print(1) else print(2) end",
        range: [3, 11],
        negative: "if flag then print(1) else print(2) end",
    },
    Fixture {
        rule: Rule::ParenthesizedConditions,
        positive: "if (flag) then print(1) end",
        range: [3, 9],
        negative: "if flag then print(1) end",
    },
    Fixture {
        rule: Rule::GlobalUsage,
        positive: "return external",
        range: [7, 15],
        negative: "local external=1\nreturn external",
    },
    Fixture {
        rule: Rule::TypeCheckInsideCall,
        positive: "return type(1 == 2)",
        range: [7, 19],
        negative: "return type(1) == 'number'",
    },
    Fixture {
        rule: Rule::LoopInvariantCall,
        positive: "while running do game:GetService('Players') end",
        range: [17, 43],
        negative: "local Players=game:GetService('Players')\nwhile running do print(Players) end",
    },
    Fixture {
        rule: Rule::ManualTableClone,
        positive: "local dest={}\nfor k,v in pairs(src) do dest[k]=v end\nreturn dest",
        range: [14, 52],
        negative: "local dest={}\nfor k,v in pairs(src) do dest[k]=v print(k) end\nreturn dest",
    },
    Fixture {
        rule: Rule::StringConcatInLoop,
        positive: "local s=''\nwhile running do s=s..'x' end\nreturn s",
        range: [30, 36],
        negative: "local s=''\nwhile running do local t=s..'x' print(t) end",
    },
    Fixture {
        rule: Rule::UnusedVariable,
        positive: "local unused=1\nreturn 2",
        range: [6, 12],
        negative: "local used=1\nreturn used",
    },
    Fixture {
        rule: Rule::HighCyclomaticComplexity,
        positive: "local function f(x) if x then return 1 end while x do break end end\nreturn f",
        range: [0, 67],
        negative: "local function f(x) if x then return 1 end end\nreturn f",
    },
    Fixture {
        rule: Rule::PreferConst,
        positive: "local value=1\nreturn value",
        range: [6, 11],
        negative: "local value=1\nvalue=2\nreturn value",
    },
    Fixture {
        rule: Rule::Deprecated,
        positive: "old.api()",
        range: [0, 7],
        negative: "new.api()",
    },
    Fixture {
        rule: Rule::RestrictedGlobals,
        positive: "print(1)",
        range: [0, 5],
        negative: "local print=function() end\nprint(1)",
    },
];

struct Fixture {
    rule: Rule,
    positive: &'static str,
    negative: &'static str,
    range: [usize; 2],
}

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

fn assert_findings(result: &instar_lint::Result<&str>, rule: Rule, ranges: &[[usize; 2]]) {
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
    for fixture in FIXTURES {
        let configuration = configured(fixture.rule);
        let positive = run(fixture.positive, &configuration, &[], None)?;
        let negative = run(fixture.negative, &configuration, &[], None)?;
        assert_findings(&positive, fixture.rule, &[fixture.range]);
        assert_findings(&negative, fixture.rule, &[]);
        assert!(positive.diagnostics[0].location.range[1] <= fixture.positive.len());
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
        &run("require(path)", &configuration, &[dynamic], None)?,
        Rule::NonConstRequire,
        &[[8, 12]],
    );

    assert_findings(
        &run("require('./clear')", &configuration, &[constant], None)?,
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
        &run("require('./blocked')", &configuration, &[blocked], None)?,
        Rule::RestrictedModulePaths,
        &[[8, 19]],
    );

    assert_findings(
        &run("require('./clear')", &configuration, &[clear], None)?,
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

        assert_eq!(
            diagnostic.kind,
            Kind::Analysis(instar_analysis::Kind::Unsupported)
        );

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
        "local function f(value) do local value=1 print(value) end end\nfor skip,kept in iterator do print(1) end\nreturn f",
        &configuration,
        &[],
        None,
    )?;

    let source = "local function f(value) do local value=1 print(value) end end\nfor skip,kept in iterator do print(1) end\nreturn f";
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
        &run("local t={}\nt.field=1\nreturn t", &configuration, &[], None)?,
        Rule::PreferConst,
        &[],
    );

    assert_findings(
        &run(
            "local t={}\nlocal function f(t) t.field=1 end\nreturn t,f",
            &configuration,
            &[],
            None,
        )?,
        Rule::PreferConst,
        &[[6, 7]],
    );

    assert_findings(
        &run(
            "local function f(a,b) return a,b end\nlocal function g(f) return f(1) end\nreturn g",
            &configured(Rule::MismatchedArgumentCount),
            &[],
            None,
        )?,
        Rule::MismatchedArgumentCount,
        &[],
    );

    assert_findings(
        &run(
            "local pcall=function() end\npcall()",
            &configured(Rule::IgnoredPcallResult),
            &[],
            None,
        )?,
        Rule::IgnoredPcallResult,
        &[],
    );

    assert_findings(
        &run(
            "local Color3={new=function() end}\nColor3.new(255)",
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
    let result = run("local =", &Configuration::default(), &[], None)?;
    let tree = vermis::parse(b"local =");
    assert_eq!(result.completion, Completion::Complete);
    assert_eq!(result.modules, ["fixture"]);
    assert_ne!(result.diagnostics, Vec::new());
    assert_eq!(result.diagnostics.len(), tree.diagnostics.len());

    for (diagnostic, expected) in result.diagnostics.iter().zip(&tree.diagnostics) {
        assert_eq!(
            diagnostic.kind,
            Kind::Analysis(instar_analysis::Kind::Syntax { code: None })
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
        "return game:GetService([=[Players]=]):GetChildren()",
        r#"return game:GetService("Pla\121ers"):GetChildren()"#,
    ] {
        assert_findings(
            &run(text, &configuration, &[], None)?,
            Rule::RobloxPreferGetPlayers,
            &[[7, text.len()]],
        );
    }

    assert_findings(
        &run(
            r#"return game:GetService("Other"):GetChildren()"#,
            &configuration,
            &[],
            None,
        )?,
        Rule::RobloxPreferGetPlayers,
        &[],
    );

    let configuration = configured(Rule::BadStringEscape);

    assert_findings(
        &run(r#"return "\255""#, &configuration, &[], None)?,
        Rule::BadStringEscape,
        &[],
    );

    Ok(())
}
