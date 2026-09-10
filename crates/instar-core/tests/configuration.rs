use instar_core::format::{Options, format};

fn check(configuration: &str, source: &str, expected: &str) {
    let options: Options = toml_edit::de::from_str(configuration).expect("valid configuration");
    let output = format(source.as_bytes(), &options).expect(source);

    assert_eq!(
        String::from_utf8_lossy(&output),
        expected,
        "{configuration}\n{source}"
    );

    assert_eq!(
        format(&output, &options).expect("second pass"),
        output,
        "idempotence: {configuration}\n{source}"
    );
}

#[test]
fn blocks() {
    check(
        "blocks.collapse = 'always'",
        "local function f() return 1 end\nif x then return 2 end",
        "local function f() return 1 end\nif x then return 2 end\n",
    );

    check(
        "blocks.blank_lines = 'preserve'",
        "do\n\nlocal x=1\n\nend",
        "do\n\n\tlocal x = 1\n\nend\n",
    );

    check(
        "blocks.collapse = 'always'",
        "if x then -- keep\nreturn 2 end",
        "if x then -- keep\n\treturn 2\nend\n",
    );
}

#[test]
fn declarations() {
    check(
        "imports.binding = 'const'",
        "local A=require('a')\nreturn A",
        "const A = require(\"a\")\nreturn A\n",
    );

    check(
        "imports.binding = 'local'",
        "const A=require('a')\nreturn A",
        "local A = require(\"a\")\nreturn A\n",
    );

    check(
        "bindings.prefer_constant = true",
        "local x=1\nx=2\nlocal y=3\nreturn x+y",
        "local x = 1\nx = 2\nconst y = 3\nreturn x + y\n",
    );

    check(
        "bindings.prefer_constant = true",
        "local x=1\ndo local x=2 x=3 end\nreturn x",
        "const x = 1\ndo\n\tlocal x = 2\n\tx = 3\nend\nreturn x\n",
    );

    check(
        "bindings.prefer_constant = true\nbindings.preserve_mutated_tables = true",
        "local t={}\nt.x=1\nlocal v={}\ntable.insert(v,2)",
        "local t = {}\nt.x = 1\nlocal v = {}\ntable.insert(v, 2)\n",
    );

    check(
        "functions.binding = 'const'",
        "function f() return f end",
        "const function f()\n\treturn f\nend\n",
    );

    check(
        "functions.binding = 'global'",
        "local function f() end",
        "function f()\nend\n",
    );

    check(
        "functions.binding = 'local'",
        "function f() end\nfunction t.m() end",
        "local function f()\nend\nfunction t.m()\nend\n",
    );

    check(
        "functions.binding = 'const'",
        "local function f() end\nf=nil",
        "local function f()\nend\nf = nil\n",
    );
}

#[test]
fn imports() {
    check(
        "imports.unused = 'underscore'",
        "local A=require('a')\nreturn 1",
        "local _A = require(\"a\")\nreturn 1\n",
    );

    check(
        "imports.unused = 'remove'",
        "-- keep\nlocal A=require('a') -- tail\nreturn 1",
        "-- keep\n-- tail\nreturn 1\n",
    );

    check(
        "imports.unused = 'remove'",
        "local A=require('a')\ntype T={value:A.Value}\nreturn 1",
        "local A = require(\"a\")\ntype T = { value: A.Value }\nreturn 1\n",
    );

    check(
        "imports.sort = true",
        "local Z=require('z')\nlocal A=require('a')\nreturn Z,A",
        "local A = require(\"a\")\nlocal Z = require(\"z\")\nreturn Z, A\n",
    );

    check(
        "imports.sort = true",
        "--!strict\n-- Z\nlocal Z=require('z') -- tail Z\n-- A\nlocal A=require('a') -- tail A\nreturn Z,A",
        "--!strict\n-- A\nlocal A = require(\"a\") -- tail A\n-- Z\nlocal Z = require(\"z\") -- tail Z\nreturn Z, A\n",
    );

    check(
        "imports.sort = true\nimports.grouping = 'by-kind'",
        "local R=require('./r')\nlocal A=require('@a')\nlocal G=require('game/g')",
        "local A = require(\"@a\")\n\nlocal G = require(\"game/g\")\n\nlocal R = require(\"./r\")\n",
    );
}

