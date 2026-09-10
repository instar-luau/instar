use std::{error::Error, fs};

use instar_core::format::{self, Endings, Options, Quotes, Whitespace, Zero};

type TestResult = Result<(), Box<dyn Error>>;

fn formatted(source: &str, options: &Options) -> String {
    String::from_utf8(format::format(source.as_bytes(), options).expect("formats")).expect("UTF-8")
}

#[test]
fn formats_syntax_layouts() {
    let options = Options::default();

    for (source, expected) in [
        ("local  a=1", "local a = 1\n"),
        (
            "if a then if b then c() end end",
            "if a then\n\tif b then\n\t\tc()\n\tend\nend\n",
        ),
        ("local t = { a, b, }", "local t = {\n\ta,\n\tb,\n}\n"),
        ("local t={a=1}", "local t = { a = 1 }\n"),
        ("local a=1\n\n\nlocal b=2", "local a = 1\n\nlocal b = 2\n"),
        (
            "local function f(a:number):number return a end",
            "local function f(a: number): number\n\treturn a\nend\n",
        ),
        ("", "\n"),
    ] {
        assert_eq!(formatted(source, &options), expected, "{source:?}");

        assert_eq!(
            formatted(expected, &options),
            expected,
            "not idempotent: {source:?}"
        );
    }
}

#[test]
fn syntax_and_comments_survive() {
    let options = Options::default();

    for source in [
        "local a, b: number = 1, 2",
        "local t = { a = 1, [2] = 'x', 3 }",
        "local f = function(a, b) return a + b end",
        "function M.thing:method(a: number): string return tostring(a) end",
        "if a then b() elseif c then d() else e() end",
        "for i = 1, 10, 2 do print(i) end",
        "for k, v in pairs(t) do print(k, v) end",
        "while true do break end",
        "repeat x() until done",
        "do local scoped = 1 end",
        "export type Thing = { a: number, b: string? }",
        "type Callback<T> = (value: T) -> (T, string)",
        "type Pack<T...> = (T...) -> ()",
        "local x = a and b or c",
        "local x = - -y + #z",
        "local x = (a + b) * c",
        "local s = `interp {value} here`",
        "local s = [[\nlong\n]]",
        "-- leading\nlocal a = 1 -- trailing\n\n-- after a gap\nlocal b = 2",
        "--[[\n\ta long comment\n]]\nlocal a = 1",
        "local x = obj:method(1):chain(2).field",
        "return",
        "local x = value :: SomeType",
        "local x = if cond then a else b",
        "continue",
        "local f = require('@pkg/thing')",
        "do -- a note on the keyword\nx() end",
        "local value = { -- inside\na = 1, }",
        "f(); (g)()",
        "local empty = function( ) end",
        "local array: {number} = {}",
    ] {
        let output = formatted(source, &options);

        assert_eq!(
            formatted(&output, &options),
            output,
            "not idempotent: {source:?}"
        );

        assert_eq!(vermis::parse(output.as_bytes().into()).diagnostics, []);
    }
}

#[test]
fn widths_and_line_endings_are_configurable() {
    let options = Options {
        column_width: 16,
        ..Options::default()
    };

    assert_eq!(
        formatted("f(alpha, beta, gamma)", &options),
        "f(\n\talpha,\n\tbeta,\n\tgamma\n)\n"
    );

    let options = Options {
        indentation: instar_core::format::configuration::Indentation {
            style: Whitespace::Spaces,
            ..Default::default()
        },
        line_endings: Endings::Windows,
        ..Options::default()
    };

    assert_eq!(
        formatted("do f() end", &options),
        "do\r\n    f()\r\nend\r\n"
    );
}

#[test]
fn formats_literal_quotes() {
    for source in [
        "return 'hello'",
        "return 'a\\\"b'",
        "return 'a\\\'b'",
        "return '\\\\'",
        "return .5",
    ] {
        let output = formatted(source, &Options::default());
        assert_eq!(formatted(&output, &Options::default()), output);
    }

    assert_eq!(
        formatted("return 'hello'", &Options::default()),
        "return \"hello\"\n"
    );

    assert_eq!(formatted("return .5", &Options::default()), "return 0.5\n");

    let options = Options {
        quotes: Quotes::Single,
        leading_zero: Zero::Strip,
        ..Options::default()
    };

    assert_eq!(
        formatted("return \"hello\", 0.5", &options),
        "return 'hello', .5\n"
    );
}

