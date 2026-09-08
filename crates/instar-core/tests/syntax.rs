use std::{collections::BTreeSet, error::Error, fs, path::Path, sync::Arc};

use instar_core::{
    source::SourceStore,
    syntax::{
        EntryPoint, Feature, NumberStatus, NumberValue, Parse, ParseOptions, SyntaxKind as K,
        number_value, string_bytes,
    },
};

type TestResult = Result<(), Box<dyn Error>>;

fn parse(text: &str) -> Result<Parse, Box<dyn Error>> {
    let source = SourceStore::default().open(std::path::Path::new("syntax-test.luau"), 1, text)?;
    Ok(Parse::new(source)?)
}

fn lossless(parsed: &Parse, text: &str) -> TestResult {
    assert_eq!(parsed.syntax().text().to_string(), text);
    let mut end = 0;
    for token in parsed
        .syntax()
        .descendants_with_tokens()
        .filter_map(rowan::NodeOrToken::into_token)
    {
        let range = token.text_range();
        assert_eq!(u32::from(range.start()), end);
        assert_eq!(parsed.source().slice(range)?, token.text().as_bytes());
        end = u32::from(range.end());
    }
    assert_eq!(usize::try_from(end)?, text.len());
    Ok(())
}

#[test]
fn syntax_limits_are_local_and_comments_are_not_false_errors() -> TestResult {
    for text in ["--[[closed]]", "--[ordinary line comment"] {
        let parsed = parse(text)?;
        assert!(
            parsed.errors().is_empty(),
            "{text:?}: {:?}",
            parsed.errors()
        );
        lossless(&parsed, text)?;
    }
    let broken = parse("--[[unfinished")?;
    assert_ne!(broken.errors(), []);
    assert!(
        broken
            .errors()
            .iter()
            .any(|error| error.message.contains("comment"))
    );
    lossless(&broken, "--[[unfinished")?;
    let source = SourceStore::default().open(
        Path::new("limit-local.luau"),
        1,
        "local a: number?; local b: string?",
    )?;
    let parsed = Parse::with_options(
        source,
        ParseOptions {
            features: BTreeSet::new(),
            recursion_limit: None,
            type_length_limit: Some(1),
            error_limit: None,
        },
    )?;
    assert!(
        parsed.errors().is_empty(),
        "per-type length leaked across annotations: {:?}",
        parsed.errors()
    );
    lossless(&parsed, "local a: number?; local b: string?")?;
    let below = parse("function f(): () -> () -> () end")?;
    assert!(
        below.errors().is_empty(),
        "below-limit recursion rejected: {:?}",
        below.errors()
    );
    let source = SourceStore::default().open(
        Path::new("recursion-limit.luau"),
        1,
        "function f(): () -> () -> () -> () end",
    )?;
    let above = Parse::with_options(
        source,
        ParseOptions {
            features: BTreeSet::new(),
            recursion_limit: Some(2),
            type_length_limit: None,
            error_limit: None,
        },
    )?;
    assert!(
        above
            .errors()
            .iter()
            .any(|error| error.message.contains("recursion"))
    );
    let branch_source = SourceStore::default().open(
        Path::new("branch-limit.luau"),
        1,
        "if true then elseif true then elseif true then end",
    )?;
    let branch = Parse::with_options(
        branch_source,
        ParseOptions {
            features: BTreeSet::new(),
            recursion_limit: Some(2),
            type_length_limit: None,
            error_limit: None,
        },
    )?;
    assert!(
        branch
            .errors()
            .iter()
            .any(|error| error.message.contains("recursion"))
    );
    lossless(&above, "function f(): () -> () -> () -> () end")
}

