use std::{error::Error, path::Path};

use instar_core::{
    semantics::{Access, DeclarationKind, Namespace, Semantics},
    source::SourceStore,
    syntax::{EntryPoint, Feature, Parse, ParseOptions},
};

type TestResult = Result<(), Box<dyn Error>>;

fn analyze(text: &str) -> Result<Semantics, Box<dyn Error>> {
    let source = SourceStore::default().open(Path::new("semantics.luau"), 1, text)?;
    let parse = Parse::new(source)?;
    assert!(parse.errors().is_empty(), "{:?}", parse.errors());
    Ok(Semantics::new(parse))
}

#[test]
fn initializer_visibility_shadowing_and_capture_chains() -> TestResult {
    let facts = analyze(
        "local x = x; local function f() local x = x; return function() return x, f end end",
    )?;
    let xs: Vec<_> = facts
        .references()
        .iter()
        .filter(|r| r.name == "x")
        .collect();
    assert!(xs[0].declaration.is_none());
    let outer = xs[1].declaration.ok_or("missing outer x")?;
    let inner = xs[2].declaration.ok_or("missing inner x")?;
    assert_ne!(outer, inner);
    assert_eq!(
        facts
            .captures()
            .iter()
            .filter(|c| c.declaration == outer)
            .count(),
        1
    );
    assert_eq!(
        facts
            .captures()
            .iter()
            .filter(|c| c.declaration == inner)
            .count(),
        1
    );
    let f = facts
        .references()
        .iter()
        .find(|r| r.name == "f")
        .and_then(|r| r.declaration)
        .ok_or("missing recursive f")?;
    assert_eq!(facts.declarations()[f].kind, DeclarationKind::LocalFunction);
    assert_eq!(
        facts
            .captures()
            .iter()
            .filter(|c| c.declaration == f)
            .count(),
        2
    );
    Ok(())
}

#[test]
fn loops_and_repeat_have_correct_visibility() -> TestResult {
    let facts = analyze(
        "local i = 4; for i = i, 8 do print(i) end; repeat local stop = i until stop; print(stop, i)",
    )?;
    let is: Vec<_> = facts
        .references()
        .iter()
        .filter(|r| r.name == "i")
        .collect();
    assert_ne!(is[0].declaration, is[1].declaration);
    assert_eq!(
        facts.declarations()[is[1].declaration.ok_or("loop binding")?].kind,
        DeclarationKind::Loop
    );
    assert_eq!(is[0].declaration, is[2].declaration);
    assert_eq!(is[0].declaration, is[3].declaration);
    let stops: Vec<_> = facts
        .references()
        .iter()
        .filter(|r| r.name == "stop")
        .collect();
    assert!(stops[0].declaration.is_some());
    assert!(stops[1].declaration.is_none());
    Ok(())
}

#[test]
fn fields_types_methods_and_assignment_access_are_distinct() -> TestResult {
    let facts = analyze(
        "local t = {}; type T = number; function t:m(p: T) self.x = p; p += 1; return {p = p, [p] = t.x} end",
    )?;
    assert!(
        !facts
            .references()
            .iter()
            .any(|r| matches!(r.name.as_str(), "m" | "x"))
    );
    let self_ref = facts
        .references()
        .iter()
        .find(|r| r.name == "self")
        .ok_or("self reference")?;
    assert_eq!(
        facts.declarations()[self_ref.declaration.ok_or("self binding")?].kind,
        DeclarationKind::ImplicitSelf
    );
    assert!(
        facts
            .references()
            .iter()
            .any(|r| r.name == "p" && r.access == Access::ReadWrite)
    );
    let ty = facts
        .references()
        .iter()
        .find(|r| r.name == "T")
        .ok_or("type reference")?;
    assert_eq!(ty.namespace, Namespace::Type);
    assert_eq!(
        facts.declarations()[ty.declaration.ok_or("type binding")?].kind,
        DeclarationKind::TypeAlias
    );
    Ok(())
}

