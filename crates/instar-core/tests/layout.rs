use instar_core::configuration::format::Options;
use instar_core::configuration::format::{
    Blocks, CallStyle, Calls, Chains, Collapse, Conditional, ConditionalExpansion,
    ConditionalStyle, Constants, Endings, Expansion, Functions, Imports, Indentation, Indexer,
    Operators, Order, Parameters, Parentheses, Placement, Quotes, Separation, Separator, Sorting,
    Spacing, Tables, TypeExpansion, Types, Unused, Whitespace,
};
fn format(source: &str, options: &Options) -> std::io::Result<String> {
    instar_core::format::format(source.as_bytes(), options)
        .map(|output| String::from_utf8(output).unwrap())
}

fn formatted(source: &str) -> String {
    format(source, &Options::default()).expect("formats")
}

#[expect(
    clippy::needless_pass_by_value,
    reason = "Layout cases consume their configuration"
)]
fn configured(source: &str, configuration: Options) -> String {
    format(source, &configuration).expect("formats")
}

fn narrow(width: usize) -> Options {
    Options {
        column_width: width,
        ..Default::default()
    }
}

const SAMPLES: &[&str] = &[
    "local a = 1\n",
    "local a, b: number = 1, 2\n",
    "local t = { a = 1, [2] = 'x', 3 }\n",
    "local f = function(a, b) return a + b end\n",
    "function M.thing:method(a: number): string\n\treturn tostring(a)\nend\n",
    "if a then\n\tb()\nelseif c then\n\td()\nelse\n\te()\nend\n",
    "for i = 1, 10, 2 do\n\tprint(i)\nend\n",
    "for k, v in pairs(t) do\n\tprint(k, v)\nend\n",
    "while true do\n\tbreak\nend\n",
    "repeat\n\tx()\nuntil done\n",
    "do\n\tlocal scoped = 1\nend\n",
    "export type Thing = { a: number, b: string? }\n",
    "type Wide = {\n\tname: string,\n\thealth: number,\n\tposition: Vector3,\n\tinventory: { [string]: number },\n}\n",
    "local x = a and b or c\n",
    "local x = -y + #z\n",
    "local x = (a + b) * c\n",
    "local s = `interp {value} here`\n",
    "local s = [[\nlong\n]]\n",
    "-- leading\nlocal a = 1 -- trailing\n\n-- after a gap\nlocal b = 2\n",
    "--[[\n\ta long comment\n]]\nlocal a = 1\n",
    "local x = obj:method(1):chain(2).field\n",
    "return\n",
    "local x = value :: SomeType\n",
    "local x = if cond then a else b\n",
    "continue\n",
    "local f = require('@pkg/thing')\n",
    "do -- a note on the keyword\n\tx()\nend\n",
];

#[test]
fn formatting_is_idempotent() {
    for source in SAMPLES {
        let once = formatted(source);
        let twice = formatted(&once);

        assert_eq!(once, twice, "unstable for {source:?}");
    }
}

#[test]
fn output_always_parses() {
    for source in SAMPLES {
        let output = formatted(source);

        assert_eq!(
            vermis::parse(output.as_bytes().into()).diagnostics,
            [] as [vermis::Diagnostic; 0]
        );
    }
}

#[test]
fn no_output_line_ends_in_whitespace() {
    for source in SAMPLES {
        for line in formatted(source).lines() {
            assert_eq!(line, line.trim_end(), "trailing whitespace from {source:?}");
        }
    }
}

#[test]
fn every_comment_survives() {
    for source in SAMPLES {
        let output = formatted(source);
        let before = vermis::parse(source.as_bytes().into());

        for token in before.tokens.iter().filter(|token| {
            matches!(
                token.kind,
                vermis::TokenKind::Comment | vermis::TokenKind::BlockComment
            )
        }) {
            let (start, end) = (&token.span.start, &token.span.end);
            let text = source[*start..*end].trim_end();

            assert!(
                output.contains(text),
                "{source:?} lost the comment {text:?}"
            );
        }
    }
}

#[test]
fn formatted_input_is_left_alone() {
    let already = "local Players = game:GetService(\"Players\")\n\nlocal function greet(name: string): string\n\treturn \"hi \" .. name\nend\n\nreturn greet\n";

    assert_eq!(formatted(already), already);
}

#[test]
fn an_empty_file_stays_empty() {
    assert_eq!(formatted(""), "\n");
    assert_eq!(formatted("\n\n\n"), "\n");
}

#[test]
fn a_file_that_does_not_parse_is_refused_rather_than_mangled() {
    assert!(format("local = = =", &Options::default()).is_err());
    assert!(format("local x = [[unterminated", &Options::default()).is_err());
}

#[test]
fn indentation_follows_nesting() {
    let output = formatted("if a then if b then c() end end");

    assert_eq!(output, "if a then\n\tif b then\n\t\tc()\n\tend\nend\n");
}

#[test]
fn a_call_that_does_not_fit_breaks_one_argument_per_line() {
    assert_eq!(
        configured("f(alpha, beta, gamma)", narrow(16)),
        "f(\n\talpha,\n\tbeta,\n\tgamma\n)\n"
    );
}

#[test]
fn an_inner_call_stays_on_one_line_when_the_outer_one_breaks() {
    let output = configured("outer(inner(a), someVeryLongArgumentName)", narrow(30));

    assert!(
        output.contains("inner(a)"),
        "inner should not break, got {output}"
    );

    assert!(
        output.contains("outer(\n"),
        "outer should break, got {output}"
    );
}

#[test]
fn a_long_binary_chain_breaks_with_the_operator_leading() {
    let output = configured("local ok = first and second and third", narrow(24));

    assert_eq!(output, "local ok = first\n\tand second\n\tand third\n");
}

#[test]
fn a_chain_breaks_at_the_loosest_operator_first() {
    let output = configured("local x = aaaa and bbbb or cccc and dddd", narrow(24));

    assert!(
        output.contains("\n\tor "),
        "should break at or, got {output}"
    );

    assert!(
        output.contains("aaaa and bbbb"),
        "and should stay flat, got {output}"
    );
}

#[test]
fn a_callback_hugs_the_parentheses_instead_of_indenting_twice() {
    let output = formatted("thing:Connect(function(a)\n\tprint(a)\nend)");

    assert_eq!(output, "thing:Connect(function(a)\n\tprint(a)\nend)\n");
}

#[test]
fn a_table_assigned_to_a_name_hangs_off_the_equals() {
    let output = formatted("local t = {\n\ta = 1,\n}");

    assert_eq!(output, "local t = {\n\ta = 1,\n}\n");
}

#[test]
fn one_blank_line_is_kept_and_several_collapse_to_one() {
    assert_eq!(
        formatted("local a = 1\n\nlocal b = 2\n"),
        "local a = 1\n\nlocal b = 2\n"
    );

    assert_eq!(
        formatted("local a = 1\n\n\n\nlocal b = 2\n"),
        "local a = 1\n\nlocal b = 2\n"
    );
}

#[test]
fn a_newline_after_the_brace_keeps_a_table_expanded() {
    let output = formatted("local t = {\n\ta = 1\n}");

    assert_eq!(output, "local t = {\n\ta = 1,\n}\n");
}

#[test]
fn a_table_written_on_one_line_stays_on_one_line() {
    assert_eq!(formatted("local t = { a = 1 }"), "local t = { a = 1 }\n");
}

#[test]
fn a_trailing_comma_keeps_a_one_line_table_expanded() {
    assert_eq!(
        formatted("local t = { a, b, }"),
        "local t = {\n\ta,\n\tb,\n}\n"
    );
}

#[test]
fn without_the_trailing_comma_the_same_table_stays_flat() {
    assert_eq!(formatted("local t = { a, b }"), "local t = { a, b }\n");
}

#[test]
fn source_comma_expansion_is_configurable() {
    let configuration = Options {
        expand_on_trailing_comma: false,
        ..Default::default()
    };

    assert_eq!(
        configured("local t = { a, b, }", configuration),
        "local t = { a, b }\n"
    );
}

#[test]
fn a_call_is_laid_out_by_width_alone() {
    assert_eq!(formatted("f(\n\ta,\n\tb\n)"), "f(a, b)\n");
    assert!(format("f(a, b,)", &Options::default()).is_err());
}

#[test]
fn a_stray_semicolon_is_dropped() {
    assert_eq!(
        formatted("local a = 1;\n;\nlocal b = 2\n"),
        "local a = 1\nlocal b = 2\n"
    );
}

#[test]
fn a_trailing_comment_stays_on_its_line() {
    assert_eq!(formatted("local a = 1   -- why\n"), "local a = 1 -- why\n");
}

#[test]
fn a_leading_comment_keeps_its_own_line_and_its_gap() {
    let output = formatted("local a = 1\n\n-- section\nlocal b = 2\n");

    assert_eq!(output, "local a = 1\n\n-- section\nlocal b = 2\n");
}

#[test]
fn a_gap_below_a_comment_is_kept() {
    assert_eq!(
        formatted("local a = 1\n-- section\n\nlocal b = 2\n"),
        "local a = 1\n-- section\n\nlocal b = 2\n"
    );
}

#[test]
fn no_gap_below_a_comment_stays_no_gap() {
    assert_eq!(
        formatted("local a = 1\n-- attached\nlocal b = 2\n"),
        "local a = 1\n-- attached\nlocal b = 2\n"
    );
}

#[test]
fn a_gap_between_two_comments_is_kept() {
    assert_eq!(
        formatted("-- one\n\n-- two\nlocal a = 1\n"),
        "-- one\n\n-- two\nlocal a = 1\n"
    );
}

#[test]
fn a_gap_below_a_directive_is_kept() {
    assert_eq!(
        formatted("--!strict\n\nlocal a = 1\n"),
        "--!strict\n\nlocal a = 1\n"
    );
}

#[test]
fn a_gap_below_a_comment_is_kept_inside_a_block() {
    assert_eq!(
        formatted("if x then\n\t-- why\n\n\tfoo()\nend\n"),
        "if x then\n\t-- why\n\n\tfoo()\nend\n"
    );
}

#[test]
fn a_gap_between_two_comments_that_close_a_block_is_kept() {
    assert_eq!(
        formatted("do\n\tx()\n\t-- one\n\n\t-- two\nend\n"),
        "do\n\tx()\n\t-- one\n\n\t-- two\nend\n"
    );
}

#[test]
fn several_gaps_below_a_comment_become_one() {
    assert_eq!(
        formatted("local a = 1\n-- note\n\n\n\nlocal b = 2\n"),
        "local a = 1\n-- note\n\nlocal b = 2\n"
    );
}

#[test]
fn a_comment_on_the_opening_keyword_is_not_lost() {
    assert_eq!(
        formatted("do -- note\n\tx()\nend"),
        "do -- note\n\tx()\nend\n"
    );
}

#[test]
fn a_comment_at_the_end_of_a_block_is_kept() {
    assert_eq!(
        formatted("do\n\tx()\n\t-- last\nend"),
        "do\n\tx()\n\t-- last\nend\n"
    );
}

#[test]
fn a_comment_at_the_end_of_the_file_is_kept() {
    assert_eq!(
        formatted("local a = 1\n-- the end\n"),
        "local a = 1\n-- the end\n"
    );
}

#[test]
fn a_long_comment_keeps_its_interior_exactly() {
    let source = "do\n\t--[[\nnot indented\n\t]]\n\tx()\nend";

    assert!(formatted(source).contains("\nnot indented\n"));
}

#[test]
fn a_shebang_style_directive_survives() {
    assert!(formatted("--!strict\nlocal a = 1\n").starts_with("--!strict\n"));
}

#[test]
fn quotes_normalise_to_double_by_default() {
    assert_eq!(formatted("local s = 'hi'"), "local s = \"hi\"\n");
}

#[test]
fn requoting_fixes_the_escapes() {
    assert_eq!(formatted(r"local s = 'it\'s'"), "local s = \"it's\"\n");

    assert_eq!(
        configured(
            r#"local s = "say \"hi\"""#,
            Options {
                quotes: Quotes::Single,
                ..Default::default()
            }
        ),
        "local s = 'say \"hi\"'\n"
    );
}