#[test]
fn suppression_preserves_regions() {
    let counted = "-- instar: format off(1)\nlocal  a=1\nlocal b=2";

    assert_eq!(
        formatted(counted, &Options::default()),
        "-- instar: format off(1)\nlocal  a=1\nlocal b = 2\n"
    );

    let source = "-- instar: format off\nlocal  value=1";
    assert_eq!(formatted(source, &Options::default()), source);

    let source = "local a=1\n-- instar: format off\nlocal  b=2\n\n\nlocal c =3\n-- instar: format on\nlocal d=4";
    let expected = "local a = 1\n-- instar: format off\nlocal  b=2\n\n\nlocal c =3\n-- instar: format on\nlocal d = 4\n";
    assert_eq!(formatted(source, &Options::default()), expected);
    assert_eq!(formatted(expected, &Options::default()), expected);
}

#[test]
fn formatting_uses_luau_parsing() {
    let source = b"return <Frame />";

    assert_eq!(
        vermis::parse_luaux(source.as_slice().into()).diagnostics,
        []
    );

    assert!(format::format(source, &Options::default()).is_err());

    assert_eq!(
        formatted("return a < b", &Options::default()),
        "return a < b\n"
    );
}

#[test]
fn calls_declarations_and_separators_follow_configuration() {
    use instar_core::format::configuration::{Expansion, Parentheses, Semicolons, Separation};

    for (parentheses, source, expected) in [
        (Parentheses::OmitString, "f('x')", "f \"x\"\n"),
        (Parentheses::OmitTable, "f({x=1})", "f { x = 1 }\n"),
        (Parentheses::OmitOptional, "f(value)", "f(value)\n"),
        (Parentheses::Preserve, "f 'x'", "f \"x\"\n"),
    ] {
        let options = Options {
            calls: instar_core::format::configuration::Calls {
                parentheses,
                ..Default::default()
            },
            ..Options::default()
        };

        assert_eq!(formatted(source, &options), expected);
        assert_eq!(formatted(expected, &options), expected);
    }

    let options = Options {
        spacing: instar_core::format::configuration::Spacing {
            function_names: Separation::Always,
            ..Default::default()
        },
        ..Options::default()
    };

    assert_eq!(
        formatted("function f(a) g(a) end", &options),
        "function f (a)\n\tg (a)\nend\n"
    );

    let options = Options {
        semicolons: Semicolons::Always,
        ..Options::default()
    };

    assert_eq!(
        formatted("local a=1; f()", &options),
        "local a = 1;\nf();\n"
    );

    assert_eq!(
        formatted("local a=1; f()", &Options::default()),
        "local a = 1\nf()\n"
    );

    assert_eq!(
        formatted("f(); (g)()", &Options::default()),
        "f();\n(g)()\n"
    );

    let options = Options {
        functions: instar_core::format::configuration::Functions {
            parameters: instar_core::format::configuration::Parameters {
                expand: Expansion::Always,
                ..Default::default()
            },
            ..Default::default()
        },
        ..Options::default()
    };

    assert_eq!(
        formatted("function f(a,b) return a end", &options),
        "function f(\n\ta,\n\tb\n)\n\treturn a\nend\n"
    );

    let options = Options {
        column_width: 8,
        functions: instar_core::format::configuration::Functions {
            parameters: instar_core::format::configuration::Parameters {
                expand: Expansion::Never,
                ..Default::default()
            },
            ..Default::default()
        },
        ..Options::default()
    };

    assert_eq!(
        formatted("function f(first,second) end", &options),
        "function f(first, second)\nend\n"
    );
}

#[test]
fn call_and_conditional_layouts_follow_configuration() {
    use instar_core::format::configuration::{
        CallStyle, ConditionalExpansion, ConditionalStyle, Expansion, Placement,
    };

    let options = Options {
        calls: instar_core::format::configuration::Calls {
            expand: Expansion::Always,
            indentation: 2,
            ..Default::default()
        },
        ..Options::default()
    };

    assert_eq!(formatted("f(a,b)", &options), "f(\n\t\ta,\n\t\tb\n)\n");

    let options = Options {
        column_width: 5,
        calls: instar_core::format::configuration::Calls {
            expand: Expansion::Never,
            ..Default::default()
        },
        ..Options::default()
    };

    assert_eq!(formatted("f(first,second)", &options), "f(first, second)\n");

    assert_eq!(
        formatted(
            "thing:Connect(function(a) print(a) end)",
            &Options::default()
        ),
        "thing:Connect(function(a)\n\tprint(a)\nend)\n"
    );

    let options = Options {
        calls: instar_core::format::configuration::Calls {
            style: CallStyle::HugLast,
            ..Default::default()
        },
        ..Options::default()
    };

    assert_eq!(
        formatted("f(a,{value=1,})", &options),
        "f(a, {\n\tvalue = 1,\n})\n"
    );

    let options = Options {
        conditionals: instar_core::format::configuration::Conditional {
            expand: ConditionalExpansion::Always,
            ..Default::default()
        },
        ..Options::default()
    };

    let expected = "local value = if condition then\n\tfirst\nelse\n\tsecond\n";

    assert_eq!(
        formatted("local value=if condition then first else second", &options),
        expected
    );

    assert_eq!(formatted(expected, &options), expected);

    let options = Options {
        conditionals: instar_core::format::configuration::Conditional {
            expand: ConditionalExpansion::Always,
            style: ConditionalStyle::Leading,
            placement: Placement::NextLine,
            ..Default::default()
        },
        ..Options::default()
    };

    assert_eq!(
        formatted("local value=if condition then first else second", &options),
        "local value =\n\tif condition\n\t\tthen first\n\t\telse second\n"
    );
}