#[test]
fn require_sites_respect_lexical_shadowing_and_retain_arguments() -> TestResult {
    let facts = analyze(
        "local x = require('./a'); do local require = require; require('./ignored') end; local function f(require) return require('./ignored') end; return require(script.Parent.Mod), require(dynamic), require(), require('a', 'b')",
    )?;
    assert_eq!(facts.requires().len(), 5);
    assert!(facts.requires()[0].argument.is_some());
    assert!(facts.requires()[1].argument.is_some());
    assert!(facts.requires()[2].argument.is_some());
    assert!(facts.requires()[3].argument.is_none());
    assert!(facts.requires()[4].argument.is_none());
    Ok(())
}

#[test]
fn function_signatures_precede_value_bindings_and_parenthesized_require_is_visible() -> TestResult {
    let facts = analyze(
        "local function require(x: typeof(require('./types'))): typeof(x) return require('./shadowed') end; local function f() end",
    )?;
    assert_eq!(facts.requires().len(), 1);
    let x = facts
        .references()
        .iter()
        .find(|r| r.name == "x")
        .ok_or("signature x")?;
    assert!(x.declaration.is_none());
    let facts = analyze("return (require)('./dep')")?;
    assert_eq!(facts.requires().len(), 1);
    Ok(())
}

#[test]
fn malformed_regions_are_incomplete_and_snapshots_remain_owned() -> TestResult {
    let mut store = SourceStore::default();
    let source = store.open(
        Path::new("incomplete.luau"),
        1,
        "local x = 1; local broken = ; local after = 2",
    )?;
    let revision = source.revision();
    let facts = Semantics::new(Parse::new(source.clone())?);
    store.update(&source, 2, "return require('./new')")?;
    assert!(!facts.is_complete());
    assert_eq!(facts.parse().source().revision(), revision);
    assert_eq!(facts.declarations().len(), 2);
    assert!(facts.requires().is_empty());
    Ok(())
}

