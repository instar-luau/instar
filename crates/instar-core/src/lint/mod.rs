use regex::Regex;
use vermis::{Span, Token, View};

use crate::{
    config::{LintConfig, LintLevel},
    graph::Writes,
};

mod complexity;
mod correctness;
mod performance;
mod roblox;
mod style;
mod suspicious;

pub(crate) struct Finding {
    pub(crate) rule: &'static str,
    pub(crate) span: Span,
    pub(crate) level: LintLevel,
    pub(crate) message: String,
}

pub(super) struct Context<'a> {
    pub(super) source: &'a str,
    pub(super) tokens: &'a [Token],
    pub(super) config: &'a LintConfig,
    pub(super) globals: &'a [String],
    ignore_pattern: Option<Regex>,
    writes: Writes,
}

impl Context<'_> {
    pub(super) fn enabled(&self, rule: &str) -> bool {
        self.config.level(rule) != LintLevel::Allow
    }

    pub(super) fn ignored_name(&self, name: &[u8]) -> bool {
        self.ignore_pattern.as_ref().map_or_else(
            || name.first() == Some(&b'_'),
            |pattern| std::str::from_utf8(name).is_ok_and(|name| pattern.is_match(name)),
        )
    }

    pub(super) fn emit(
        &self,
        findings: &mut Vec<Finding>,
        rule: &'static str,
        span: Span,
        message: impl Into<String>,
    ) {
        let level = self.config.level(rule);

        if level != LintLevel::Allow {
            findings.push(Finding {
                rule,
                span,
                level,
                message: message.into(),
            });
        }
    }
}

fn walk<'tree, 'source>(
    node: View<'tree, 'source>,
    ancestors: &mut Vec<View<'tree, 'source>>,
    context: &Context<'_>,
    roblox_enabled: bool,
    findings: &mut Vec<Finding>,
) {
    correctness::check(node, ancestors, context, findings);
    suspicious::check(node, ancestors, context, findings);
    style::check(node, ancestors, context, findings);
    complexity::check(node, ancestors, context, findings);
    performance::check(node, ancestors, context, findings);

    if roblox_enabled {
        roblox::check(node, ancestors, context, findings);
    }

    ancestors.push(node);

    for child in node.children() {
        walk(child, ancestors, context, roblox_enabled, findings);
    }

    ancestors.pop();
}