#[test]
fn syntax_limits_replay_upstream_defaults_and_overrides() -> TestResult {
    let source =
        SourceStore::default().open(Path::new("limits.luau"), 1, "local x = (((((1)))))")?;
    let parse = Parse::with_options(
        source,
        ParseOptions {
            features: BTreeSet::new(),
            recursion_limit: Some(2),
            type_length_limit: Some(2),
            error_limit: Some(1),
        },
    )?;
    assert!(parse.errors().len() <= 1);
    assert!(
        parse
            .errors()
            .iter()
            .any(|error| error.message.contains("limit"))
    );
    lossless(&parse, "local x = (((((1)))))")
}

#[test]
fn syntax_diagnostics_preserve_upstream_delimiter_coordinates() -> TestResult {
    for (source, message) in [
        (
            "return (1",
            "Expected ')' (to close '(' at column 8), got <eof>",
        ),
        (
            "return (1\n",
            "Expected ')' (to close '(' at line 1), got <eof>",
        ),
        ("return 0b123", "Malformed number"),
    ] {
        let parsed = parse(source)?;
        assert_eq!(parsed.errors().len(), 1, "{source}: {:?}", parsed.errors());
        assert_eq!(parsed.errors()[0].message, message);
        lossless(&parsed, source)?;
    }
    Ok(())
}

#[test]
fn syntax_retains_trivia_ranges_and_revision() -> TestResult {
    let mut store = SourceStore::default();
    let text = "-- café\r\nlocal value: string = [==[你好\n]==] -- tail\r\nreturn value\n";
    let source = store.open(std::path::Path::new("revision.luau"), 1, text)?;
    let parsed = Parse::new(Arc::clone(&source))?;
    let updated = store.update(&source, 2, "return 2")?;
    assert!(parsed.errors().is_empty(), "{:?}", parsed.errors());
    assert!(Arc::ptr_eq(parsed.source(), &source));
    assert_ne!(parsed.source().revision(), updated.revision());
    lossless(&parsed, text)
}

#[test]
fn syntax_structures_bindings_scopes_and_require_calls() -> TestResult {
    let text = r#"local outer = require("./module")
local function build<T>(value: T, ...: string): T
    local captured = outer
    for index, item in pairs(value) do
        if item then
            captured = item
        elseif index > 2 then
            break
        else
            continue
        end
    end
    repeat local ready = captured until ready
    return captured
end
function service:run(arg)
    while arg do arg -= 1 end
    do local hidden = arg end
    return self:finish(build(arg))
end
"#;
    let parsed = parse(text)?;
    assert!(parsed.errors().is_empty(), "{:?}", parsed.errors());
    for kind in [
        K::Binding,
        K::FunctionStatement,
        K::FunctionBody,
        K::Parameters,
        K::ForStatement,
        K::IfBranch,
        K::RepeatStatement,
        K::WhileStatement,
        K::DoStatement,
        K::CallExpression,
        K::MethodExpression,
    ] {
        assert!(
            parsed
                .syntax()
                .descendants()
                .any(|node| node.kind() == kind),
            "{kind:?}"
        );
    }
    let require = parsed
        .syntax()
        .descendants()
        .find(|node| node.kind() == K::CallExpression && node.text() == "require(\"./module\")")
        .expect("require call");
    assert_eq!(
        require.children().next().expect("callee").kind(),
        K::NameExpression
    );
    assert!(require.children().any(|node| node.kind() == K::Arguments));
    lossless(&parsed, text)
}