#[test]
fn the_quote_needing_fewer_escapes_wins() {
    assert_eq!(
        formatted(r#"local s = 'say "hi"'"#),
        "local s = 'say \"hi\"'\n"
    );
}

#[test]
fn preserve_leaves_every_literal_alone() {
    let configuration = Options {
        quotes: Quotes::Preserve,
        ..Default::default()
    };

    assert_eq!(
        configured("local s = 'hi'", configuration),
        "local s = 'hi'\n"
    );
}

#[test]
fn a_long_string_is_never_requoted() {
    assert_eq!(
        formatted("local s = [[it's \"both\"]]"),
        "local s = [[it's \"both\"]]\n"
    );
}

#[test]
fn escape_sequences_are_left_intact() {
    assert_eq!(
        formatted(r#"local s = "a\tb\nc\\d""#),
        "local s = \"a\\tb\\nc\\\\d\"\n"
    );
}

#[test]
fn spaces_can_replace_tabs() {
    let configuration = Options {
        indentation: Indentation {
            style: Whitespace::Spaces,
            width: 2,
        },
        ..Default::default()
    };

    assert_eq!(
        configured("do\nx()\nend", configuration),
        "do\n  x()\nend\n"
    );
}

#[test]
fn windows_line_endings_apply_everywhere() {
    let configuration = Options {
        line_endings: Endings::Windows,
        ..Default::default()
    };

    assert_eq!(
        configured("do\nx()\nend", configuration),
        "do\r\n\tx()\r\nend\r\n"
    );
}

#[test]
fn space_after_function_names_targets_definitions_and_calls_separately() {
    let defs = Options {
        spacing: Spacing {
            function_names: Separation::Definitions,
            ..Default::default()
        },
        ..Default::default()
    };

    assert_eq!(
        configured("local function f(a) return a end", defs),
        "local function f (a)\n\treturn a\nend\n"
    );

    let calls = Options {
        spacing: Spacing {
            function_names: Separation::Calls,
            ..Default::default()
        },
        ..Default::default()
    };

    assert_eq!(configured("print(1)", calls), "print (1)\n");
}

#[test]
fn inner_spacing_is_configurable() {
    let configuration = Options {
        spacing: Spacing {
            parentheses: true,
            brackets: true,
            braces: false,
            ..Default::default()
        },
        ..Default::default()
    };

    assert_eq!(configured("f(a)", configuration.clone()), "f( a )\n");

    assert_eq!(
        configured("local x = t[k]", configuration.clone()),
        "local x = t[ k ]\n"
    );

    assert_eq!(
        configured("local t = { a }", configuration),
        "local t = {a}\n"
    );
}

#[test]
fn call_parentheses_can_be_dropped_for_a_single_string_or_table() {
    let no_string = Options {
        calls: Calls {
            parentheses: Parentheses::OmitString,
            ..Default::default()
        },
        ..Default::default()
    };

    assert_eq!(configured(r#"require("x")"#, no_string), "require \"x\"\n");

    let no_table = Options {
        calls: Calls {
            parentheses: Parentheses::OmitTable,
            ..Default::default()
        },
        ..Default::default()
    };

    assert_eq!(configured("f({ a = 1 })", no_table), "f { a = 1 }\n");
}

#[test]
fn call_parentheses_are_added_by_default() {
    assert_eq!(formatted("require 'x'"), "require(\"x\")\n");
    assert_eq!(formatted("f { a = 1 }"), "f({ a = 1 })\n");
}

#[test]
fn collapses_a_single_statement_body() {
    let configuration = Options {
        blocks: Blocks {
            collapse: Collapse::Always,
            ..Default::default()
        },
        ..Default::default()
    };

    assert_eq!(
        configured(
            "local function f(a)\n\treturn a\nend",
            configuration.clone()
        ),
        "local function f(a) return a end\n"
    );

    assert_eq!(
        configured("if a then\n\treturn\nend", configuration),
        "if a then return end\n"
    );
}

#[test]
fn collapsing_never_swallows_a_comment() {
    let configuration = Options {
        blocks: Blocks {
            collapse: Collapse::Always,
            ..Default::default()
        },
        ..Default::default()
    };

    let output = configured(
        "local function f(a)\n\t-- why\n\treturn a\nend",
        configuration,
    );

    assert!(output.contains("-- why"), "comment lost, got {output}");

    assert!(
        output.contains('\n'),
        "should not have collapsed, got {output}"
    );
}

#[test]
fn collapse_is_off_by_default() {
    assert_eq!(
        formatted("local function f(a) return a end"),
        "local function f(a)\n\treturn a\nend\n"
    );
}

#[test]
fn a_type_annotation_is_normalised_but_not_restructured() {
    assert_eq!(
        formatted("local x:   Array < string >  = {}"),
        "local x: Array<string> = {}\n"
    );

    assert_eq!(
        formatted("local f: (  number,string )->boolean"),
        "local f: (number, string) -> boolean\n"
    );

    assert_eq!(
        formatted("local t: {x:number,y:number}"),
        "local t: { x: number, y: number }\n"
    );

    assert!(format("local u: A|B&C", &Options::default()).is_err());
    assert_eq!(formatted("local o: string ?"), "local o: string?\n");

    assert_eq!(
        formatted("local m: {[string] : number}"),
        "local m: { [string]: number }\n"
    );
}

#[test]
fn a_type_alias_keeps_its_shape() {
    let source = "export type Handler<T> = (T) -> ()\n";

    assert_eq!(formatted(source), source);
}

#[test]
fn generics_and_return_types_come_through() {
    let source = "local function map<T, U>(t: { T }, f: (T) -> U): { U }\n\treturn t\nend\n";

    assert_eq!(formatted(source), source);
}

#[test]
fn explicit_type_instantiation_survives() {
    assert_eq!(
        formatted("local a = charm.atom<<number>>()"),
        "local a = charm.atom<<number>>()\n"
    );

    assert_eq!(
        formatted("local a = charm.atom<<(number, string)>>()"),
        "local a = charm.atom<<(number, string)>>()\n"
    );
}

#[test]
fn attributes_stay_above_their_function() {
    let source = "@native\nlocal function hot()\n\treturn 1\nend\n";

    assert_eq!(formatted(source), source);
}

use instar_core::configuration::format::Grouping;

fn sorting(grouping: Grouping) -> Options {
    Options {
        imports: Imports {
            sort: true,
            grouping,
            ..Default::default()
        },
        ..Default::default()
    }
}

#[test]
fn requires_are_left_alone_unless_asked() {
    let source = "local b = require(\"b\")\nlocal a = require(\"a\")\n";

    assert_eq!(formatted(source), source);
}

#[test]
fn a_run_of_requires_sorts() {
    let output = configured(
        "local c = require(\"c\")\nlocal a = require(\"a\")\nlocal b = require(\"b\")\n",
        sorting(Grouping::Flat),
    );

    assert_eq!(
        output,
        "local a = require(\"a\")\nlocal b = require(\"b\")\nlocal c = require(\"c\")\n"
    );
}

#[test]
fn a_statement_between_two_requires_breaks_the_run() {
    let source = "local c = require(\"c\")\nsideEffect()\nlocal a = require(\"a\")\n";

    assert_eq!(configured(source, sorting(Grouping::Flat)), source);
}

#[test]
fn a_blank_line_separates_two_runs_that_sort_independently() {
    let output = configured(
        "local d = require(\"d\")\nlocal c = require(\"c\")\n\nlocal b = require(\"b\")\nlocal a = require(\"a\")\n",
        sorting(Grouping::Flat),
    );

    assert_eq!(
        output,
        "local c = require(\"c\")\nlocal d = require(\"d\")\n\nlocal a = require(\"a\")\nlocal b = require(\"b\")\n"
    );
}

#[test]
fn a_comment_moves_with_the_require_it_describes() {
    let output = configured(
        "-- about c\nlocal c = require(\"c\")\n-- about a\nlocal a = require(\"a\")\n",
        sorting(Grouping::Flat),
    );

    assert_eq!(
        output,
        "-- about a\nlocal a = require(\"a\")\n-- about c\nlocal c = require(\"c\")\n"
    );
}

#[test]
fn a_trailing_comment_moves_with_its_require_too() {
    let output = configured(
        "local c = require(\"c\") -- see c\nlocal a = require(\"a\") -- see a\n",
        sorting(Grouping::Flat),
    );

    assert_eq!(
        output,
        "local a = require(\"a\") -- see a\nlocal c = require(\"c\") -- see c\n"
    );
}

#[test]
fn by_kind_groups_aliases_then_absolute_then_relative() {
    let output = configured(
        "local r = require(\"./sibling\")\nlocal g = require(\"game/Thing\")\nlocal p = require(\"@pkg/signal\")\n",
        sorting(Grouping::ByKind),
    );

    assert_eq!(
        output,
        "local p = require(\"@pkg/signal\")\n\nlocal g = require(\"game/Thing\")\n\nlocal r = require(\"./sibling\")\n"
    );
}

#[test]
fn a_computed_require_is_not_sorted_and_breaks_the_run() {
    let source =
        "local c = require(\"c\")\nlocal x = require(base .. name)\nlocal a = require(\"a\")\n";

    assert_eq!(configured(source, sorting(Grouping::Flat)), source);
}

#[test]
fn sorting_is_idempotent_and_keeps_every_comment() {
    let source = "-- c\nlocal c = require(\"c\") -- t\nlocal a = require(\"a\")\n\nlocal b = require(\"@x/b\")\n";
    let configuration = sorting(Grouping::ByKind);
    let once = configured(source, configuration.clone());
    let twice = configured(&once, configuration);

    assert_eq!(once, twice, "unstable");

    for text in ["-- c", "-- t"] {
        assert!(once.contains(text), "lost {text}, got {once}");
    }
}

#[test]
fn nested_unary_minus_keeps_its_space() {
    assert_eq!(formatted("local y = - -x"), "local y = - -x\n");
    assert_eq!(formatted("local y = -  -  -a"), "local y = - - -a\n");

    assert_eq!(
        formatted("local y = -x"),
        "local y = -x\n",
        "one minus still hugs"
    );
}

#[test]
fn a_dropped_semicolon_is_put_back_where_it_is_load_bearing() {
    assert_eq!(formatted("local a = b\n;(c)()\n"), "local a = b;\n(c)()\n");
    assert_eq!(formatted("local x = a;\n(f)()\n"), "local x = a;\n(f)()\n");
}

#[test]
fn a_bracket_index_of_a_long_string_keeps_its_space() {
    assert_eq!(formatted("local x = t[ [[k]] ]"), "local x = t[ [[k]] ]\n");

    assert_eq!(
        formatted("local u = { [ [[key]] ] = 1 }"),
        "local u = { [ [[key]] ] = 1 }\n"
    );

    assert_eq!(
        formatted("local x = t[ [=[k]=] ]"),
        "local x = t[ [=[k]=] ]\n"
    );
}

#[test]
fn const_function_stays_const() {
    assert_eq!(
        formatted("const function f() end"),
        "const function f()\nend\n"
    );

    assert_eq!(
        formatted("local function f() end"),
        "local function f()\nend\n"
    );
}

#[test]
fn a_word_and_a_number_in_a_type_keep_their_space() {
    assert_eq!(
        formatted("local v: typeof(x and 1) = nil"),
        "local v: typeof(x and 1) = nil\n"
    );

    assert_eq!(
        formatted("local w: typeof(2 or b) = nil"),
        "local w: typeof(2 or b) = nil\n"
    );

    assert_eq!(
        formatted("type T = typeof(1 .. 2)"),
        "type T = typeof(1 .. 2)\n"
    );
}

#[test]
fn a_type_function_body_is_emitted_as_written() {
    let source = "type function K(t)\n\tlocal a = 1\n\treturn t\nend\n";

    assert_eq!(formatted(source), source);
}

#[test]
fn sorting_requires_leaves_the_strict_directive_at_the_top() {
    let output = configured(
        "--!strict\nlocal zzz = require(\"./z\")\nlocal aaa = require(\"./a\")\nreturn nil\n",
        sorting(Grouping::Flat),
    );

    assert_eq!(
        output,
        "--!strict\nlocal aaa = require(\"./a\")\nlocal zzz = require(\"./z\")\nreturn nil\n",
        "the directive stays put and the requires still sort"
    );
}

#[test]
fn a_comment_above_a_table_field_is_kept() {
    let source = "local t = {\n\t-- describes a\n\ta = 1,\n}\n";

    assert_eq!(formatted(source), source);
}

#[test]
fn a_comment_after_a_table_field_is_kept() {
    let source = "local t = {\n\ta = 1, -- one\n\tb = 2, -- two\n}\n";

    assert_eq!(formatted(source), source);
}

#[test]
fn a_table_of_only_a_comment_keeps_it() {
    let source = "local t = {\n\t-- nothing yet\n}\n";

    assert_eq!(formatted(source), source);
    assert_eq!(formatted("local t = {}"), "local t = {}\n");
}

#[test]
fn a_comment_forces_a_table_to_expand() {
    assert_eq!(
        formatted("local t = { a = 1, -- one\n}"),
        "local t = {\n\ta = 1, -- one\n}\n"
    );
}

#[test]
fn table_comments_survive_a_second_pass() {
    let source = "local t = {\n\t-- a\n\ta = 1, -- t\n\t-- b\n\tb = 2,\n}\n";

    assert_eq!(formatted(&formatted(source)), formatted(source));
    assert_eq!(formatted(source), source);
}

#[test]
fn a_comment_inside_a_type_is_kept() {
    let source = "local function f(\n\toriginalError: (Error & { extensions: any? }) -- new syntax\n): GError?\n\treturn nil\nend\n";

    assert!(formatted(source).contains("-- new syntax"));
}

#[test]
fn a_comment_inside_a_parameter_list_is_kept() {
    let source = "local function x(...--[[comment here]])\nend\n";

    assert!(formatted(source).contains("--[[comment here]]"));
}

#[test]
fn a_comment_among_call_arguments_is_kept() {
    let source = "f(\n\t-- why\n\tx\n)\n";

    assert_eq!(formatted(source), source);
}

#[test]
fn a_comment_after_a_call_argument_is_kept() {
    let source = "g(\n\ta, -- first\n\tb -- second\n)\n";

    assert_eq!(formatted(source), source);
}

#[test]
fn call_argument_comments_survive_a_second_pass() {
    let source = "f(\n\t-- why\n\tx\n)\n";

    assert_eq!(formatted(&formatted(source)), formatted(source));
}

#[test]
fn placing_argument_comments_does_not_change_a_call_without_any() {
    assert_eq!(formatted("f(\n\ta,\n\tb\n)\n"), "f(a, b)\n");
}

#[test]
fn a_comment_is_placed_or_the_file_is_refused_never_dropped() {
    let with_comments = [
        "local a = 1 -- trailing\n",
        "-- leading\nlocal a = 1\n",
        "do -- on the keyword\n\tx()\nend\n",
        "do\n\tx()\n\t-- last\nend\n",
        "local t = {\n\t-- field\n\ta = 1,\n}\n",
        "f(\n\t-- argument\n\tx\n)\n",
        "local function g(a --[[ param ]])\nend\n",
        "local x: number -- annotated\n",
        "--[[\n\tlong\n]]\nlocal a = 1\n",
    ];

    for source in with_comments {
        let before = vermis::parse(source.as_bytes().into());

        match format(source, &Options::default()) {
            Ok(output) => {
                for token in before.tokens.iter().filter(|token| {
                    matches!(
                        token.kind,
                        vermis::TokenKind::Comment | vermis::TokenKind::BlockComment
                    )
                }) {
                    let (start, end) = (&token.span.start, &token.span.end);
                    let text = source[*start..*end].trim_end();

                    assert!(output.contains(text), "{source:?} silently lost {text:?}");
                }
            }

            Err(error) => assert!(
                format!("{error:#}").contains("would drop the comment"),
                "{source:?} failed for an unrelated reason, {error:#}"
            ),
        }
    }
}

fn gaps(mode: instar_core::configuration::format::Gaps) -> Options {
    Options {
        blocks: Blocks {
            blank_lines: mode,
            ..Default::default()
        },
        ..Default::default()
    }
}

#[test]
fn a_blank_at_the_edge_of_a_block_is_dropped_by_default() {
    let source = "local function f()\n\n\tbody()\n\nend\n";

    assert_eq!(formatted(source), "local function f()\n\tbody()\nend\n");
}

#[test]
fn preserve_keeps_the_blank_at_both_edges() {
    let source = "local function f()\n\n\tbody()\n\nend\n";

    assert_eq!(
        configured(
            source,
            gaps(instar_core::configuration::format::Gaps::Preserve)
        ),
        source
    );
}

#[test]
fn preserve_keeps_one_edge_when_only_one_has_a_blank() {
    let configuration = gaps(instar_core::configuration::format::Gaps::Preserve);

    assert_eq!(
        configured("do\n\n\tx()\nend\n", configuration.clone()),
        "do\n\n\tx()\nend\n"
    );

    assert_eq!(
        configured("do\n\tx()\n\nend\n", configuration),
        "do\n\tx()\n\nend\n"
    );
}

#[test]
fn a_blank_between_statements_is_not_an_edge_gap() {
    let source = "do\n\ta()\n\n\tb()\nend\n";

    assert_eq!(formatted(source), source, "kept even at the default");
}

#[test]
fn preserving_gaps_is_still_idempotent() {
    let configuration = gaps(instar_core::configuration::format::Gaps::Preserve);
    let source = "local function f()\n\n\tif a then\n\n\t\tb()\n\n\tend\n\nend\n";
    let once = configured(source, configuration.clone());

    assert_eq!(configured(&once, configuration), once);
}

fn binding(mode: instar_core::configuration::format::Binding) -> Options {
    Options {
        imports: Imports {
            binding: mode,
            ..Default::default()
        },
        ..Default::default()
    }
}

#[test]
fn require_binding_preserves_what_was_written_by_default() {
    let source = "local A = require(\"@pkg/a\")\nconst B = require(\"@pkg/b\")\nreturn A, B\n";

    assert_eq!(formatted(source), source);
}

#[test]
fn const_converts_a_local_require() {
    assert_eq!(
        configured(
            "local Signal = require(\"@pkg/signal\")\nreturn Signal\n",
            binding(instar_core::configuration::format::Binding::Const)
        ),
        "const Signal = require(\"@pkg/signal\")\nreturn Signal\n"
    );
}

#[test]
fn local_converts_a_const_require_back() {
    assert_eq!(
        configured(
            "const Signal = require(\"@pkg/signal\")\nreturn Signal\n",
            binding(instar_core::configuration::format::Binding::Local)
        ),
        "local Signal = require(\"@pkg/signal\")\nreturn Signal\n"
    );
}

#[test]
fn a_require_whose_name_is_reassigned_keeps_local() {
    let source = "local M = require(\"@pkg/m\")\nM = fallback\nreturn M\n";

    assert_eq!(
        configured(
            source,
            binding(instar_core::configuration::format::Binding::Const)
        ),
        source
    );
}

#[test]
fn only_a_single_unannotated_binding_converts() {
    let configuration = binding(instar_core::configuration::format::Binding::Const);

    for source in [
        "local A, B = require(\"@pkg/a\"), require(\"@pkg/b\")\nreturn A, B\n",
        "local S: Signal = require(\"@pkg/signal\")\nreturn S\n",
        "local x = compute()\nreturn x\n",
    ] {
        assert_eq!(
            configured(source, configuration.clone()),
            source,
            "{source:?}"
        );
    }
}

#[test]
fn a_nested_require_converts_too() {
    let output = configured(
        "local function f()\n\tlocal S = require(\"@pkg/s\")\n\treturn S\nend\nreturn f\n",
        binding(instar_core::configuration::format::Binding::Const),
    );

    assert!(output.contains("const S = require"), "{output}");
}

#[test]
fn converting_the_binding_is_idempotent() {
    let configuration = binding(instar_core::configuration::format::Binding::Const);

    let once = configured(
        "local S = require(\"@pkg/s\")\nreturn S\n",
        configuration.clone(),
    );

    assert_eq!(configured(&once, configuration), once);
}

fn semicolons(mode: instar_core::configuration::format::Semicolons) -> Options {
    Options {
        semicolons: mode,
        ..Default::default()
    }
}

#[test]
fn semicolons_are_absent_by_default() {
    assert_eq!(
        formatted("local a = 1\nlocal b = 2\nreturn b\n"),
        "local a = 1\nlocal b = 2\nreturn b\n"
    );
}

#[test]
fn always_terminates_every_statement() {
    assert_eq!(
        configured(
            "local a = 1\nlocal b = 2\nreturn b\n",
            semicolons(instar_core::configuration::format::Semicolons::Always)
        ),
        "local a = 1;\nlocal b = 2;\nreturn b;\n"
    );
}

#[test]
fn the_one_semicolon_luau_requires_survives_every_setting() {
    for mode in [
        instar_core::configuration::format::Semicolons::Never,
        instar_core::configuration::format::Semicolons::Always,
    ] {
        let output = configured("local a = b\n;(c)()\nreturn a\n", semicolons(mode));

        assert!(output.contains("local a = b;"), "{output}");
    }
}

#[test]
fn a_semicolon_lands_before_a_trailing_comment_not_after_it() {
    assert_eq!(
        configured(
            "local a = 1 -- note\nreturn a\n",
            semicolons(instar_core::configuration::format::Semicolons::Always)
        ),
        "local a = 1; -- note\nreturn a;\n"
    );
}

#[test]
fn terminating_every_statement_is_idempotent() {
    let configuration = semicolons(instar_core::configuration::format::Semicolons::Always);

    let once = configured(
        "local a = 1\nif a then\n\treturn a\nend\n",
        configuration.clone(),
    );

    assert_eq!(configured(&once, configuration), once);
}

#[test]
fn bare_calls_keep_parentheses_where_required() {
    let configuration = Options {
        calls: Calls {
            parentheses: instar_core::configuration::format::Parentheses::OmitOptional,
            ..Default::default()
        },
        ..Default::default()
    };

    assert_eq!(configured("f(\"s\")\n", configuration.clone()), "f \"s\"\n");

    assert_eq!(
        configured("g({ t = 1 })\n", configuration.clone()),
        "g { t = 1 }\n"
    );

    assert_eq!(
        configured("h(a)\n", configuration),
        "h(a)\n",
        "a single name keeps them"
    );
}

#[test]
fn a_file_ends_with_a_newline_by_default() {
    assert_eq!(formatted("local a = 1"), "local a = 1\n");
}

#[test]
fn the_final_newline_can_be_turned_off() {
    let configuration = Options {
        final_newline: false,
        ..Default::default()
    };

    assert_eq!(configured("local a = 1\n", configuration), "local a = 1");
}

#[test]
fn turning_it_off_does_not_bring_back_trailing_whitespace() {
    let configuration = Options {
        final_newline: false,
        ..Default::default()
    };

    let output = configured("local a = 1   \nlocal b = 2   \n", configuration);

    for line in output.lines() {
        assert_eq!(line, line.trim_end(), "trailing whitespace on {line:?}");
    }
}

#[test]
fn dropping_the_final_newline_is_idempotent() {
    let configuration = Options {
        final_newline: false,
        ..Default::default()
    };

    let once = configured("local a = 1\n", configuration.clone());

    assert_eq!(configured(&once, configuration), once);
}

#[test]
fn an_ambiguous_call_across_lines_is_refused() {
    let source = "local f = print\nprint(1)\n(f)()\n";
    let error = format(source, &Options::default()).expect_err("Luau rejects this");

    assert!(format!("{error:#}").contains("ambiguous"), "{error:#}");
}

#[test]
fn a_semicolon_resolves_the_ambiguity() {
    let source = "local f = print\nprint(1);\n(f)()\n";

    assert!(format(source, &Options::default()).is_ok());
}

#[test]
fn a_call_whose_arguments_wrap_is_not_ambiguous() {
    assert_eq!(formatted("print(\n\t1,\n\t2\n)\n"), "print(1, 2)\n");
}

#[test]
fn a_chained_call_on_the_next_line_is_not_ambiguous() {
    let output = formatted("local t = {}\nlocal x = t.f(1)\n\t.g(2)\nreturn x\n");

    assert!(output.contains("t.f(1).g(2)"), "{output}");
}

#[test]
fn a_file_held_off_in_full_is_untouched() {
    let source = "-- instar: format off\nlocal  matrix = {\n\t1,0,0,\n\t0,1,0,\n}\n";

    assert_eq!(formatted(source), source);
}

#[test]
fn a_region_between_two_markers_is_untouched() {
    let source = "local  a   =  1\n-- instar: format off\nlocal  m = {\n\t1,0,\n\t0,1,\n}\n-- instar: format on\nlocal  b   =  2\n";

    assert_eq!(
        formatted(source),
        "local a = 1\n-- instar: format off\nlocal  m = {\n\t1,0,\n\t0,1,\n}\n-- instar: format on\nlocal b = 2\n"
    );
}

#[test]
fn a_count_holds_that_many_lines_below_the_marker() {
    let source =
        "local  a  = 1\n-- instar: format off(2)\nlocal  x  = 1\nlocal  y  = 2\nlocal  c  = 3\n";

    assert_eq!(
        formatted(source),
        "local a = 1\n-- instar: format off(2)\nlocal  x  = 1\nlocal  y  = 2\nlocal c = 3\n"
    );
}

#[test]
fn formatting_markers_preserve_inline_tables() {
    let source = "local  a  = 1\n-- instar: format off\nlocal  m = {1,0}\n-- instar: format on\nlocal  b  = 2\n";

    assert_eq!(
        formatted(source),
        "local a = 1\n-- instar: format off\nlocal  m = {1,0}\n-- instar: format on\nlocal b = 2\n"
    );
}

#[test]
fn a_gap_below_an_on_marker_is_kept() {
    let source = "-- instar: format off\nlocal  m = {1,0}\n-- instar: format on\n\nlocal b = 2\n";

    assert_eq!(formatted(source), source);
}

#[test]
fn a_region_inside_a_block_keeps_its_own_shape() {
    let source = "local function f()\n\t-- instar: format off\n\tlocal  m = {\n\t\t1,0,\n\t}\n\t-- instar: format on\nend\nreturn f\n";

    assert_eq!(formatted(source), source);
}

#[test]
fn holding_the_formatter_off_is_idempotent() {
    let source = "local  a  = 1\n-- instar: format off\nlocal  m = {1,0}\n-- instar: format on\nlocal  b  = 2\n";

    let once = formatted(source);

    assert_eq!(formatted(&once), once);
}

#[test]
fn malformed_markers_are_formatted() {
    assert_eq!(
        formatted("local  a  = 1\n-- instar: format off(five)\nlocal  b  = 2\n"),
        "local a = 1\n-- instar: format off(five)\nlocal b = 2\n"
    );
}

#[test]
fn a_lint_marker_does_not_hold_the_formatter() {
    assert_eq!(
        formatted("-- instar: lint off\nlocal  a  = 1\n"),
        "-- instar: lint off\nlocal a = 1\n"
    );
}

fn conditional(expand: ConditionalExpansion) -> Options {
    Options {
        conditionals: Conditional {
            expand,
            ..Default::default()
        },
        ..Default::default()
    }
}

#[test]
fn an_if_expression_stays_on_one_line_by_default() {
    assert_eq!(
        formatted("local a = if bar then 'baz' else 'foo'"),
        "local a = if bar then \"baz\" else \"foo\"\n"
    );
}

#[test]
fn always_opens_an_if_expression_at_every_width() {
    assert_eq!(
        configured(
            "local a = if bar then 'baz' else 'foo'",
            conditional(ConditionalExpansion::Always)
        ),
        "local a = if bar then\n\t\"baz\"\nelse\n\t\"foo\"\n"
    );
}

#[test]
fn always_opens_each_arm_of_an_elseif_chain() {
    assert_eq!(
        configured(
            "local a = if x then 1 elseif y then 2 else 3",
            conditional(ConditionalExpansion::Always)
        ),
        "local a = if x then\n\t1\nelseif y then\n\t2\nelse\n\t3\n"
    );
}

#[test]
fn width_based_expansion_keeps_short_expressions_flat() {
    assert_eq!(
        configured(
            "local a = if bar then 'baz' else 'foo'",
            conditional(ConditionalExpansion::Needed)
        ),
        "local a = if bar then \"baz\" else \"foo\"\n"
    );
}

#[test]
fn width_based_expansion_opens_long_expressions() {
    let output = configured(
        "local a = if someCondition then 'a rather long branch value' else 'another long branch value'",
        conditional(ConditionalExpansion::Needed),
    );

    assert_eq!(
        output,
        "local a = if someCondition then\n\t\"a rather long branch value\"\nelse\n\t\"another long branch value\"\n"
    );
}

#[test]
fn conditional_width_controls_expansion() {
    let source = "local a = if bar then 'baz' else 'foo'";

    let wide = Options {
        conditionals: Conditional {
            expand: ConditionalExpansion::Needed,
            width: 4,
            ..Default::default()
        },
        ..Default::default()
    };

    assert!(configured(source, wide).contains("if bar then\n"));
}

#[test]
fn a_short_nested_expression_stays_on_one_line_under_always() {
    assert_eq!(
        configured(
            "local a = if x then (if y then 1 else 2) else 3",
            conditional(ConditionalExpansion::Always)
        ),
        "local a = if x then\n\t(if y then 1 else 2)\nelse\n\t3\n"
    );
}

#[test]
fn a_nested_expression_over_the_width_opens_as_well() {
    let output = configured(
        "local a = if x then (if someLongCondition then 'a long inner branch' else 'another long inner') else 3",
        conditional(ConditionalExpansion::Always),
    );

    assert!(
        output.contains("\t(\n\t\tif someLongCondition then\n"),
        "{output}"
    );

    assert!(
        output.contains("\t\t\t\"a long inner branch\"\n"),
        "{output}"
    );

    assert!(output.contains("\n\t)\n"), "{output}");
}

#[test]
fn the_width_reaches_a_nested_expression() {
    let configuration = Options {
        conditionals: Conditional {
            expand: ConditionalExpansion::Always,
            width: 10,
            ..Default::default()
        },
        ..Default::default()
    };

    let output = configured(
        "local a = if x then (if y then 1 else 2) else 3",
        configuration,
    );

    assert!(output.contains("(\n\t\tif y then\n"), "{output}");
}

#[test]
fn next_line_starts_the_if_below_the_equals() {
    let configuration = Options {
        conditionals: Conditional {
            expand: ConditionalExpansion::Always,
            placement: Placement::NextLine,
            ..Default::default()
        },
        ..Default::default()
    };

    assert_eq!(
        configured("local a = if bar then 'baz' else 'foo'", configuration),
        "local a =\n\tif bar then\n\t\t\"baz\"\n\telse\n\t\t\"foo\"\n"
    );
}

#[test]
fn next_line_leaves_an_expression_that_stays_flat_where_it_is() {
    let configuration = Options {
        conditionals: Conditional {
            expand: ConditionalExpansion::Needed,
            placement: Placement::NextLine,
            ..Default::default()
        },
        ..Default::default()
    };

    assert_eq!(
        configured("local a = if bar then 'baz' else 'foo'", configuration),
        "local a = if bar then \"baz\" else \"foo\"\n"
    );
}

#[test]
fn the_indent_levels_are_the_projects_to_choose() {
    let configuration = |indent| Options {
        conditionals: Conditional {
            expand: ConditionalExpansion::Always,
            indentation: indent,
            ..Default::default()
        },
        ..Default::default()
    };

    let source = "local a = if bar then 'baz' else 'foo'";

    assert_eq!(
        configured(source, configuration(2)),
        "local a = if bar then\n\t\t\"baz\"\nelse\n\t\t\"foo\"\n"
    );

    assert_eq!(
        configured(source, configuration(0)),
        "local a = if bar then\n\"baz\"\nelse\n\"foo\"\n"
    );
}

fn leading(expand: ConditionalExpansion) -> Options {
    Options {
        conditionals: Conditional {
            expand,
            style: ConditionalStyle::Leading,
            ..Default::default()
        },
        ..Default::default()
    }
}

#[test]
fn the_leading_style_puts_the_keyword_first() {
    assert_eq!(
        configured(
            "local a = if bar then 'baz' else 'foo'",
            leading(ConditionalExpansion::Always)
        ),
        "local a = if bar\n\tthen \"baz\"\n\telse \"foo\"\n"
    );
}

#[test]
fn the_leading_style_gives_each_clause_of_a_chain_a_line() {
    assert_eq!(
        configured(
            "local a = if x then 1 elseif y then 2 else 3",
            leading(ConditionalExpansion::Always)
        ),
        "local a = if x\n\tthen 1\n\telseif y\n\tthen 2\n\telse 3\n"
    );
}

#[test]
fn the_leading_style_is_the_same_on_one_line() {
    assert_eq!(
        configured(
            "local a = if bar then 'baz' else 'foo'",
            leading(ConditionalExpansion::Needed)
        ),
        "local a = if bar then \"baz\" else \"foo\"\n"
    );
}

#[test]
fn the_leading_style_takes_next_line_too() {
    let configuration = Options {
        conditionals: Conditional {
            expand: ConditionalExpansion::Always,
            style: ConditionalStyle::Leading,
            placement: Placement::NextLine,
            ..Default::default()
        },
        ..Default::default()
    };

    assert_eq!(
        configured("local a = if bar then 'baz' else 'foo'", configuration),
        "local a =\n\tif bar\n\t\tthen \"baz\"\n\t\telse \"foo\"\n"
    );
}

fn next_line(width: usize) -> Options {
    Options {
        column_width: width,
        indentation: Indentation {
            style: Whitespace::Spaces,
            width: 4,
        },
        conditionals: Conditional {
            expand: ConditionalExpansion::Always,
            style: ConditionalStyle::Block,
            placement: Placement::NextLine,
            ..Default::default()
        },
        ..Default::default()
    }
}

#[test]
fn a_parenthesised_nested_expression_hangs_off_the_operator() {
    let source = concat!(
        "local option_line =\n",
        "    if option_index == selected then\n",
        "        `{\" \" .. colors.bold.green(\">\")} {index_for_display} {colors.style.underline(option)}` .. (\n",
        "            if submit_on_click then\n",
        "                string.rep(\" \", 4) .. GREEN_BACKGROUND_WITH_WHITE_TEXT .. \" Click again to confirm \" .. colors.codes.RESET\n",
        "            else\n",
        "                \"\"\n",
        "        )\n",
        "    else\n",
        "        `   {index_for_display} {option}`\n",
    );

    assert_eq!(configured(source, next_line(140)), source);
}

#[test]
fn the_parentheses_keep_their_lines_when_the_inner_chain_breaks() {
    let source = concat!(
        "local option_line =\n",
        "    if selected then\n",
        "        `a` .. (\n",
        "            if submit then\n",
        "                string.rep(\" \", 4) .. GREEN_BACKGROUND .. \" Click again to confirm \" .. colors.codes.RESET\n",
        "            else\n",
        "                \"\"\n",
        "        )\n",
        "    else\n",
        "        `b`\n",
    );

    let output = configured(source, next_line(80));

    assert!(
        output.contains("`a` .. (\n"),
        "the operator stays on the line: {output}"
    );

    assert!(
        output.contains("\n        )\n"),
        "the closer takes its own line: {output}"
    );

    assert!(
        output.contains("string.rep(\" \", 4)\n"),
        "the inner chain breaks at this width: {output}"
    );

    assert_eq!(
        configured(&output, next_line(80)),
        output,
        "and it is stable"
    );
}

#[test]
fn an_elseif_chain_opens_below_the_equals() {
    let source = concat!(
        "local scroller_message =\n",
        "    if current_size.x > 60 then\n",
        "        `Scroll up or down to see more options ({options_window.x}-{options_window.y} of {#options} visible)`\n",
        "    elseif current_size.x > 20 then\n",
        "        `({options_window.x}-{options_window.y}/{#options} visible)`\n",
        "    else\n",
        "        \"pls widen\"\n",
    );

    assert_eq!(configured(source, next_line(120)), source);
}

#[test]
fn parentheses_that_did_not_open_are_left_alone() {
    assert_eq!(
        configured(
            "local a = if x then (if y then 1 else 2) else 3",
            conditional(ConditionalExpansion::Always)
        ),
        "local a = if x then\n\t(if y then 1 else 2)\nelse\n\t3\n"
    );
}

#[test]
fn every_if_layout_is_idempotent_and_parses() {
    let sources = [
        "local a = if bar then 'baz' else 'foo'",
        "local a = if x then 1 elseif y then 2 else 3",
        "return if x then 1 else 2",
        "f(if x then 1 else 2)",
        "local t = { a = if x then 1 else 2, b = 3 }",
        "local a = if x then (if y then 1 else 2) else 3",
        "local a = (if x then 1 else 2) + 5",
        "x = if x then 1 else 2",
    ];

    let configs = [
        conditional(ConditionalExpansion::Never),
        conditional(ConditionalExpansion::Always),
        conditional(ConditionalExpansion::Needed),
        Options {
            conditionals: Conditional {
                expand: ConditionalExpansion::Always,
                placement: Placement::NextLine,
                style: ConditionalStyle::Block,
                width: 5,
                indentation: 2,
            },
            ..Default::default()
        },
        leading(ConditionalExpansion::Always),
        leading(ConditionalExpansion::Needed),
        Options {
            conditionals: Conditional {
                expand: ConditionalExpansion::Always,
                placement: Placement::NextLine,
                style: ConditionalStyle::Leading,
                width: 5,
                indentation: 2,
            },
            ..Default::default()
        },
    ];

    for source in sources {
        for configuration in &configs {
            let once = configured(source, configuration.clone());
            let twice = configured(&once, configuration.clone());

            assert_eq!(once, twice, "unstable for {source:?}");

            assert_eq!(
                vermis::parse(once.as_bytes().into()).diagnostics,
                [] as [vermis::Diagnostic; 0]
            );

            for line in once.lines() {
                assert_eq!(line, line.trim_end(), "trailing whitespace from {source:?}");
            }
        }
    }
}

fn table_types(enabled: bool, width: usize, separator: Separator) -> Options {
    Options {
        types: Types {
            tables: Tables {
                enabled,
                width,
                separator,
                ..Default::default()
            },
            ..Default::default()
        },
        ..Default::default()
    }
}

#[test]
fn a_wide_table_type_opens_one_field_per_line() {
    let output = formatted(
        "type Big = { name: string, health: number, position: Vector3, tags: { string } }\n",
    );

    assert_eq!(
        output,
        "type Big = {\n\tname: string,\n\thealth: number,\n\tposition: Vector3,\n\ttags: { string },\n}\n"
    );
}

#[test]
fn a_small_table_type_keeps_its_line() {
    let source = "type Small = { x: number, y: number }\n";

    assert_eq!(formatted(source), source);
}

#[test]
fn annotations_params_returns_and_asserts_all_open() {
    let long = "alpha: number, beta: string, gamma: boolean, delta: Vector3, epsilon: string";

    for source in [
        format!("local x: {{ {long} }} = {{}}\n"),
        format!("local function f(e: {{ {long} }})\nend\n"),
        format!("local function f(): {{ {long} }}\n\treturn {{}}\nend\n"),
        format!("local x = (y :: {{ {long} }})\n"),
    ] {
        let output = formatted(&source);

        assert!(
            output.contains("\talpha: number,\n"),
            "{source} gave {output}"
        );

        assert!(
            output.contains("\tepsilon: string,\n"),
            "{source} gave {output}"
        );

        assert!(
            formatted(&output) == output,
            "must be a fixed point: {output}"
        );
    }
}

#[test]
fn a_short_nested_table_stays_flat_inside_an_open_one() {
    let output = formatted(
        "type T = { first: string, second: number, inner: { deeply: { nested: boolean } }, last: string }\n",
    );

    assert!(
        output.contains("\tinner: { deeply: { nested: boolean } },\n"),
        "{output}"
    );
}

#[test]
fn a_wide_inner_table_opens_its_parent() {
    let output = configured(
        "type T = { inner: { one: number, two: string } }\n",
        table_types(true, 20, Separator::Comma),
    );

    assert_eq!(
        output,
        "type T = {\n\tinner: {\n\t\tone: number,\n\t\ttwo: string,\n\t},\n}\n"
    );
}

#[test]
fn the_separator_option_covers_both_layouts() {
    let semi = table_types(true, 60, Separator::Semicolon);

    assert_eq!(
        configured("type T = { a: number, b: string }\n", semi.clone()),
        "type T = { a: number; b: string }\n"
    );

    let output = configured(
        "type Big = { name: string, health: number, position: Vector3, tags: { string } }\n",
        semi,
    );

    assert!(output.contains("name: string;\n"), "{output}");
    assert!(output.contains("tags: { string };\n"), "{output}");
}

#[test]
fn solver_forms_survive_the_layout() {
    let output = formatted(
        "type M = { read id: string; write score: number; data: { [string]: number }, list: { string }, tail: boolean }\n",
    );

    assert!(output.contains("\tread id: string,\n"), "{output}");
    assert!(output.contains("\twrite score: number,\n"), "{output}");

    assert!(
        output.contains("\tdata: { [string]: number },\n"),
        "{output}"
    );

    assert!(output.contains("\tlist: { string },\n"), "{output}");

    let sect = formatted("type S = { a: number } & { b: string }\n");

    assert_eq!(sect, "type S = { a: number } & { b: string }\n");
}

#[test]
fn separators_inside_generics_and_indexers_do_not_split() {
    let source = "type T = { meta: Map<string, Set<number>>, call: (x: number, y: number) -> { ok: boolean } }\n";
    let output = formatted(source);

    assert!(
        output.contains("\tmeta: Map<string, Set<number>>,\n"),
        "{output}"
    );

    assert!(
        output.contains("\tcall: (x: number, y: number) -> { ok: boolean },\n"),
        "{output}"
    );
}

#[test]
fn a_trailing_separator_in_the_source_is_not_a_field() {
    assert_eq!(
        formatted("type T = { x: number, }\n"),
        "type T = { x: number }\n"
    );
}

#[test]
fn an_empty_table_type_stays_flat() {
    assert_eq!(formatted("type T = {}\n"), "type T = {}\n");
}

#[test]
fn disabled_keeps_the_old_output() {
    let configuration = table_types(false, 60, Separator::Comma);

    let source =
        "type Big = { name: string, health: number, position: Vector3, tags: { string } }\n";

    assert_eq!(configured(source, configuration.clone()), source);

    let wrapped = "type W = {\n\ta: number,\n\tb: string,\n}\n";

    assert_eq!(configured(wrapped, configuration), wrapped);
}

#[test]
fn a_comment_inside_a_table_type_keeps_the_authors_text() {
    let source = "type T = {\n\t-- the id of the row\n\tid: string,\n\tvalue: number,\n}\n";

    assert_eq!(formatted(source), source);
}

#[test]
fn a_type_function_prints_as_written() {
    let source = "type function pick(t)\n\treturn t\nend\n";

    assert_eq!(formatted(source), source);
}

#[test]
fn an_author_wrapped_short_alias_collapses() {
    assert_eq!(
        formatted("type T = {\n\tx: number,\n\ty: number,\n}\n"),
        "type T = { x: number, y: number }\n"
    );
}

fn sorted(order: Order, indexer: Indexer) -> Options {
    Options {
        types: Types {
            tables: Tables {
                order,
                indexer,
                ..Default::default()
            },
            ..Default::default()
        },
        ..Default::default()
    }
}

const ROW: &str = "type Row = { id: string, [number]: any, description: string, hp: number, mana: number, name: string }\n";

#[test]
fn the_property_order_of_the_author_is_the_default() {
    assert_eq!(
        formatted(ROW),
        "type Row = {\n\tid: string,\n\t[number]: any,\n\tdescription: string,\n\thp: number,\n\tmana: number,\n\tname: string,\n}\n"
    );
}

#[test]
fn ascending_sorts_the_shortest_name_first() {
    assert_eq!(
        configured(ROW, sorted(Order::KeyLengthAscending, Indexer::First)),
        "type Row = {\n\t[number]: any,\n\thp: number,\n\tid: string,\n\tmana: number,\n\tname: string,\n\tdescription: string,\n}\n"
    );
}

#[test]
fn descending_sorts_the_longest_name_first() {
    assert_eq!(
        configured(ROW, sorted(Order::KeyLengthDescending, Indexer::First)),
        "type Row = {\n\t[number]: any,\n\tdescription: string,\n\tmana: number,\n\tname: string,\n\thp: number,\n\tid: string,\n}\n"
    );
}

#[test]
fn indexer_position_controls_field_order() {
    let ascending = configured(ROW, sorted(Order::KeyLengthAscending, Indexer::First));

    assert_eq!(
        configured(ROW, sorted(Order::KeyLengthAscending, Indexer::Sorted)),
        ascending
    );

    assert_eq!(
        configured(ROW, sorted(Order::KeyLengthDescending, Indexer::Sorted)),
        "type Row = {\n\tdescription: string,\n\tmana: number,\n\tname: string,\n\thp: number,\n\tid: string,\n\t[number]: any,\n}\n"
    );
}

#[test]
fn a_flat_table_type_sorts_as_well() {
    assert_eq!(
        configured(
            "type S = { name: string, id: number }\n",
            sorted(Order::KeyLengthAscending, Indexer::First)
        ),
        "type S = { id: number, name: string }\n"
    );
}

#[test]
fn a_comment_holds_the_properties_of_its_table_where_they_are() {
    let source = "type C = {\n\t-- the id of the row\n\tidentifier: string,\n\tx: number,\n}\n";

    assert_eq!(
        configured(source, sorted(Order::KeyLengthAscending, Indexer::First)),
        source
    );

    assert_eq!(
        configured(source, sorted(Order::KeyLengthDescending, Indexer::First)),
        source
    );
}

#[test]
fn a_field_with_no_name_leaves_its_table_as_written() {
    let configuration = sorted(Order::KeyLengthAscending, Indexer::First);

    assert_eq!(
        configured("type A = { string }\n", configuration.clone()),
        "type A = { string }\n"
    );

    let quoted = "type Q = { [\"a b\"]: number, zz: string }\n";

    assert_eq!(configured(quoted, configuration), quoted);
}

#[test]
fn a_read_or_write_modifier_travels_with_its_property() {
    assert_eq!(
        configured(
            "type M = { read identifier: string, write hp: number, [string]: any }\n",
            sorted(Order::KeyLengthAscending, Indexer::First)
        ),
        "type M = { [string]: any, write hp: number, read identifier: string }\n"
    );
}

#[test]
fn a_property_named_read_sorts_under_that_name() {
    assert_eq!(
        configured(
            "type K = { abcdef: boolean, write: string, read: number }\n",
            sorted(Order::KeyLengthAscending, Indexer::First)
        ),
        "type K = { read: number, write: string, abcdef: boolean }\n"
    );
}

#[test]
fn the_sort_needs_the_table_type_layout() {
    let configuration = Options {
        types: Types {
            tables: Tables {
                enabled: false,
                order: Order::KeyLengthAscending,
                ..Default::default()
            },
            ..Default::default()
        },
        ..Default::default()
    };

    let source = "type O = { description: string, id: number }\n";

    assert_eq!(configured(source, configuration), source);
}

#[test]
fn every_property_order_is_idempotent_and_parses() {
    let sources = [
        ROW,
        "type S = { name: string, id: number }\n",
        "type A = { string }\n",
        "type Q = { [\"a b\"]: number, zz: string }\n",
        "type C = {\n\t-- a note\n\tid: string,\n\tx: number,\n}\n",
        "type M = { read identifier: string, write hp: number, [string]: any }\n",
        "type N = { inner: { deep: boolean, a: number }, other: Gamma }\n",
        "local x: { name: string, id: number } = t\n",
        "local function f(a: { name: string, id: number })\nend\n",
        "local x = y :: { name: string, id: number }\n",
    ];

    let mut configs = Vec::new();

    for order in [
        Order::Preserve,
        Order::KeyLengthAscending,
        Order::KeyLengthDescending,
    ] {
        for indexer in [Indexer::First, Indexer::Sorted] {
            for column_width in [40, 120] {
                configs.push(Options {
                    column_width,
                    types: Types {
                        tables: Tables {
                            order,
                            indexer,
                            ..Default::default()
                        },
                        ..Default::default()
                    },
                    ..Default::default()
                });
            }
        }
    }

    for source in sources {
        for configuration in &configs {
            let once = configured(source, configuration.clone());
            let twice = configured(&once, configuration.clone());

            assert_eq!(once, twice, "unstable for {source:?}");

            assert_eq!(
                vermis::parse(once.as_bytes().into()).diagnostics,
                [] as [vermis::Diagnostic; 0]
            );

            for line in once.lines() {
                assert_eq!(line, line.trim_end(), "trailing whitespace from {source:?}");
            }
        }
    }
}

fn operators(expand: TypeExpansion) -> Options {
    Options {
        types: Types {
            operators: Operators { expand },
            ..Default::default()
        },
        ..Default::default()
    }
}

const LONG_UNION: &str =
    "type Long = AlphaAlphaAlpha | BetaBetaBetaBeta | GammaGammaGamma | DeltaDeltaDelta\n";

#[test]
fn compact_type_operators_fit_on_one_line() {
    assert_eq!(formatted(LONG_UNION), LONG_UNION);

    let narrow = Options {
        column_width: 40,
        ..Default::default()
    };

    assert_eq!(configured(LONG_UNION, narrow), LONG_UNION);
}

#[test]
fn always_opens_every_member_under_its_operator() {
    assert_eq!(
        configured(
            "type U = Alpha | Beta | Gamma\n",
            operators(TypeExpansion::Always)
        ),
        "type U =\n\t| Alpha\n\t| Beta\n\t| Gamma\n"
    );
}

#[test]
fn always_opens_an_intersection_too() {
    assert_eq!(
        configured(
            "type I = { a: number } & { b: string }\n",
            operators(TypeExpansion::Always)
        ),
        "type I =\n\t& { a: number }\n\t& { b: string }\n"
    );
}

#[test]
fn always_reaches_every_type_position() {
    let configuration = operators(TypeExpansion::Always);

    for source in [
        "local x: Alpha | Beta = nil\n",
        "local function f(a: Alpha | Beta)\nend\n",
        "local function f(): Alpha | Beta\n\treturn nil\nend\n",
        "local x = y :: Alpha | Beta\n",
    ] {
        let output = configured(source, configuration.clone());

        assert!(output.contains("| Alpha\n"), "{source} gave {output}");
        assert!(output.contains("| Beta"), "{source} gave {output}");
    }
}

#[test]
fn a_nested_chain_opens_with_its_parent() {
    assert_eq!(
        configured(
            "type N = { inner: Alpha | Beta } | nil\n",
            operators(TypeExpansion::Always)
        ),
        "type N =\n\t| {\n\t\tinner:\n\t\t\t| Alpha\n\t\t\t| Beta,\n\t}\n\t| nil\n"
    );
}

#[test]
fn never_holds_one_line_over_the_table_type_width() {
    let source = "type W = { alpha: number, beta: string, gamma: boolean, delta: Vector3 } | nil\n";

    assert_eq!(configured(source, operators(TypeExpansion::Never)), source);

    assert_eq!(
        formatted(source),
        "type W = {\n\talpha: number,\n\tbeta: string,\n\tgamma: boolean,\n\tdelta: Vector3,\n} | nil\n"
    );
}

#[test]
fn column_width_outranks_never() {
    let configuration = Options {
        column_width: 40,
        types: Types {
            operators: Operators {
                expand: TypeExpansion::Never,
            },
            ..Default::default()
        },
        ..Default::default()
    };

    assert_eq!(
        configured(LONG_UNION, configuration),
        "type Long =\n\t| AlphaAlphaAlpha\n\t| BetaBetaBetaBeta\n\t| GammaGammaGamma\n\t| DeltaDeltaDelta\n"
    );
}

#[test]
fn never_keeps_a_chain_that_fits_on_its_line() {
    let source = "type U = Alpha | Beta | Gamma\n";

    assert_eq!(configured(source, operators(TypeExpansion::Never)), source);
}

#[test]
fn a_leading_operator_reads_as_the_chain_it_opens() {
    let source = "type L = | Alpha | Beta\n";

    assert_eq!(
        configured(source, operators(TypeExpansion::Always)),
        "type L =\n\t| Alpha\n\t| Beta\n"
    );

    assert_eq!(
        configured(source, operators(TypeExpansion::Never)),
        "type L = Alpha | Beta\n"
    );
}

#[test]
fn an_operator_inside_brackets_opens_nothing() {
    let configuration = operators(TypeExpansion::Always);

    for source in [
        "type G = Map<Alpha | Beta, Gamma>\n",
        "type P = (Alpha | Beta)?\n",
    ] {
        assert_eq!(
            configured(source, configuration.clone()),
            source,
            "{source}"
        );
    }
}

#[test]
fn every_type_layout_is_idempotent_and_parses() {
    let sources = [
        ROW,
        LONG_UNION,
        "type U = Alpha | Beta | Gamma\n",
        "type L = | Alpha | Beta\n",
        "type I = { a: number } & { b: string }\n",
        "type N = { inner: Alpha | Beta, other: Gamma } | nil\n",
        "type W = { alpha: number, beta: string, gamma: boolean, delta: Vector3 } | nil\n",
        "type A = { string }\n",
        "type C = {\n\t-- a note\n\tid: string,\n\tx: number,\n}\n",
        "type M = { read identifier: string, write hp: number, [string]: any }\n",
        "local x: Alpha | Beta = nil\n",
        "local function f(a: Alpha | Beta): Gamma | Delta\n\treturn a\nend\n",
        "local x = y :: Alpha | Beta\n",
        "type Fn = (a: number) -> string | nil\n",
        "type G = Map<string, Set<number>> | Alpha\n",
    ];

    let mut configs = Vec::new();

    for order in [
        Order::Preserve,
        Order::KeyLengthAscending,
        Order::KeyLengthDescending,
    ] {
        for indexer in [Indexer::First, Indexer::Sorted] {
            for expand in [
                TypeExpansion::Needed,
                TypeExpansion::Always,
                TypeExpansion::Never,
            ] {
                for column_width in [40, 120] {
                    configs.push(Options {
                        column_width,
                        types: Types {
                            tables: Tables {
                                order,
                                indexer,
                                ..Default::default()
                            },
                            operators: Operators { expand },
                        },
                        ..Default::default()
                    });
                }
            }
        }
    }

    for source in sources {
        for configuration in &configs {
            let once = configured(source, configuration.clone());
            let twice = configured(&once, configuration.clone());

            assert_eq!(once, twice, "unstable for {source:?}");

            assert_eq!(
                vermis::parse(once.as_bytes().into()).diagnostics,
                [] as [vermis::Diagnostic; 0]
            );

            for line in once.lines() {
                assert_eq!(line, line.trim_end(), "trailing whitespace from {source:?}");
            }
        }
    }
}

#[test]
fn table_type_configuration_preserves_existing_blank_lines() {
    let options: Options =
        toml_edit::de::from_str("[types.tables]\nblank_lines = 'preserve'").unwrap();

    let schema = serde_json::to_value(instar_core::configuration::InstarConfig::schema()).unwrap();

    assert_eq!(
        schema["$defs"]["Tables"]["properties"]["blank_lines"]["default"],
        "remove"
    );

    for (source, expected) in [
        (
            "type Record = {first:number,\n\nsecond:string}",
            "type Record = {\n\tfirst: number,\n\n\tsecond: string,\n}\n",
        ),
        (
            "declare registry: {first:number,\n \n\t\nsecond:string}",
            "declare registry: {\n\tfirst: number,\n\n\n\tsecond: string,\n}\n",
        ),
        (
            "type Record = {child:{first:number,\n\nsecond:string}}",
            "type Record = {\n\tchild: {\n\t\tfirst: number,\n\n\t\tsecond: string,\n\t},\n}\n",
        ),
    ] {
        assert_eq!(configured(source, options.clone()), expected);
        assert_eq!(configured(expected, options.clone()), expected);

        let windows = Options {
            line_endings: Endings::Windows,
            ..options.clone()
        };

        assert_eq!(
            configured(&source.replace('\n', "\r\n"), windows.clone()),
            expected.replace('\n', "\r\n"),
        );

        assert_eq!(
            configured(&expected.replace('\n', "\r\n"), windows),
            expected.replace('\n', "\r\n"),
        );
    }

    let compact = "type Record = { first: number, second: string }\n";
    assert_eq!(configured(compact, options), compact);

    assert_eq!(
        formatted("type Record = {first:number,\n\nsecond:string}"),
        compact
    );
}

#[test]
fn overloads_wrap_between_complete_signatures() {
    let source = "declare convert: ((input: number) -> string) & ((input: string) -> number)";

    let expected =
        "declare convert: ((input: number) -> string)\n\t& ((input: string) -> number)\n";

    let options = narrow(60);
    assert_eq!(configured(source, options.clone()), expected);
    assert_eq!(configured(expected, options), expected);
    assert_eq!(formatted(expected), expected);
    assert_eq!(formatted(source), format!("{source}\n"));
}

#[test]
fn declarations_wrap_parameters_before_return_signatures() {
    let source = "declare function combine<Input..., Output...>(left: (Input...) -> Output..., right: (Input...) -> Output...): (Input...) -> Output...";
    let expected = "declare function combine<Input..., Output...>(\n\tleft: (Input...) -> Output...,\n\tright: (Input...) -> Output...\n): (Input...) -> Output...\n";
    let options = narrow(80);
    assert_eq!(configured(source, options.clone()), expected);
    assert_eq!(configured(expected, options), expected);
}

#[test]
fn external_type_declarations_preserve_their_syntax() {
    for source in [
        "declare extern type Record with\nend\n",
        "declare extern type Record with\n\tread Value: boolean\n\tfunction Apply(self, ...: any): ()\nend\n",
        "declare extern type Entry extends Record with\n\tValue: number\nend\n",
    ] {
        let output = formatted(source);
        assert_eq!(output, source);
        assert_eq!(formatted(&output), output);
    }
}

#[test]
fn a_class_formats_one_member_per_line() {
    let source = "open class Animal\n\tpublic species: string\n\tfunction speak(self)\n\t\treturn \"...\"\n\tend\nend\n";

    assert_eq!(formatted(source), source);
}

#[test]
fn an_extends_clause_and_export_survive() {
    let source = "export class Cat extends Animal\n\tfunction speak(self)\n\t\treturn \"meow\"\n\tend\nend\n";

    assert_eq!(formatted(source), source);
}

#[test]
fn a_field_annotation_takes_the_table_type_layout() {
    let output = formatted(
        "class C\n\tpublic stats: { health: number, stamina: number, position: Vector3, tags: { string } }\nend\n",
    );

    assert!(
        output.contains("\tpublic stats: {\n\t\thealth: number,\n"),
        "{output}"
    );
}

#[test]
fn a_comment_inside_a_class_keeps_the_authors_text() {
    let source = "class C\n\t-- the species tag\n\tpublic species: string\nend\n";

    assert_eq!(formatted(source), source);
}

#[test]
fn export_by_value_prints_as_written() {
    for source in [
        "export local version = \"5.1\"\n",
        "export const TAU = math.pi * 2\n",
        "export function init()\nend\n",
    ] {
        assert_eq!(formatted(source), source);
    }
}

#[test]
fn read_and_write_modifiers_survive_types() {
    let source = "type Cell = { read value: number, write dirty: boolean }\n\
               type Store = { read [string]: number }\n\
               local box: { read data: { string } } = { data = {} }\n";
    let output = formatted(source);

    assert_eq!(output, source);
    assert_eq!(formatted(&output), output, "fixed point");
}

fn calls(expand: Expansion, style: CallStyle) -> Options {
    Options {
        calls: Calls {
            expand,
            style,
            indentation: 1,
            ..Default::default()
        },
        ..Default::default()
    }
}

fn declarations(expand: Expansion) -> Options {
    Options {
        functions: Functions {
            parameters: Parameters {
                expand,
                indentation: 1,
            },
            ..Default::default()
        },
        ..Default::default()
    }
}

const HUG_SOURCE: &str = "Colors:Apply(frame, \"Rarity\", rarity, { Children = { stroke, } })";

#[test]
fn a_call_keeps_its_layout_by_default() {
    assert_eq!(formatted("f(a, b, c)"), "f(a, b, c)\n");
    assert_eq!(formatted("function f(a, b) end"), "function f(a, b)\nend\n");
}

#[test]
fn hug_last_keeps_the_arguments_on_the_line_of_the_call() {
    let output = configured(HUG_SOURCE, calls(Expansion::Needed, CallStyle::HugLast));

    assert_eq!(
        output,
        "Colors:Apply(frame, \"Rarity\", rarity, {\n\tChildren = {\n\t\tstroke,\n\t},\n})\n"
    );
}

#[test]
fn one_per_line_gives_every_argument_a_line() {
    let output = configured(HUG_SOURCE, calls(Expansion::Needed, CallStyle::OnePerLine));

    assert!(output.starts_with("Colors:Apply(\n\tframe,\n"), "{output}");
}

#[test]
fn hug_last_falls_back_where_the_last_argument_is_not_a_block() {
    let source = "someFunction(argumentNumberOne, argumentNumberTwo, argumentNumberThree, argumentNumberFour, argumentNumberFive, argumentSix)";
    let output = configured(source, calls(Expansion::Needed, CallStyle::HugLast));

    assert!(output.contains("(\n\targumentNumberOne,\n"), "{output}");
}

#[test]
fn always_opens_a_call_at_every_width() {
    assert_eq!(
        configured("f(a, b)", calls(Expansion::Always, CallStyle::OnePerLine)),
        "f(\n\ta,\n\tb\n)\n"
    );
}

#[test]
fn always_wins_over_hug_last() {
    let output = configured(HUG_SOURCE, calls(Expansion::Always, CallStyle::HugLast));

    assert!(output.starts_with("Colors:Apply(\n\tframe,\n"), "{output}");
}

#[test]
fn never_keeps_a_long_call_on_one_line() {
    let source = "someFunction(argumentNumberOne, argumentNumberTwo, argumentNumberThree, argumentNumberFour, argumentNumberFive, argumentSix)";
    let output = configured(source, calls(Expansion::Never, CallStyle::OnePerLine));

    assert_eq!(output.lines().count(), 1, "{output}");

    assert!(
        output.len() > 120,
        "the line is meant to run past the width: {output}"
    );
}

#[test]
fn never_still_opens_a_table_inside_the_list() {
    let output = configured(HUG_SOURCE, calls(Expansion::Never, CallStyle::OnePerLine));

    assert!(output.contains("{\n\tChildren"), "{output}");

    assert!(
        output.starts_with("Colors:Apply(frame,"),
        "the list stayed flat: {output}"
    );
}

#[test]
fn a_declaration_opens_when_it_is_told_to() {
    let output = configured(
        "function Component(t: { foo: number }, t1: number, t2: string): number\n\treturn 2\nend",
        declarations(Expansion::Always),
    );

    assert_eq!(
        output,
        "function Component(\n\tt: { foo: number },\n\tt1: number,\n\tt2: string\n): number\n\treturn 2\nend\n"
    );
}

#[test]
fn declarations_and_calls_are_decided_apart() {
    let output = configured(
        "f(a, b)\nfunction g(c, d) end\n",
        declarations(Expansion::Always),
    );

    assert!(
        output.starts_with("f(a, b)\n"),
        "the call is untouched: {output}"
    );

    assert!(output.contains("function g(\n\tc,\n\td\n)"), "{output}");
}

#[test]
fn the_indent_levels_of_a_call_are_the_projects_to_choose() {
    let configuration = Options {
        calls: Calls {
            expand: Expansion::Always,
            style: CallStyle::OnePerLine,
            indentation: 2,
            ..Default::default()
        },
        ..Default::default()
    };

    assert_eq!(configured("f(a)", configuration), "f(\n\t\ta\n)\n");
}

#[test]
fn trailing_comma_expansion_respects_call_layout() {
    let configuration = Options {
        calls: Calls {
            expand: Expansion::Never,
            style: CallStyle::OnePerLine,
            indentation: 1,
            ..Default::default()
        },
        expand_on_trailing_comma: true,
        ..Default::default()
    };

    assert_eq!(
        configured("f(a, b, { x = 1, })", configuration.clone()),
        "f(a, b, {\n\tx = 1,\n})\n"
    );

    assert_eq!(
        configured("f(a, { x = 1, }, b)", configuration),
        "f(a, {\n\tx = 1,\n}, b)\n"
    );
}

#[test]
fn never_and_hug_last_agree_on_a_trailing_table_and_not_on_a_middle_one() {
    let never = Options {
        calls: Calls {
            expand: Expansion::Never,
            style: CallStyle::OnePerLine,
            indentation: 1,
            ..Default::default()
        },
        ..Default::default()
    };

    let hug = calls(Expansion::Needed, CallStyle::HugLast);

    let trailing = "f(a, b, { x = 1, })";

    assert_eq!(
        configured(trailing, never.clone()),
        configured(trailing, hug.clone())
    );

    let middle = "f(a, { x = 1, }, b)";
    assert_ne!(configured(middle, never), configured(middle, hug));
}

#[test]
fn every_list_layout_is_idempotent_and_parses() {
    let sources = [
        HUG_SOURCE,
        "f(a, b, c)",
        "f()",
        "f(function() return 1 end)",
        "function g(a: number, b: string): boolean\n\treturn true\nend",
        "obj:method(1, { x = 2 })",
    ];

    let configs = [
        calls(Expansion::Needed, CallStyle::HugLast),
        calls(Expansion::Always, CallStyle::OnePerLine),
        calls(Expansion::Never, CallStyle::OnePerLine),
        declarations(Expansion::Always),
        declarations(Expansion::Never),
    ];

    for source in sources {
        for configuration in &configs {
            let once = configured(source, configuration.clone());
            let twice = configured(&once, configuration.clone());

            assert_eq!(once, twice, "unstable for {source:?}");

            assert_eq!(
                vermis::parse(once.as_bytes().into()).diagnostics,
                [] as [vermis::Diagnostic; 0]
            );

            for line in once.lines() {
                assert_eq!(line, line.trim_end(), "trailing whitespace from {source:?}");
            }
        }
    }
}

fn constants(enabled: bool, preserve_mutated_tables: bool) -> Options {
    Options {
        bindings: Constants {
            prefer_constant: enabled,
            preserve_mutated_tables,
        },
        ..Default::default()
    }
}

#[test]
fn a_local_keeps_its_keyword_by_default() {
    assert_eq!(
        formatted("local x = 1\nprint(x)\n"),
        "local x = 1\nprint(x)\n"
    );
}

#[test]
fn a_local_that_nothing_reassigns_becomes_const() {
    assert_eq!(
        configured("local x = 1\nprint(x)\n", constants(true, false)),
        "const x = 1\nprint(x)\n"
    );
}

#[test]
fn a_reassigned_local_keeps_its_keyword() {
    assert_eq!(
        configured("local x = 1\nx = 2\nprint(x)\n", constants(true, false)),
        "local x = 1\nx = 2\nprint(x)\n"
    );
}

#[test]
fn the_forms_that_cannot_take_const_keep_their_keyword() {
    assert_eq!(
        configured("local x\nx = 1\nprint(x)\n", constants(true, false)),
        "local x\nx = 1\nprint(x)\n"
    );

    assert_eq!(
        configured(
            "local a, b = 1, 2\nb = 3\nprint(a, b)\n",
            constants(true, false)
        ),
        "local a, b = 1, 2\nb = 3\nprint(a, b)\n"
    );

    assert_eq!(
        configured("local a, b = 1, 2\nprint(a, b)\n", constants(true, false)),
        "const a, b = 1, 2\nprint(a, b)\n"
    );
}

#[test]
fn the_option_keeps_local_on_a_mutated_table() {
    let source = "local t = {}\nt.x = 1\nprint(t)\n";

    assert_eq!(
        configured(source, constants(true, false)),
        "const t = {}\nt.x = 1\nprint(t)\n"
    );

    assert_eq!(configured(source, constants(true, true)), source);

    let inserted = "local u = {}\ntable.insert(u, 1)\nprint(u)\n";
    assert_eq!(configured(inserted, constants(true, true)), inserted);
}

#[test]
fn the_const_rewrite_is_idempotent_and_parses() {
    let sources = [
        "local x = 1\nprint(x)\n",
        "local t = {}\nt.x = 1\nprint(t)\n",
        "const already = 1\nprint(already)\n",
        "local a, b = 1, 2\nprint(a, b)\n",
        "local f = function() end\nf()\n",
    ];

    for source in sources {
        for configuration in [constants(true, false), constants(true, true)] {
            let once = configured(source, configuration.clone());
            let twice = configured(&once, configuration);

            assert_eq!(once, twice, "unstable for {source:?}");

            assert_eq!(
                vermis::parse(once.as_bytes().into()).diagnostics,
                [] as [vermis::Diagnostic; 0]
            );
        }
    }
}

fn imports(mode: Unused) -> Options {
    Options {
        imports: Imports {
            unused: mode,
            ..Default::default()
        },
        ..Default::default()
    }
}

#[test]
fn preserves_unused_bindings_by_default() {
    let source = "local Dead = require(\"@pkg/Dead\")\nreturn 1\n";

    assert_eq!(formatted(source), source);
    assert_eq!(configured(source, imports(Unused::Ignore)), source);
}

#[test]
fn underscore_marks_the_name_and_keeps_the_require() {
    let source = "local Dead = require(\"@pkg/Dead\")\nreturn 1\n";

    assert_eq!(
        configured(source, imports(Unused::Underscore)),
        "local _Dead = require(\"@pkg/Dead\")\nreturn 1\n"
    );
}

#[test]
fn remove_deletes_the_declaration() {
    let source = "local Dead = require(\"@pkg/Dead\")\nreturn 1\n";

    assert_eq!(configured(source, imports(Unused::Remove)), "return 1\n");
}

#[test]
fn an_import_that_only_a_type_uses_survives_both_modes() {
    let source = "local jecs = require(\"@pkg/jecs\")\n\ntype Component = jecs.Component\n\nreturn nil :: Component?\n";

    assert_eq!(configured(source, imports(Unused::Remove)), source);
    assert_eq!(configured(source, imports(Unused::Underscore)), source);
}

#[test]
fn an_import_used_deep_inside_a_type_survives() {
    let source = "local Deep = require(\"@pkg/Deep\")\n\ntype Held = { field: Deep.Thing }\n\nreturn nil :: Held?\n";

    assert_eq!(configured(source, imports(Unused::Remove)), source);
}

#[test]
fn a_used_import_is_never_touched() {
    let source = "local Used = require(\"@pkg/Used\")\n\nreturn Used.make()\n";

    for mode in [Unused::Ignore, Unused::Underscore, Unused::Remove] {
        assert_eq!(
            configured(source, imports(mode)),
            source,
            "{mode:?} changed it"
        );
    }
}

#[test]
fn a_name_already_marked_is_left_as_it_is() {
    let source = "local _Dead = require(\"@pkg/Dead\")\nreturn 1\n";

    assert_eq!(configured(source, imports(Unused::Underscore)), source);
    assert_eq!(configured(source, imports(Unused::Remove)), source);
}

#[test]
fn removing_an_import_keeps_the_comments_around_it() {
    let source =
        "-- the dead one\nlocal Dead = require(\"@pkg/Dead\")\n-- a trailing thought\n\nreturn 1\n";

    let output = configured(source, imports(Unused::Remove));

    assert!(
        !output.contains("require"),
        "the declaration stayed: {output}"
    );

    assert!(output.contains("-- the dead one"), "{output}");
    assert!(output.contains("-- a trailing thought"), "{output}");
}

#[test]
fn a_held_off_import_is_not_removed() {
    let source = "-- instar: format off\nlocal Dead = require(\"@pkg/Dead\")\n-- instar: format on\nreturn 1\n";

    assert_eq!(configured(source, imports(Unused::Remove)), source);
    assert_eq!(configured(source, imports(Unused::Underscore)), source);
}

#[test]
fn a_multi_name_declaration_is_left_alone() {
    let source = "local A, B = require(\"@pkg/A\"), require(\"@pkg/B\")\nreturn 1\n";

    assert_eq!(configured(source, imports(Unused::Remove)), source);
}

#[test]
fn only_a_require_counts_as_an_import() {
    let source = "local dead = 1\nreturn 2\n";

    assert_eq!(configured(source, imports(Unused::Remove)), source);
    assert_eq!(configured(source, imports(Unused::Underscore)), source);
}

#[test]
fn a_dead_import_inside_a_function_is_handled() {
    let source = "local function f()\n\tlocal Dead = require(\"@pkg/Dead\")\nend\n\nreturn f\n";

    assert_eq!(
        configured(source, imports(Unused::Remove)),
        "local function f()\nend\n\nreturn f\n"
    );

    assert_eq!(
        configured(source, imports(Unused::Underscore)),
        "local function f()\n\tlocal _Dead = require(\"@pkg/Dead\")\nend\n\nreturn f\n"
    );
}

#[test]
fn both_modes_are_stable() {
    let source = "-- a note\nlocal Dead = require(\"@pkg/Dead\")\nlocal jecs = require(\"@pkg/jecs\")\n\ntype C = jecs.Component\n\nreturn nil :: C?\n";

    for mode in [Unused::Underscore, Unused::Remove] {
        let once = configured(source, imports(mode));
        let twice = configured(&once, imports(mode));

        assert_eq!(once, twice, "{mode:?} did not settle");
    }
}

#[test]
fn a_value_table_keeps_its_order_by_default() {
    let source = "local t = { zeta = 1, alpha = 2 }\nreturn t\n";

    assert_eq!(configured(source, Options::default()), source);
}

#[test]
fn a_value_table_sorts_when_the_project_asks() {
    let configuration = |order| Options {
        tables: Sorting { order },
        ..Default::default()
    };

    let source = "local t = { zeta = 1, al = 2, mid = 3 }\nreturn t\n";

    assert_eq!(
        configured(source, configuration(Order::Alphabetical)),
        "local t = { al = 2, mid = 3, zeta = 1 }\nreturn t\n"
    );

    assert_eq!(
        configured(source, configuration(Order::KeyLengthAscending)),
        "local t = { al = 2, mid = 3, zeta = 1 }\nreturn t\n"
    );
}

#[test]
fn the_value_sort_leaves_a_table_it_cannot_read_whole() {
    let configuration = Options {
        tables: Sorting {
            order: Order::Alphabetical,
        },
        ..Default::default()
    };

    let positional = "local t = { zeta = 1, 5, alpha = 2 }\nreturn t\n";
    assert_eq!(configured(positional, configuration.clone()), positional);

    let commented = "local t = {\n\tzeta = 1, -- the last one\n\talpha = 2,\n}\nreturn t\n";
    assert_eq!(configured(commented, configuration), commented);
}

#[test]
fn a_sorted_table_keeps_one_separator_and_its_magic_comma() {
    let configuration = Options {
        expand_on_trailing_comma: true,
        tables: Sorting {
            order: Order::Alphabetical,
        },
        ..Default::default()
    };

    let output = configured(
        "local t = {\n\tzeta = 1;\n\talpha = 2,\n}\nreturn t\n",
        configuration,
    );

    assert!(!output.contains(';'), "{output}");

    assert_eq!(
        output,
        "local t = {\n\talpha = 2,\n\tzeta = 1,\n}\nreturn t\n"
    );
}

#[test]
fn width_orders_measure_the_field() {
    let configuration = |order| Options {
        types: Types {
            tables: Tables {
                order,
                indexer: Indexer::Sorted,
                ..Default::default()
            },
            ..Default::default()
        },
        ..Default::default()
    };

    let source = "type T = { ab: SomeVeryLongTypeName, long: no }\nreturn nil\n";

    assert_eq!(
        configured(source, configuration(Order::FieldWidthAscending)),
        "type T = { long: no, ab: SomeVeryLongTypeName }\nreturn nil\n"
    );

    assert_eq!(
        configured(source, configuration(Order::FieldWidthDescending)),
        "type T = { ab: SomeVeryLongTypeName, long: no }\nreturn nil\n"
    );
}

#[test]
fn the_indexer_takes_its_position() {
    use instar_core::configuration::format::Indexer;

    let configuration = |indexer| Options {
        types: Types {
            tables: Tables {
                order: Order::Alphabetical,
                indexer,
                ..Default::default()
            },
            ..Default::default()
        },
        ..Default::default()
    };

    let source = "type T = { foo: number, [number]: string, bar: number }\nreturn nil\n";

    assert_eq!(
        configured(source, configuration(Indexer::First)),
        "type T = { [number]: string, bar: number, foo: number }\nreturn nil\n"
    );

    assert_eq!(
        configured(source, configuration(Indexer::Last)),
        "type T = { bar: number, foo: number, [number]: string }\nreturn nil\n"
    );
}

#[test]
fn a_tie_sorts_the_same_way_every_run() {
    let configuration = Options {
        tables: Sorting {
            order: Order::FieldWidthAscending,
        },
        types: Types {
            tables: Tables {
                order: Order::FieldWidthAscending,
                ..Default::default()
            },
            ..Default::default()
        },
        ..Default::default()
    };

    let source =
        "type T = { bbb: number, aaa: number }\nlocal t = { bbb = 1, aaa = 2 }\nreturn t\n";

    let once = configured(source, configuration.clone());

    assert_eq!(
        once,
        configured(&once, configuration.clone()),
        "the second run moved something"
    );

    assert!(
        once.starts_with("type T = { aaa: number, bbb: number }"),
        "{once}"
    );
}

#[test]
fn the_function_style_leaves_the_forms_that_have_no_keyword() {
    use instar_core::configuration::format::Declaration;

    let configuration = |style| Options {
        functions: Functions {
            binding: style,
            ..Default::default()
        },
        ..Default::default()
    };

    let source = "local C = {}\nfunction plain()\n\treturn 1\nend\nfunction C.method()\n\treturn 2\nend\nfunction C:other()\n\treturn 3\nend\nlocal anon = function()\n\treturn 4\nend\nreturn { C, plain, anon }\n";

    let output = configured(source, configuration(Declaration::Local));

    assert!(output.contains("local function plain()"), "{output}");
    assert!(output.contains("function C.method()"), "{output}");
    assert!(output.contains("function C:other()"), "{output}");
    assert!(output.contains("local anon = function()"), "{output}");
    assert!(!output.contains("local function C"), "{output}");

    let back = configured(&output, configuration(Declaration::Global));

    assert!(back.contains("function plain()"), "{back}");
    assert!(!back.contains("local function plain"), "{back}");
}

#[test]
fn the_const_style_refuses_a_reassigned_name() {
    let configuration = Options {
        functions: Functions {
            binding: instar_core::configuration::format::Declaration::Const,
            ..Default::default()
        },
        ..Default::default()
    };

    let output = configured(
        "function safe()\n\treturn 1\nend\nfunction moved()\n\treturn 2\nend\nmoved = nil\nreturn { safe, moved }\n",
        configuration,
    );

    assert!(output.contains("const function safe()"), "{output}");
    assert!(output.contains("\nfunction moved()"), "{output}");
}

#[test]
fn a_removed_import_leaves_no_blank_line() {
    let configuration = Options {
        imports: Imports {
            unused: instar_core::configuration::format::Unused::Remove,
            ..Default::default()
        },
        ..Default::default()
    };

    let output = configured(
        "local Used = require(\"./a\")\nlocal Dead = require(\"./b\")\nlocal Other = require(\"./c\")\n\nreturn { Used, Other }\n",
        configuration,
    );

    assert_eq!(
        output,
        "local Used = require(\"./a\")\nlocal Other = require(\"./c\")\n\nreturn { Used, Other }\n"
    );
}

fn chained(style: instar_core::configuration::format::Chain, minimum_calls: usize) -> Options {
    Options {
        calls: Calls {
            chains: Chains {
                style,
                minimum_calls,
            },
            ..Default::default()
        },
        ..Default::default()
    }
}

#[test]
fn a_chain_keeps_its_line_by_default() {
    let source = "local a = map.new():some():other()\nreturn a\n";

    assert_eq!(formatted(source), source);
}

#[test]
fn the_method_style_breaks_at_each_call() {
    use instar_core::configuration::format::Chain;

    let output = configured(
        "local a = map.new():some():some1():some2()\nreturn a\n",
        chained(Chain::Method, 3),
    );

    assert_eq!(
        output,
        "local a = map.new()\n\t:some()\n\t:some1()\n\t:some2()\nreturn a\n"
    );
}

#[test]
fn the_full_style_breaks_before_every_step() {
    use instar_core::configuration::format::Chain;

    let output = configured(
        "local a = map.new():some():some1()\nreturn a\n",
        chained(Chain::Full, 3),
    );

    assert_eq!(
        output,
        "local a = map\n\t.new()\n\t:some()\n\t:some1()\nreturn a\n"
    );
}

#[test]
fn the_threshold_and_the_width_both_open_a_chain() {
    use instar_core::configuration::format::Chain;

    let short = "local a = obj:one():two()\nreturn a\n";
    assert_eq!(configured(short, chained(Chain::Method, 3)), short);

    assert_eq!(
        configured(short, chained(Chain::Method, 2)),
        "local a = obj:one()\n\t:two()\nreturn a\n"
    );

    let long = "local aRatherLongBindingName = someModule.new():aLongMethodNameHere():anotherLongMethodName():aThirdLongMethodName():plusMore():evenMore()\nreturn aRatherLongBindingName\n";
    let output = configured(long, chained(Chain::Method, 0));

    assert!(output.contains("\n\t:aLongMethodNameHere()"), "{output}");

    assert!(
        output.lines().all(|line| line.len() <= 120),
        "a line still runs past the width: {output}"
    );
}

#[test]
fn an_opened_chain_formats_to_itself() {
    use instar_core::configuration::format::Chain;

    let configuration = chained(Chain::Full, 3);

    let once = configured(
        "local a = map.new():some():some1()\nreturn a\n",
        configuration.clone(),
    );

    assert_eq!(
        once,
        configured(&once, configuration),
        "the second run moved something"
    );
}

#[test]
fn a_plain_index_is_not_a_chain() {
    use instar_core::configuration::format::Chain;

    for source in [
        "local a = one.two.three\nreturn a\n",
        "local a = obj:only()\nreturn a\n",
    ] {
        assert_eq!(
            configured(source, chained(Chain::Full, 2)),
            source,
            "{source}"
        );
    }
}

fn zero(mode: instar_core::configuration::format::Zero) -> Options {
    Options {
        leading_zero: mode,
        ..Default::default()
    }
}

#[test]
fn a_fraction_takes_its_leading_zero_by_default() {
    let source = "local a = .5\nlocal b = -.25\nlocal c = .5e3\nlocal d = 0.\nlocal e = 0x10\nlocal f = 10.5\nlocal t = { [.5] = 1 }\nreturn { a, b, c, d, e, f, t }\n";
    let want = "local a = 0.5\nlocal b = -0.25\nlocal c = 0.5e3\nlocal d = 0.\nlocal e = 0x10\nlocal f = 10.5\nlocal t = { [0.5] = 1 }\nreturn { a, b, c, d, e, f, t }\n";

    assert_eq!(formatted(source), want);
}

#[test]
fn strip_removes_the_leading_zero_where_a_fraction_follows() {
    use instar_core::configuration::format::Zero;

    let source = "local a = 0.5\nlocal b = -0.25\nlocal c = 0.5e3\nlocal d = 0.\nlocal e = 0x10\nlocal f = 10.5\nlocal g = 0\nreturn { a, b, c, d, e, f, g }\n";
    let want = "local a = .5\nlocal b = -.25\nlocal c = .5e3\nlocal d = 0.\nlocal e = 0x10\nlocal f = 10.5\nlocal g = 0\nreturn { a, b, c, d, e, f, g }\n";

    assert_eq!(configured(source, zero(Zero::Strip)), want);
}

#[test]
fn the_leading_zero_modes_are_stable() {
    use instar_core::configuration::format::Zero;

    let mixed = "local a = .5\nlocal b = 0.5\nreturn { a, b }\n";

    assert_eq!(configured(mixed, zero(Zero::Preserve)), mixed);

    for mode in [Zero::Add, Zero::Strip] {
        let once = configured(mixed, zero(mode));

        assert_eq!(
            once,
            configured(&once, zero(mode)),
            "{mode:?} moved on a second run"
        );
    }
}