#[test]
fn properties() {
    check(
        "tables.order = 'alphabetical'",
        "return {z=1,a=2}",
        "return { a = 2, z = 1 }\n",
    );

    check(
        "types.tables.order = 'key-length-ascending'",
        "type T={long:number,a:string}",
        "type T = { a: string, long: number }\n",
    );

    check(
        "types.tables.order = 'alphabetical'\ntypes.tables.indexer = 'last'",
        "type T={[number]:boolean,z:number,a:string}",
        "type T = { a: string, z: number, [number]: boolean }\n",
    );

    check(
        "tables.order = 'alphabetical'",
        "return {z={b=1,a=2},a=3}",
        "return { a = 3, z = { a = 2, b = 1 } }\n",
    );

    check(
        "tables.order = 'alphabetical'",
        "return {z=1,2,a=3}",
        "return { z = 1, 2, a = 3 }\n",
    );
}

#[test]
fn chains() {
    check(
        "calls.chains.style = 'method'",
        "return map.new():some():other()",
        "return map.new()\n\t:some()\n\t:other()\n",
    );

    check(
        "calls.chains.style = 'full'",
        "return map.new():some():other()",
        "return map\n\t.new()\n\t:some()\n\t:other()\n",
    );

    check(
        "types.operators.expand = 'always'",
        "type U=Alpha|Beta|Gamma",
        "type U =\n\t| Alpha\n\t| Beta\n\t| Gamma\n",
    );

    check(
        "types.operators.expand = 'never'",
        "type U=|Alpha|Beta",
        "type U = Alpha | Beta\n",
    );

    check(
        "types.operators.expand = 'always'",
        "type U=Map<Alpha|Beta,Gamma>",
        "type U = Map<Alpha | Beta, Gamma>\n",
    );
}

#[test]
fn configuration_preserves_selection_paths() {
    let directory = tempfile::tempdir().unwrap();
    std::fs::write(directory.path().join("instar.toml"), "[format]\nindentation.style = 'spaces'\nquotes = 'prefer-single'\nexclude = ['Generated']\n[format.calls]\nexpand = 'always'").unwrap();

    let configuration = instar_core::format::configuration::Configuration::discover(
        &directory.path().join("source.luau"),
        None,
    )
    .unwrap();

    assert_eq!(
        configuration.format(b"f(first,second)").unwrap(),
        b"f(\n    first,\n    second\n)\n"
    );

    assert!(
        !configuration
            .selection
            .includes(&directory.path().join("Generated/source.luau"))
            .unwrap()
    );
}

#[test]
fn binding_safety_covers_annotations_and_method_receivers() {
    check(
        "imports.unused = 'remove'",
        "local P=require('p')\ntype T<A=P.Value> = A",
        "local P = require(\"p\")\ntype T<A = P.Value> = A\n",
    );

    check(
        "imports.unused = 'remove'",
        "local P=require('p')\nlocal function f(...:P.Value) end\nreturn f",
        "local P = require(\"p\")\nlocal function f(...: P.Value)\nend\nreturn f\n",
    );

    check(
        "imports.unused = 'remove'\nimports.binding = 'local'",
        "const P=require('p')",
        "local P = require(\"p\")\n",
    );

    check(
        "imports.unused = 'remove'",
        "local A=require('a')\nlocal A,x:A.Value=1,nil\nreturn x",
        "local A = require(\"a\")\nlocal A, x: A.Value = 1, nil\nreturn x\n",
    );

    check(
        "imports.unused = 'remove'",
        "local self=require('a')\nfunction t:read() return self end",
        "function t:read()\n\treturn self\nend\n",
    );

    check(
        "imports.unused = 'remove'",
        "local Dead=require(-- keep\n'a')\nreturn 1",
        "-- keep\nreturn 1\n",
    );
}

#[test]
fn comments() {
    check(
        "",
        "local t={-- keep\na=1,b=2}",
        "local t = {\n\t-- keep\n\ta = 1,\n\tb = 2,\n}\n",
    );

    check("", "f(a,-- keep\nb)", "f(\n\ta, -- keep\n\tb\n)\n");

    check(
        "",
        "do -- open\n-- before\n\nlocal x=1 -- after\n-- close\nend",
        "do -- open\n\t-- before\n\n\tlocal x = 1 -- after\n\t-- close\nend\n",
    );
}