#[test]
fn syntax_interpolation_keeps_nested_expressions_visible() -> TestResult {
    let text = r#"local message = `héllo \{literal\} {format({key = `nested {require('./inside')}`})} \u{41}`
local quoted = "escaped \" quote"
--[=[ long comment ` { ]=]
return message
"#;
    let parsed = parse(text)?;
    assert!(parsed.errors().is_empty(), "{:?}", parsed.errors());
    assert_eq!(
        parsed
            .syntax()
            .descendants()
            .filter(|node| node.kind() == K::InterpolationExpression)
            .count(),
        2
    );
    assert!(
        parsed
            .syntax()
            .descendants()
            .any(|node| node.kind() == K::CallExpression && node.text() == "require('./inside')")
    );
    lossless(&parsed, text)
}

#[test]
fn syntax_types_do_not_turn_names_or_keys_into_value_references() -> TestResult {
    let text = r"export type Result<T> = {value: T, [string]: number} | nil
type Callback = (name: string, number) -> (boolean, string)
type Module = typeof(require('./types'))
local object: Result<number> = {value = 1, [key] = other}
local callback = function(x: number): number return x end
return (object.value :: number) + 2 * 3 ^ 4 ^ 5, if true then 1 else 2
";
    let parsed = parse(text)?;
    assert!(parsed.errors().is_empty(), "{:?}", parsed.errors());
    assert_eq!(
        parsed
            .syntax()
            .descendants()
            .filter(|node| node.kind() == K::TypeAlias)
            .count(),
        3
    );
    assert!(
        parsed
            .syntax()
            .descendants()
            .any(|node| node.kind() == K::TypeofType)
    );
    assert!(
        !parsed
            .syntax()
            .descendants()
            .any(|node| node.kind() == K::NameExpression && node.text() == "value")
    );
    lossless(&parsed, text)
}

#[test]
fn syntax_matches_upstream_binary_precedence_and_contextual_type() -> TestResult {
    let parsed = parse("type('a')\ntype = nil\nreturn -2^2 .. 'x' .. 'y'")?;
    assert!(parsed.errors().is_empty(), "{:?}", parsed.errors());
    assert!(
        !parsed
            .syntax()
            .descendants()
            .any(|node| node.kind() == K::TypeAlias)
    );
    let unary = parsed
        .syntax()
        .descendants()
        .find(|node| node.kind() == K::UnaryExpression)
        .expect("unary");
    assert_eq!(unary.text().to_string(), "-2^2");
    assert_eq!(
        unary.children().next().expect("power").kind(),
        K::BinaryExpression
    );
    assert!(
        parsed
            .syntax()
            .descendants()
            .any(|node| node.kind() == K::BinaryExpression && node.text() == "'x' .. 'y'")
    );
    Ok(())
}

#[test]
fn syntax_recovery_preserves_input_and_following_statements() -> TestResult {
    for text in [
        "local x =\nlocal y = 2",
        "local x = 'broken\nreturn 1",
        "--[=[unterminated",
        "local x = `broken {name",
        "function f(x) local y = 1",
        "\0λ\nreturn 1",
    ] {
        let parsed = parse(text)?;
        assert!(!parsed.errors().is_empty(), "{text:?}");
        lossless(&parsed, text)?;
    }
    let text = "local before = 1\ntype function make()\nlocal hidden = require('./hidden')\nend\nlocal after = 2";
    let parsed = parse(text)?;
    assert!(parsed.errors().is_empty(), "{:?}", parsed.errors());
    assert_eq!(
        parsed
            .syntax()
            .descendants()
            .filter(|node| node.kind() == K::LocalStatement)
            .count(),
        3
    );
    lossless(&parsed, text)
}

#[test]
fn syntax_number_boundaries_match_upstream_lexer() -> TestResult {
    let text = "return 0x1e+2, 0b10_01, .5, 1.25e-2, 1_000";
    let parsed = parse(text)?;
    assert!(parsed.errors().is_empty(), "{:?}", parsed.errors());
    assert!(
        parsed
            .syntax()
            .descendants_with_tokens()
            .filter_map(rowan::NodeOrToken::into_token)
            .any(|token| token.kind() == K::Number && token.text() == "0x1e")
    );
    lossless(&parsed, text)?;
    for malformed in ["return 1..2", "return 1e+", "return 0b2", "return 0x"] {
        let parsed = parse(malformed)?;
        assert!(!parsed.errors().is_empty(), "{malformed}");
        lossless(&parsed, malformed)?;
    }
    Ok(())
}

#[test]
fn syntax_prefix_recovery_and_nesting_limit_terminate() -> TestResult {
    let text = "local f = function(x: {value: string}) return `hello {x.value}` end\nreturn f({value = '世界'})";
    for (end, _) in text
        .char_indices()
        .chain(std::iter::once((text.len(), '\0')))
    {
        lossless(&parse(&text[..end])?, &text[..end])?;
    }
    let nested = format!("return {}1{}", "(".repeat(1100), ")".repeat(1100));
    let parsed = parse(&nested)?;
    assert!(
        parsed
            .errors()
            .iter()
            .any(|error| error.message.contains("recursion limit"))
    );
    lossless(&parsed, &nested)
}

#[test]
fn syntax_preserves_upstream_interpolation_fixture() -> TestResult {
    let text = include_str!("../../../vendor/luau/tests/conformance/stringinterp.luau");
    let parsed = parse(text)?;
    assert!(parsed.errors().is_empty(), "{:?}", parsed.errors());
    lossless(&parsed, text)
}

#[test]
fn syntax_reports_invalid_identifier_bytes_without_replacing_source() -> TestResult {
    let root = tempfile::tempdir()?;
    let path = root.path().join("invalid.luau");
    fs::write(&path, [0xff, b'a'])?;
    let source = SourceStore::default().read(&path)?;
    assert_ne!(Parse::new(Arc::clone(&source))?.errors(), []);
    assert_eq!(source.bytes(), &[0xff, b'a']);
    Ok(())
}

#[test]
fn byte_literals_and_fragments_retain_original_values() -> TestResult {
    let root = tempfile::tempdir()?;
    let path = root.path().join("bytes.luau");
    let prefix = b"--!\xff\n";
    let body = b"return '\xff', [=[\r\n\xfe\r\nx]=], `\xfd{1}\xfc`, '\x1a'";
    let bytes = [prefix.as_slice(), body.as_slice()].concat();
    fs::write(&path, &bytes)?;
    let source = SourceStore::default().read(&path)?;
    for parsed in [
        Parse::new(Arc::clone(&source))?,
        Parse::fragment(
            Arc::clone(&source),
            text_size::TextRange::new(prefix.len().try_into()?, bytes.len().try_into()?),
            ParseOptions::default(),
            instar_core::syntax::EntryPoint::Module,
        )?,
    ] {
        assert!(parsed.errors().is_empty(), "{:?}", parsed.errors());
        assert_eq!(parsed.source().bytes(), bytes);
        assert_eq!(
            usize::from(parsed.syntax().text_range().len()),
            usize::from(parsed.extent().len())
        );
        let values: Result<Vec<_>, _> = parsed
            .syntax()
            .descendants_with_tokens()
            .filter_map(rowan::NodeOrToken::into_token)
            .filter(|token| matches!(token.kind(), K::String | K::InterpolationText))
            .map(|token| string_bytes(&parsed, &token))
            .collect();
        assert_eq!(
            values?,
            [
                b"\xff".to_vec(),
                b"\xfe\nx".to_vec(),
                b"\xfd".to_vec(),
                b"\xfc".to_vec(),
                b"\x1a".to_vec()
            ]
        );
        if parsed.extent().start() == 0.into() {
            assert_eq!(parsed.hot_comments()[0].text, b"\xff");
        }
    }
    Ok(())
}

#[test]
fn syntax_string_decoding_matches_luau_byte_rules() -> TestResult {
    for (text, expected) in [
        (r"'\x2e/\100ep'", b"./dep".as_slice()),
        (r"'\q\z  text'", b"qtext".as_slice()),
        (r"'\u{1f600}'", "😀".as_bytes()),
        (r"'\255\u{d800}'", &[255, 0xed, 0xa0, 0x80]),
        ("[=[\r\nfirst\r\nlast]=]", b"first\nlast".as_slice()),
    ] {
        let parsed = parse(&format!("return {text}"))?;
        let token = parsed
            .syntax()
            .descendants_with_tokens()
            .filter_map(rowan::NodeOrToken::into_token)
            .find(|token| token.kind() == K::String)
            .ok_or("string token")?;
        assert_eq!(string_bytes(&parsed, &token)?, expected);
        assert_eq!(token.text(), text);
    }
    for text in [r"'\999'", r"'\x0g'", r"'\u{}'", r"'\u{110000}'"] {
        let parsed = parse(&format!("return {text}"))?;
        assert_ne!(parsed.errors(), []);
        let token = parsed
            .syntax()
            .descendants_with_tokens()
            .filter_map(rowan::NodeOrToken::into_token)
            .find(|token| token.kind() == K::String)
            .ok_or("string token")?;
        assert!(string_bytes(&parsed, &token).is_err());
    }
    Ok(())
}

fn enabled(text: &str) -> Result<Parse, Box<dyn Error>> {
    let source = SourceStore::default().open(std::path::Path::new("grammar.luau"), 1, text)?;
    Ok(Parse::with_options(
        source,
        ParseOptions {
            recursion_limit: None,
            type_length_limit: None,
            error_limit: None,
            features: [
                Feature::Classes,
                Feature::ConditionalBindings,
                Feature::IntegerLiterals,
                Feature::ValueExports,
                Feature::DebugNoInline,
                Feature::Declarations,
            ]
            .into_iter()
            .collect(),
        },
    )?)
}

#[test]
fn every_added_statement_family_is_structural_and_lossless() -> TestResult {
    for text in [
        "const x: number, y = 1, 2; @native const function f() return x end",
        "if local x: number = f() then print(x) elseif const y = g() then print(y) else print('none') end",
        "open class Widget extends package.Base public value: number public function get(self) return self.value end function __init(self) end end",
        "declare value: string; declare function f<T, P...>(x: T, ...: P...): (T, P...)",
        "declare extern type Widget extends Base with read value: string [number]: string function get(self, x: number): string end",
        "@[native, deprecated({use = 'newName', reason = 'renamed'})] local function f() end",
        "local f = @native function() end; @debugnoinline function g() end",
        "declare handlers: { f: @checked (number) -> string }",
        "type function make(t) type function nested(x) return x end return nested(t) end; export type function other(t) return t end; local after = 1",
        "export local x = 1; export const y = 2; @native export function f() return y end; export open class C public x end",
        "local specialized = f<<number, (string, boolean)>>; local x = obj:method<<string>>('a'); return specialized, f<<>>",
    ] {
        let parsed = enabled(text)?;
        assert!(parsed.errors().is_empty(), "{text}\n{:?}", parsed.errors());
        lossless(&parsed, text)?;
    }
    Ok(())
}

#[test]
fn types_packs_defaults_and_composites_keep_their_roles() -> TestResult {
    for text in [
        "type S<T = number, P... = (string), Q... = ...boolean> = {read x: T, write [number]: string}",
        "type F = <T, P...>(x: T, P...) -> (T, P...); type G = F<>",
        "type U = | string | number; type I = & {a: number} & {b: string}; type O = string??",
        "type Packs = F<(number, string), ()>; type Single = F<(number)>; type Group = F<(number)?>",
        "type A = (() -> ()) & (() -> (number, string)); type B = {read number}; type C = {['other field']: number}",
        "function f(): (number) | string return 1 end; function g(): (number, string) return 1, 'a' end; function h(): ((string)) return 'a' end",
    ] {
        let parsed = enabled(text)?;
        assert!(parsed.errors().is_empty(), "{text}\n{:?}", parsed.errors());
        lossless(&parsed, text)?;
    }
    let parsed =
        enabled("type Nested = ((string)); function f(): (string, ...number) | boolean end")?;
    assert!(parsed.errors().is_empty(), "{:?}", parsed.errors());
    assert_eq!(
        parsed
            .syntax()
            .descendants()
            .filter(|node| node.kind() == K::TypeGroup)
            .count(),
        2
    );
    for kind in [K::TypePack, K::VariadicTypePack, K::UnionType] {
        assert!(
            parsed
                .syntax()
                .descendants()
                .any(|node| node.kind() == kind),
            "{kind:?}"
        );
    }
    lossless(
        &parsed,
        "type Nested = ((string)); function f(): (string, ...number) | boolean end",
    )?;

    let parsed = enabled("type T<P...> = (number, P...) -> (string, P...)")?;
    for kind in [
        K::FunctionType,
        K::GenericPackParameter,
        K::GenericTypePack,
        K::TypePack,
    ] {
        assert!(
            parsed
                .syntax()
                .descendants()
                .any(|node| node.kind() == kind),
            "{kind:?}"
        );
    }
    Ok(())
}

#[test]
fn invalid_grammar_is_reported_without_losing_text() -> TestResult {
    for text in [
        "name",
        "1 + 2",
        "local x = 1; x, y",
        "a + b = 1",
        "f() = 1",
        "a, b += 1",
        "a += 1, 2",
        "for a, b = 1, 2 do end",
        "for a = 1 do end",
        "for a = 1, 2, 3, 4 do end",
        "break",
        "continue",
        "while true do function f() break end end",
        "function f() return ... end",
        "return 1; print(2)",
        "while true do break; print(2) end",
        ";;",
        "return {}.x",
        "return f\n(1)",
        "return f `text`",
        "return x :: T :: U",
        "type T = A | B & C",
        "type T = ()",
        "type T = (number, string)",
        "type F = () -> ((string, number))",
        "type T = P...",
        "type T = ...number",
        "type T = A.B.C",
        "type T = typeof",
        "type T = {number, x: string}",
        "type T = {[number]: string, [string]: number}",
        "type T = (number,) -> string",
        "function f<T = number>() end",
        "type T<P..., A> = number",
        "type T<A = number, B> = B",
        "type T<P... = number> = number",
        "type T = {['a\\0b']: number}",
        "type T = `text`",
        "const x",
        "const x, y = 1",
        "if local a, b = f() then end",
        "if local a then end",
        "@[native, native] function f() end",
        "@unknown function f() end",
        "@native local x = 1",
        "return `x {{a = 1}}`",
        "return '\\999'",
        "return `\\xZZ {1}`",
    ] {
        let parsed = enabled(text)?;
        assert!(!parsed.errors().is_empty(), "accepted: {text}");
        lossless(&parsed, text)?;
    }
    Ok(())
}

#[test]
fn contextual_identifiers_are_disambiguated_after_calls_and_assignments() -> TestResult {
    for text in [
        "declare()",
        "declare = f",
        "continue += 1",
        "continue, x = 1, 2",
        "continue {}",
        "continue 'x'",
        "const()",
        "class()",
        "open()",
        "type()",
        "export()",
    ] {
        let parsed = enabled(text)?;
        assert!(parsed.errors().is_empty(), "{text}: {:?}", parsed.errors());
        assert!(
            !parsed
                .syntax()
                .descendants()
                .any(|node| node.kind() == K::ContinueStatement)
        );
    }
    Ok(())
}

#[test]
fn feature_availability_does_not_discard_structure() -> TestResult {
    for (text, feature, kind) in [
        ("class C public x end", Feature::Classes, K::ClassStatement),
        (
            "if local x = f() then end",
            Feature::ConditionalBindings,
            K::ConditionalBinding,
        ),
        ("return 42i", Feature::IntegerLiterals, K::LiteralExpression),
        (
            "export const x = 1",
            Feature::ValueExports,
            K::ExportStatement,
        ),
        (
            "@debugnoinline function f() end",
            Feature::DebugNoInline,
            K::Attribute,
        ),
        ("declare x: number", Feature::Declarations, K::DeclareGlobal),
    ] {
        let disabled = parse(text)?;
        assert!(!disabled.errors().is_empty(), "{text}");
        assert!(
            disabled
                .feature_uses()
                .iter()
                .any(|usage| usage.feature == feature)
        );
        assert!(
            disabled
                .syntax()
                .descendants()
                .any(|node| node.kind() == kind)
        );
        assert_eq!(enabled(text)?.errors(), []);
    }
    let source = SourceStore::default().open(
        std::path::Path::new("class-export.luau"),
        1,
        "export open class Animal public species: string end",
    )?;
    let parsed = Parse::with_options(
        source,
        ParseOptions {
            recursion_limit: None,
            type_length_limit: None,
            error_limit: None,
            features: [Feature::Classes].into_iter().collect(),
        },
    )?;
    assert_eq!(parsed.errors(), []);
    assert!(
        parsed
            .feature_uses()
            .iter()
            .all(|usage| usage.feature != Feature::ValueExports)
    );
    Ok(())
}

#[test]
fn literal_metadata_vertical_space_and_constant_backticks() -> TestResult {
    let parsed = enabled("local\u{b}x = `constant`; return 42i, 0xffi, 0b101i, '\\z\u{b}x'")?;
    assert!(parsed.errors().is_empty(), "{:?}", parsed.errors());
    assert!(
        !parsed
            .syntax()
            .descendants()
            .any(|node| node.kind() == K::InterpolationExpression)
    );
    assert_eq!(
        number_value("42i").ok_or("integer")?.value,
        NumberValue::Integer(42)
    );
    assert_eq!(
        number_value("0xffffffffffffffffi")
            .ok_or("integer bits")?
            .value,
        NumberValue::Integer(-1)
    );
    assert_eq!(
        number_value("9007199254740993").ok_or("number")?.status,
        NumberStatus::Imprecise
    );
    assert_eq!(
        number_value("0x10000000000000000")
            .ok_or("overflow")?
            .status,
        NumberStatus::Overflow
    );
    assert!(number_value("9223372036854775808i").is_none());
    let tokens: Vec<_> = parsed
        .syntax()
        .descendants_with_tokens()
        .filter_map(rowan::NodeOrToken::into_token)
        .filter(|token| token.kind() == K::String)
        .collect();
    assert_eq!(string_bytes(&parsed, &tokens[0])?, b"constant");
    assert_eq!(string_bytes(&parsed, &tokens[1])?, b"x");
    Ok(())
}

#[test]
fn entry_points_fragments_hotcomments_and_statement_extents() -> TestResult {
    let parsed = parse("--!strict  \nlocal x = 1; --!after\nreturn x")?;
    assert_eq!(parsed.hot_comments().len(), 2);
    assert!(parsed.hot_comments()[0].header);
    assert_eq!(parsed.hot_comments()[0].text, b"strict");
    assert!(!parsed.hot_comments()[1].header);
    assert!(
        parsed
            .syntax()
            .descendants()
            .find(|node| node.kind() == K::LocalStatement)
            .ok_or("local")?
            .text()
            .to_string()
            .ends_with(';')
    );
    let source = SourceStore::default().open(
        std::path::Path::new("fragment.luau"),
        1,
        "prefix 1 + 2 suffix",
    )?;
    let fragment = Parse::fragment(
        source.clone(),
        text_size::TextRange::new(7.into(), 12.into()),
        ParseOptions::default(),
        EntryPoint::Expression,
    )?;
    assert_eq!(fragment.errors(), []);
    assert_eq!(fragment.syntax().text().to_string(), "1 + 2");
    assert_eq!(
        fragment.source_range(fragment.syntax().text_range()),
        fragment.extent()
    );
    assert!(Arc::ptr_eq(fragment.source(), &source));
    let source =
        SourceStore::default().open(std::path::Path::new("type.luau"), 1, "(number) -> string")?;
    assert_eq!(
        Parse::entry(source, ParseOptions::default(), EntryPoint::Type)?.errors(),
        []
    );
    Ok(())
}