pub(crate) fn check(
    source: &str,
    config: &LintConfig,
    globals: &[String],
    roblox_enabled: bool,
) -> Vec<Finding> {
    let tree = vermis::parse(source.as_bytes());

    if !tree.diagnostics.is_empty() {
        return Vec::new();
    }

    let Some(root) = tree.view(tree.root) else {
        return Vec::new();
    };

    let options = &config.options.unused_variable;

    let ignore_pattern =
        if (options.parameters() || options.loop_variables()) && options.ignore_pattern() != "^_" {
            Some(Regex::new(options.ignore_pattern()).expect("lint configuration was validated"))
        } else {
            None
        };

    let context = Context {
        source,
        tokens: &tree.tokens,
        config,
        globals,
        ignore_pattern,
        writes: Writes::analyze(root),
    };

    let mut findings = Vec::new();

    walk(
        root,
        &mut Vec::new(),
        &context,
        roblox_enabled,
        &mut findings,
    );

    findings
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_correctness() {
        let cases = [
            (
                "almost_swapped",
                "local a,b=1,2\na=b\nb=a",
                "local a,b=1,2\na,b=b,a",
            ),
            ("bad_string_escape", r#"print("\q")"#, r#"print("\n")"#),
            ("compare_nan", "print(0/0 == 1)", "print(0/1 == 1)"),
            (
                "constant_condition",
                "if true then print(1) end",
                "if flag then print(1) end",
            ),
            (
                "constant_table_comparison",
                "print({} == value)",
                "print(value == other)",
            ),
            (
                "length_as_condition",
                "if #list then print(1) end",
                "if #list > 0 then print(1) end",
            ),
            (
                "mismatched_arg_count",
                "local function f(a,b) return a+b end\nf(1)",
                "local function f(a,b) return a+b end\nf(1,2)",
            ),
            ("must_use", "math.abs(-1)", "print(math.abs(-1))"),
            (
                "type_check_inside_call",
                "print(type(1 == 2))",
                "print(type(1) == \"number\")",
            ),
            (
                "zero_step_loop",
                "for i=1,10,0 do print(i) end",
                "for i=1,10,1 do print(i) end",
            ),
            (
                "unused_variable",
                "local function f(value) return 1 end\nf(2)",
                "local function f(value) return value end\nf(2)",
            ),
        ];

        assert_cases(&cases);
    }

    #[test]
    fn catalog_suspicious() {
        let cases = [
            ("divide_by_zero", "print(1/0.00)", "print(1/1)"),
            ("empty_if", "if flag then end", "if flag then print(1) end"),
            ("empty_loop", "while flag do end", "while flag do break end"),
            (
                "global_usage",
                "print(_G.value)",
                "local _G = {}\nprint(_G.value)",
            ),
            (
                "if_same_then_else",
                "if flag then print(1) else print(1) end",
                "if flag then print(1) else print(2) end",
            ),
            (
                "ignored_pcall_result",
                "pcall(print,1)",
                "local ok=pcall(print,1)\nprint(ok)",
            ),
            (
                "implicit_any_local",
                "local value\nvalue=1",
                "local value=1\nvalue=2",
            ),
            (
                "implicit_any_parameter",
                "local function f(value) return value end",
                "local function f(value:number) return value end",
            ),
            (
                "mixed_table",
                "local t={1, key=2}",
                "local t={key=1, other=2}",
            ),
            (
                "self_assignment",
                "local value=1\nvalue=value",
                "local value=1\nvalue=2",
            ),
            (
                "unscoped_variables",
                "created=1",
                "local created=1\ncreated=2",
            ),
        ];

        assert_cases(&cases);
    }

    #[test]
    fn catalog_style() {
        let cases = [
            (
                "and_or_conditional",
                "local v=flag and 1 or 2",
                "local v=flag and false or 2",
            ),
            (
                "collapsible_if",
                "if a then if b then print(1) end end",
                "if a then print(1) end",
            ),
            ("deprecated", "old.api()", "new.api()"),
            (
                "else_after_return",
                "local function f(x) if x then return 1 else return 2 end end",
                "local function f(x) if x then print(1) else return 2 end end",
            ),
            (
                "if_expression_assignment",
                "local v=if flag then 1 else 2",
                "print(if flag then 1 else 2)",
            ),
            (
                "negated_condition",
                "if not flag then print(1) else print(2) end",
                "if flag then print(1) else print(2) end",
            ),
            (
                "non_const_require",
                "local module=require(\"./dep\")",
                "local module=require(\"./dep\")\nmodule={}",
            ),
            (
                "parenthese_conditions",
                "if (flag) then print(1) end",
                "if flag then print(1) end",
            ),
            (
                "prefer_const",
                "local value=1\nprint(value)",
                "local value=1\nvalue=2\nprint(value)",
            ),
            (
                "restricted_globals",
                "print(1)",
                "local print=function() end\nprint(1)",
            ),
            (
                "restricted_module_paths",
                "require(\"./dep\")",
                "local require=function() end\nrequire(\"./dep\")",
            ),
        ];

        assert_cases(&cases);
    }

    #[test]
    fn catalog_complexity() {
        let cases = [(
            "high_cyclomatic_complexity",
            "local function f(x) if x then return 1 end while x do break end end",
            "local function f(x) if x then return 1 end end",
        )];

        assert_cases(&cases);
    }

    #[test]
    fn catalog_performance() {
        let cases = [
            (
                "loop_invariant_call",
                "while running do require(\"./dep\") end",
                "local dep=require(\"./dep\")\nwhile running do print(dep) end",
            ),
            (
                "manual_table_clone",
                "local dest={}\nfor k,v in pairs(src) do dest[k]=v end",
                "local dest={}\nfor k,v in pairs(src) do dest[k]=v print(k) end",
            ),
            (
                "string_concat_in_loop",
                "local s=\"\"\nwhile running do s=s..\"x\" end",
                "local s=\"\"\nwhile running do local t=s..\"x\" print(t) end",
            ),
        ];

        assert_cases(&cases);
    }

    #[test]
    fn catalog_roblox() {
        let cases = [
            (
                "roblox_incorrect_color3_new_bounds",
                "Color3.new(255,0,0)",
                "Color3.fromRGB(255,0,0)",
            ),
            (
                "roblox_manual_fromscale_or_fromoffset",
                "UDim2.new(0.5,0,0.2,0)",
                "UDim2.new(0.5,10,0.2,5)",
            ),
            (
                "roblox_prefer_get_players",
                "local Players=game:GetService(\"Players\")\nPlayers:GetChildren()",
                "local Folder=game:GetService(\"ReplicatedStorage\")\nFolder:GetChildren()",
            ),
            (
                "roblox_suspicious_udim2_new",
                "UDim2.new(1,2)",
                "UDim2.new(1,2,3,4)",
            ),
        ];

        assert_cases(&cases);
    }

    #[test]
    fn const_suggestions_follow_binding_writes() {
        let mut config = LintConfig::default();
        config.rules.insert("prefer_const".into(), LintLevel::Warn);

        let cases: &[(&str, &[usize])] = &[
            ("local x,y=1,2\nx,y=3,4", &[]),
            ("local x,y=1,2\nprint(x,y)", &[6, 8]),
            ("local value=1\ndo local value=2 value=3 end", &[6]),
            (
                "local value=1\nlocal function change(value) value=2 end",
                &[6],
            ),
            ("local value=1\nlocal function change() value+=1 end", &[]),
            ("local value=1\nfor value=1,2 do value+=1 end", &[6]),
            ("local value=1\nif flag then value=2 end", &[]),
            (
                "repeat local value=1 until (function() value+=1 return true end)()",
                &[],
            ),
            ("local value=function() end\nfunction value() end", &[]),
        ];

        for &(source, expected) in cases {
            let starts: Vec<_> = check(source, &config, &[], false)
                .into_iter()
                .filter(|finding| finding.rule == "prefer_const")
                .map(|finding| finding.span.start)
                .collect();
            assert_eq!(starts, expected, "{source}");
        }

        let source = "local value=1\ndo local value=(function() value=2 return 3 end)() end";
        let starts: Vec<_> = check(source, &config, &[], false)
            .into_iter()
            .filter(|finding| finding.rule == "prefer_const")
            .map(|finding| finding.span.start)
            .collect();
        assert_eq!(starts, [source.rfind("local value").unwrap() + 6]);

        assert_cases(&[(
            "non_const_require",
            "local dep=require('./dep')\ndo local dep={} dep={} end",
            "local dep,other=require('./dep'),1\ndep,other={},2",
        )]);
    }

    #[test]
    fn mutated_table_option_follows_binding_ownership() {
        let mut config = LintConfig::default();
        config.rules.insert("prefer_const".into(), LintLevel::Warn);
        config.options.prefer_const.mutated_tables_stay_local = Some(true);

        let cases: &[(&str, &[usize])] = &[
            ("local t={}\nt.field=1", &[]),
            ("local t={}\nprint(t[key])\nt.other=1", &[]),
            ("local t={}\nt[key]+=1", &[]),
            ("local t={}\nt.x,t.y=1,2", &[]),
            ("local t={}\nlocal function change(t) t.field=1 end", &[6]),
            ("local t={}\ndo local t={} t.field=1 end", &[6]),
            ("local t={}\nother.t=1", &[6]),
            ("local t={}\nfunction t.method() end", &[]),
        ];

        for &(source, expected) in cases {
            let starts: Vec<_> = check(source, &config, &[], false)
                .into_iter()
                .filter(|finding| finding.rule == "prefer_const")
                .map(|finding| finding.span.start)
                .collect();
            assert_eq!(starts, expected, "{source}");
        }

        config.options.prefer_const.mutated_tables_stay_local = Some(false);
        assert!(
            check("local t={}\nt.field=1", &config, &[], false)
                .iter()
                .any(|finding| finding.rule == "prefer_const" && finding.span.start == 6)
        );
    }

    #[test]
    fn function_arity_follows_visible_binding() {
        let config = LintConfig::default();
        let cases = [
            (
                "local function f(a,b) end\nlocal function g(f) f(1) end",
                None,
            ),
            (
                "local function f(a,b) end\nlocal function g() f(1) end",
                Some("f(1)"),
            ),
            ("local f=function(a,b) end\nf(1)", Some("f(1)")),
            ("local function f(a,b) end\ndo local f\nf(1) end", None),
            (
                "local function f(a,b) end\ndo local f=f(1) end",
                Some("f(1)"),
            ),
            ("local function f(a,b) end\nfunction f() end\nf(1)", None),
            (
                "local function f(a,b) end\nf(1)\nf=callback\nf()",
                Some("f(1)"),
            ),
            ("local function f(a,b) end\ndo f=callback end\nf()", None),
            (
                "local function f(a,b) end\ndo local f=callback f=other end\nf(1)",
                Some("f(1)"),
            ),
            (
                "local function f(a,b) end\nfor f in iterator do f(1) end",
                None,
            ),
            (
                "local function self(a,b) end\nfunction object:run() self(1) end",
                None,
            ),
            ("local function f(a,b) f(1) end", Some("f(1)")),
            ("local function f(a,...) end\nf(1,2,3)", None),
            ("local function f(a,b) end\nf(1,2)", None),
        ];

        for (source, bad_call) in cases {
            let starts: Vec<_> = check(source, &config, &[], false)
                .into_iter()
                .filter(|finding| finding.rule == "mismatched_arg_count")
                .map(|finding| finding.span.start)
                .collect();
            let expected: Vec<_> = bad_call
                .map(|call| source.find(call).unwrap())
                .into_iter()
                .collect();
            assert_eq!(starts, expected, "{source}");
        }
    }

    fn assert_cases(cases: &[(&str, &str, &str)]) {
        for &(rule, positive, negative) in cases {
            let mut config = LintConfig::default();
            config.rules.insert(rule.to_owned(), LintLevel::Warn);

            match rule {
                "unused_variable" => config.options.unused_variable.parameters = Some(true),

                "high_cyclomatic_complexity" => {
                    config.options.high_cyclomatic_complexity.maximum_complexity = Some(2);
                }

                "deprecated" => {
                    config
                        .options
                        .deprecated
                        .additional
                        .insert("old.api".into(), "new.api".into());
                }

                "restricted_globals" => {
                    config
                        .options
                        .restricted_globals
                        .insert("print".into(), "use logging".into());
                }

                "restricted_module_paths" => {
                    config
                        .options
                        .restricted_module_paths
                        .paths
                        .insert("./dep".into(), "blocked".into());
                }

                _ => {}
            }

            let roblox = rule.starts_with("roblox_");

            assert!(
                check(positive, &config, &[], roblox)
                    .iter()
                    .any(|finding| finding.rule == rule),
                "expected {rule} for {positive:?}"
            );

            assert!(
                !check(negative, &config, &[], roblox)
                    .iter()
                    .any(|finding| finding.rule == rule),
                "unexpected {rule} for {negative:?}"
            );
        }
    }

    #[test]
    fn explicit_defaults_override_inherited_option_values() {
        let mut parent: LintConfig = toml::from_str(
            "[options.unused_variable]\nparameters = true\nignore_pattern = \"^skip\"\n[options.high_cyclomatic_complexity]\nmaximum_complexity = 7"
        ).unwrap();

        let child: LintConfig = toml::from_str(
            "[options.unused_variable]\nparameters = false\nignore_pattern = \"^_\"\n[options.high_cyclomatic_complexity]\nmaximum_complexity = 40"
        ).unwrap();

        parent.merge(&child);
        assert!(!parent.options.unused_variable.parameters());
        assert_eq!(parent.options.unused_variable.ignore_pattern(), "^_");

        assert_eq!(
            parent
                .options
                .high_cyclomatic_complexity
                .maximum_complexity(),
            40
        );
    }

    #[test]
    fn rule_levels_override_groups_without_enabling_opt_in_rules() {
        let source = "while running do require(\"./dep\") end";
        let mut config = LintConfig::default();
        config.groups.insert("performance".into(), LintLevel::Info);

        assert_eq!(
            check(source, &config, &[], false)
                .into_iter()
                .find(|finding| finding.rule == "loop_invariant_call")
                .map(|finding| finding.level),
            Some(LintLevel::Info),
        );

        config
            .rules
            .insert("loop_invariant_call".into(), LintLevel::Deny);

        assert_eq!(
            check(source, &config, &[], false)
                .into_iter()
                .find(|finding| finding.rule == "loop_invariant_call")
                .map(|finding| finding.level),
            Some(LintLevel::Deny),
        );

        config
            .rules
            .insert("loop_invariant_call".into(), LintLevel::Allow);

        assert!(
            !check(source, &config, &[], false)
                .iter()
                .any(|finding| finding.rule == "loop_invariant_call")
        );

        config.groups.insert("style".into(), LintLevel::Warn);

        assert!(
            !check("local x=flag and 1 or 2", &config, &[], false)
                .iter()
                .any(|finding| finding.rule == "and_or_conditional")
        );
    }

    #[test]
    fn ignored_loop_names_obey_configured_regex() {
        let mut config = LintConfig::default();
        config.options.unused_variable.loop_variables = Some(true);
        config.options.unused_variable.ignore_pattern = Some("^skip".into());

        let findings = check(
            "for skipItem, kept in pairs(items) do print(1) end",
            &config,
            &[],
            false,
        );

        assert_eq!(
            findings
                .iter()
                .filter(|finding| finding.rule == "unused_variable")
                .count(),
            1
        );

        assert_eq!(
            findings
                .iter()
                .find(|finding| finding.rule == "unused_variable")
                .unwrap()
                .span
                .start,
            14
        );
    }

    #[test]
    fn roblox_checks_require_roblox_support() {
        let config = LintConfig::default();

        assert!(
            !check("Color3.new(255, 0, 0)", &config, &[], false)
                .iter()
                .any(|finding| finding.rule == "roblox_incorrect_color3_new_bounds")
        );

        assert!(
            check("Color3.new(255, 0, 0)", &config, &[], true)
                .iter()
                .any(|finding| finding.rule == "roblox_incorrect_color3_new_bounds")
        );
    }
}