#[test]
fn table_type_layout_follows_configuration() {
    use instar_core::format::configuration::Separator;
    let source = "type Record = { first: number, second: string }";

    let options = Options {
        types: instar_core::format::configuration::Types {
            tables: instar_core::format::configuration::Tables {
                width: 10,
                separator: Separator::Semicolon,
                ..Default::default()
            },
            ..Default::default()
        },
        ..Options::default()
    };

    let expected = "type Record = {\n\tfirst: number;\n\tsecond: string;\n}\n";
    assert_eq!(formatted(source, &options), expected);
    assert_eq!(formatted(expected, &options), expected);

    let options = Options {
        column_width: 10,
        types: instar_core::format::configuration::Types {
            tables: instar_core::format::configuration::Tables {
                enabled: false,
                ..Default::default()
            },
            ..Default::default()
        },
        ..Options::default()
    };

    assert_eq!(formatted(source, &options), format!("{source}\n"));
}

#[test]
fn nested_configuration_overrides() -> TestResult {
    let directory = tempfile::tempdir()?;
    let root = directory.path();
    fs::create_dir(root.join("nested"))?;

    fs::write(
        root.join("instar.toml"),
        "[format]\nexpand_on_trailing_comma = false\nspacing.braces = false\ntrailing_separator = false\nfinal_newline = true\n[format.functions.parameters]\nexpand = 'always'\n",
    )?;

    fs::write(
        root.join("nested/instar.toml"),
        "[format]\nfinal_newline = false\n[format.functions.parameters]\nindentation = 2\n",
    )?;

    let options = Options::discover(&root.join("nested/main.luau"), None)?;
    assert!(!options.final_newline);
    assert!(!options.trailing_separator);
    assert!(!options.spacing.braces);
    assert_eq!(options.functions.parameters.indentation, 2);

    assert_eq!(
        formatted("function f(a) end", &options),
        "function f(\n\t\ta\n)\nend"
    );

    Ok(())
}

#[test]
fn rejects_invalid_source_and_options() {
    for source in [
        b"local = = =".as_slice(),
        b"local x = [[unterminated",
        b"\xff",
    ] {
        assert!(format::format(source, &Options::default()).is_err());
    }

    assert!(
        format::format(
            b"return 1",
            &Options {
                column_width: 0,
                ..Options::default()
            }
        )
        .is_err()
    );

    let source = b"\xff";

    assert_eq!(
        format::format(
            source,
            &Options {
                enabled: false,
                ..Options::default()
            }
        )
        .expect("disabled"),
        source
    );
}

#[test]
fn configuration_inherits_fields_by_proximity() -> TestResult {
    let directory = tempfile::tempdir()?;
    let root = directory.path();
    fs::create_dir(root.join("nested"))?;

    fs::write(
        root.join("instar.toml"),
        "[format]\nindentation.style = 'spaces'\ncolumn_width = 60\n",
    )?;

    fs::write(
        root.join("nested/instar.toml"),
        "[format]\ncolumn_width = 30\n",
    )?;

    let path = root.join("nested/main.luau");
    let options = Options::discover(&path, None)?;
    assert_eq!(options.column_width, 30);
    assert!(matches!(options.indentation.style, Whitespace::Spaces));

    fs::write(
        root.join("nested/instar.toml"),
        "[format]\ncolumn_width = 20\n",
    )?;

    assert_eq!(Options::discover(&path, None)?.column_width, 20);

    fs::write(
        root.join("nested/instar.toml"),
        "[format]\nunknown = true\n",
    )?;

    assert!(Options::discover(&path, None).is_err());

    Ok(())
}