fn enabled(text: &str) -> Result<Semantics, Box<dyn Error>> {
    let source = SourceStore::default().open(Path::new("semantics.luau"), 1, text)?;
    let parse = Parse::with_options(
        source,
        ParseOptions {
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
    )?;
    assert!(parse.errors().is_empty(), "{text}\n{:?}", parse.errors());
    Ok(Semantics::new(parse))
}

#[test]
fn constants_are_binding_owned_and_fields_remain_mutable() -> TestResult {
    let facts = enabled(
        "const x = {}; x.field = 1; x[1] += 2; local function f() x = {} end; const function g() end; function g() end",
    )?;
    assert_eq!(facts.errors().len(), 2, "{:?}", facts.errors());
    assert!(
        facts
            .declarations()
            .iter()
            .filter(|(_, declaration)| matches!(declaration.name.as_str(), "x" | "g"))
            .all(|(_, declaration)| declaration.is_const)
    );
    assert_eq!(facts.captures().len(), 1);
    assert!(!facts.is_complete());
    Ok(())
}

#[test]
fn conditional_bindings_have_branch_local_visibility() -> TestResult {
    let facts = enabled(
        "local x = 1; if const x = x then local function f() return x end; x = 2 elseif local y = x then print(y) else print(x, y) end; print(x, y)",
    )?;
    let xs: Vec<_> = facts
        .declarations()
        .iter()
        .filter(|(_, declaration)| declaration.name == "x")
        .map(|(id, _)| id)
        .collect();
    assert_eq!(xs.len(), 2);
    let reads: Vec<_> = facts
        .references()
        .iter()
        .filter(|reference| reference.name == "x")
        .collect();
    assert_eq!(reads[0].declaration, Some(xs[0]));
    assert_eq!(reads[1].declaration, Some(xs[1]));
    assert_eq!(reads[3].declaration, Some(xs[0]));
    assert_eq!(facts.errors().len(), 1);
    assert!(
        facts
            .references()
            .iter()
            .rfind(|reference| reference.name == "y")
            .ok_or("y")?
            .declaration
            .is_none()
    );
    Ok(())
}

#[test]
fn type_functions_packs_and_defaults_keep_namespaces_separate() -> TestResult {
    let facts = enabled(
        "local runtime = 1; type function make(t) local own = t; type function nested(x) return x end; return runtime, own, nested(t) end; type F<T = number, P... = (T)> = (T, P...) -> (T, P...); type R = make<string>",
    )?;
    assert_eq!(facts.errors().len(), 1, "{:?}", facts.errors());
    assert!(
        facts
            .declarations()
            .iter()
            .any(
                |(_, declaration)| declaration.kind == DeclarationKind::TypeFunction
                    && declaration.namespace == Namespace::Type
            )
    );
    let pack = facts
        .declarations()
        .iter()
        .find(|(_, declaration)| declaration.kind == DeclarationKind::TypePackParameter)
        .ok_or("pack")?
        .0;
    assert!(
        facts
            .references()
            .iter()
            .filter(|reference| reference.name == "P")
            .all(|reference| reference.declaration == Some(pack))
    );
    assert!(
        facts
            .references()
            .iter()
            .any(|reference| reference.name == "make" && reference.declaration.is_some())
    );
    let facts = enabled(
        "type function make(t) require('./compile-time') return t end; return require('./runtime')",
    )?;
    assert_eq!(facts.requires().len(), 1);
    Ok(())
}

#[test]
fn classes_and_ambient_declarations_expose_their_actual_namespaces() -> TestResult {
    let facts = enabled(
        "declare value: number; declare function global(x: number): string; declare extern type Base with function get(self): number end; class C extends Base public field: number function get(self) return value end end; local instance: C = C.new(); global(value)",
    )?;
    assert!(facts.is_complete(), "{:?}", facts.errors());
    assert_eq!(
        facts
            .declarations()
            .iter()
            .filter(|(_, declaration)| declaration.name == "C")
            .count(),
        2
    );
    assert!(
        facts
            .references()
            .iter()
            .filter(|reference| matches!(reference.name.as_str(), "value" | "global" | "C"))
            .all(|reference| reference.declaration.is_some())
    );
    assert!(
        !facts.captures().iter().any(
            |capture| facts.declarations()[capture.declaration].kind == DeclarationKind::Global
        )
    );
    Ok(())
}

#[test]
fn class_and_attribute_validations_are_explicit() -> TestResult {
    for text in [
        "class C public x public x end",
        "class C public new end",
        "class C function __index(self) end end",
        "class C function method(self: C) end end",
        "class C end; class C end",
        "declare extern type C with function method(x: number): number end",
        "declare function global(self): number",
        "@[native(value)] function f() end",
        "@[native({[1] = 'x'})] function f() end",
        "@[deprecated('text')] function f() end",
        "@[deprecated({unknown = 'x'})] function f() end",
        "@[deprecated({use = 1})] function f() end",
    ] {
        let facts = enabled(text)?;
        assert!(!facts.errors().is_empty(), "accepted: {text}");
    }
    assert!(enabled("@[deprecated({use = 'g', reason = 'renamed'}), native({x = 1, {true}})] function f() end")?.is_complete());
    Ok(())
}

#[test]
fn exports_detect_duplicate_values_and_return_conflicts() -> TestResult {
    for text in [
        "export const x = 1; export local x = 2",
        "export local x = 1; return x",
        "do return 1 end; export const x = 2",
    ] {
        let facts = enabled(text)?;
        assert!(!facts.errors().is_empty(), "accepted: {text}");
    }
    let facts =
        enabled("export function f() return 1 end; export class C public x end; f = 1; C = 2")?;
    assert_eq!(facts.errors().len(), 2);
    assert_eq!(
        facts
            .declarations()
            .iter()
            .filter(|(_, declaration)| declaration.is_exported)
            .count(),
        3
    );
    assert!(enabled("export type T = number; return 1")?.is_complete());
    Ok(())
}

#[test]
fn instantiated_require_and_fragment_facts_preserve_source_coordinates() -> TestResult {
    assert_eq!(
        enabled("return require<<string>>('./module')")?
            .requires()
            .len(),
        1
    );
    let source = SourceStore::default().open(
        Path::new("fragment.luau"),
        1,
        "prefix require('./module') suffix",
    )?;
    let parsed = Parse::fragment(
        source,
        text_size::TextRange::new(7.into(), 26.into()),
        ParseOptions::default(),
        EntryPoint::Expression,
    )?;
    let facts = Semantics::new(parsed);
    assert!(facts.is_complete(), "{:?}", facts.parse().errors());
    assert_eq!(u32::from(facts.requires()[0].range.start()), 7);
    assert_eq!(u32::from(facts.references()[0].range.start()), 7);
    Ok(())
}
