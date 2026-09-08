use std::{error::Error, fs, path::Path, sync::Arc};

use instar_core::{
    project::Project,
    resolve::{Resolution, ResolveError, Resolver},
    semantics::Semantics,
    source::SourceStore,
    syntax::{EntryPoint, Parse, ParseOptions},
};

type TestResult = Result<(), Box<dyn Error>>;

fn facts(store: &mut SourceStore, path: &Path, text: &str) -> Result<Semantics, Box<dyn Error>> {
    let parse = Parse::new(store.open(path, 1, text)?)?;
    assert!(parse.errors().is_empty(), "{:?}", parse.errors());
    Ok(Semantics::new(parse))
}

#[test]
fn candidates_preserve_ambiguity_missing_and_virtual_overlays() -> TestResult {
    let root = tempfile::tempdir()?;
    let project = Project::load(root.path())?;
    let mut store = SourceStore::default();
    let resolver = Resolver::new(&project, &mut store)?;
    let facts = facts(
        &mut store,
        &root.path().join("main.luau"),
        "return require('./dep'), require('./absent'), require('./folder')",
    )?;
    let path = root.path().join("dep.luau");
    let overlay = store.open(&path, 1, "return 42")?;
    assert!(
        matches!(resolver.resolve(&mut store, &facts, 0)?, Resolution::Resolved(source) if Arc::ptr_eq(&source, &overlay))
    );
    assert!(matches!(
        resolver.resolve(&mut store, &facts, 1)?,
        Resolution::Missing(_)
    ));
    fs::write(root.path().join("dep.lua"), "return 1")?;
    assert!(
        matches!(resolver.resolve(&mut store, &facts, 0)?, Resolution::Ambiguous(paths) if paths.len() == 2)
    );
    let init = store.open(&root.path().join("folder/init.luau"), 1, "return 3")?;
    assert!(
        matches!(resolver.resolve(&mut store, &facts, 2)?, Resolution::Resolved(source) if Arc::ptr_eq(&source, &init))
    );
    Ok(())
}

#[test]
fn file_directory_conflicts_and_init_relative_paths_follow_upstream() -> TestResult {
    let root = tempfile::tempdir()?;
    fs::create_dir(root.path().join("folder"))?;
    fs::write(root.path().join("folder.luau"), "return 1")?;
    fs::write(root.path().join("other.luau"), "return 2")?;
    let project = Project::load(root.path())?;
    let mut store = SourceStore::default();
    let resolver = Resolver::new(&project, &mut store)?;
    let facts = facts(
        &mut store,
        &root.path().join("folder/init.luau"),
        "return require('./other'), require('./folder'), require('./folder/init')",
    )?;
    for site in 0..3 {
        assert!(matches!(
            resolver.resolve(&mut store, &facts, site)?,
            Resolution::Ambiguous(_)
        ));
    }
    fs::remove_file(root.path().join("folder.luau"))?;
    assert!(
        matches!(resolver.resolve(&mut store, &facts, 0)?, Resolution::Resolved(source) if source.path() == root.path().join("other.luau"))
    );
    assert!(
        matches!(resolver.resolve(&mut store, &facts, 1)?, Resolution::Resolved(source) if Arc::ptr_eq(&source, facts.parse().source()))
    );
    assert!(matches!(
        resolver.resolve(&mut store, &facts, 2)?,
        Resolution::Missing(_)
    ));
    Ok(())
}

#[test]
fn upstream_self_fixture_resolves_directly_and_transitively() -> TestResult {
    let root = fs::canonicalize(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../vendor/luau/tests/require/without_config"),
    )?;
    let project = Project::load(&root)?;
    let mut store = SourceStore::default();
    let resolver = Resolver::new(&project, &mut store)?;
    let entry = root.join("nested/init.luau");
    let nested_facts = facts(&mut store, &entry, &fs::read_to_string(&entry)?)?;
    assert!(
        matches!(resolver.resolve(&mut store, &nested_facts, 0)?, Resolution::Resolved(source) if source.path() == root.join("nested/submodule.luau"))
    );

    for (entry, expected) in [
        (
            "nested_module_requirer.luau",
            [
                "nested_module_requirer.luau",
                "nested/init.luau",
                "nested/submodule.luau",
            ],
        ),
        (
            "nested_inits_requirer.luau",
            [
                "nested_inits_requirer.luau",
                "nested_inits/init.luau",
                "nested_inits/init/init.luau",
            ],
        ),
    ] {
        let mut store = SourceStore::default();
        let resolver = Resolver::new(&project, &mut store)?;
        let graph = resolver.graph(&mut store, &[entry.into()])?;
        assert_eq!(graph.modules.len(), expected.len());
        for path in expected {
            assert!(graph.modules.contains_key(&root.join(path)), "{path}");
        }
    }

    let mut store = SourceStore::default();
    let probe_facts = facts(
        &mut store,
        &root.join("fixture-probe.luau"),
        "return require('./dependency'), require('./luau'), require('./lua'), require('./lua_dependency')",
    )?;
    let resolver = Resolver::new(&project, &mut store)?;
    for (site, target) in [
        (0, "dependency.luau"),
        (1, "luau/init.luau"),
        (2, "lua/init.lua"),
        (3, "lua_dependency.lua"),
    ] {
        assert!(
            matches!(resolver.resolve(&mut store, &probe_facts, site)?, Resolution::Resolved(source) if source.path() == root.join(target)),
            "{target}"
        );
    }
    for entry in [
        "ambiguous_file_requirer.luau",
        "ambiguous_directory_requirer.luau",
    ] {
        let text = fs::read_to_string(root.join(entry))?;
        let facts = facts(&mut store, &root.join(entry), &text)?;
        assert!(matches!(
            resolver.resolve(&mut store, &facts, 0)?,
            Resolution::Ambiguous(_)
        ));
    }
    Ok(())
}

#[test]
fn self_is_case_insensitive_self_relative_and_overridable_by_default() -> TestResult {
    let root = tempfile::tempdir()?;
    let project = Project::load(root.path())?;
    let mut store = SourceStore::default();
    let current = store.open(
        &root.path().join("nested/init.luau"),
        1,
        "return require('@SELF/submodule'), require('@self')",
    )?;
    let target = store.open(&root.path().join("nested/submodule.luau"), 1, "return 1")?;
    let facts = Semantics::new(Parse::new(Arc::clone(&current))?);
    let resolver = Resolver::new(&project, &mut store)?;
    assert!(
        matches!(resolver.resolve(&mut store, &facts, 0)?, Resolution::Resolved(source) if Arc::ptr_eq(&source, &target))
    );
    assert!(
        matches!(resolver.resolve(&mut store, &facts, 1)?, Resolution::Resolved(source) if Arc::ptr_eq(&source, &current))
    );

    fs::write(
        root.path().join("instar.toml"),
        "[aliases]\nself = 'override'\n",
    )?;
    let project = Project::load(root.path())?;
    let override_target =
        store.open(&root.path().join("override/submodule.luau"), 1, "return 2")?;
    let resolver = Resolver::new(&project, &mut store)?;
    assert!(
        matches!(resolver.resolve(&mut store, &facts, 0)?, Resolution::Resolved(source) if Arc::ptr_eq(&source, &override_target))
    );
    Ok(())
}

#[test]
fn navigation_validates_each_component_and_normalizes_separators() -> TestResult {
    let root = tempfile::tempdir()?;
    fs::create_dir(root.path().join("dir"))?;
    fs::write(root.path().join("dep.luau"), "return 1")?;
    fs::write(root.path().join("dir/child.luau"), "return 2")?;
    let project = Project::load(root.path())?;
    let mut store = SourceStore::default();
    let navigation_facts = facts(
        &mut store,
        &root.path().join("main.luau"),
        r"return require('././dep'), require('.//dep'), require('./dir\\child'), require('./dir/../dep'), require('./missing/../dep'), require('./.config')",
    )?;
    let resolver = Resolver::new(&project, &mut store)?;
    for site in 0..4 {
        assert!(matches!(
            resolver.resolve(&mut store, &navigation_facts, site)?,
            Resolution::Resolved(_)
        ));
    }
    assert!(matches!(
        resolver.resolve(&mut store, &navigation_facts, 4)?,
        Resolution::Missing(_)
    ));
    assert!(matches!(
        resolver.resolve(&mut store, &navigation_facts, 5)?,
        Resolution::Unsupported(".config is not a require module")
    ));

    fs::create_dir_all(root.path().join("folder/inner"))?;
    fs::write(root.path().join("folder.luau"), "return 1")?;
    fs::write(root.path().join("folder/dep.luau"), "return 2")?;
    let parent_facts = facts(
        &mut store,
        &root.path().join("folder/inner/main.luau"),
        "return require('../dep')",
    )?;
    assert!(
        matches!(resolver.resolve(&mut store, &parent_facts, 0)?, Resolution::Resolved(source) if source.path() == root.path().join("folder/dep.luau"))
    );
    Ok(())
}

#[test]
fn aliases_are_configuration_owned_and_do_not_merge_formats() -> TestResult {
    let root = tempfile::tempdir()?;
    fs::write(
        root.path().join("instar.toml"),
        "[aliases]\nShared = 'lib'\n",
    )?;
    fs::write(
        root.path().join(".luaurc"),
        r#"{"aliases":{"foreign":"elsewhere"}}"#,
    )?;
    let project = Project::load(root.path())?;
    let mut store = SourceStore::default();
    let resolver = Resolver::new(&project, &mut store)?;
    let target = store.open(&root.path().join("lib/dep.luau"), 1, "return 1")?;
    let configured_facts = facts(
        &mut store,
        &root.path().join("nested/main.luau"),
        "return require('@SHARED/dep'), require('@foreign/dep'), require('bare')",
    )?;
    assert!(
        matches!(resolver.resolve(&mut store, &configured_facts, 0)?, Resolution::Resolved(source) if Arc::ptr_eq(&source, &target))
    );
    assert!(matches!(
        resolver.resolve(&mut store, &configured_facts, 1)?,
        Resolution::Unsupported(_)
    ));
    assert!(matches!(
        resolver.resolve(&mut store, &configured_facts, 2)?,
        Resolution::Unsupported(_)
    ));

    let root = tempfile::tempdir()?;
    fs::write(
        root.path().join("instar.toml"),
        "[aliases]\nshared = 'lib'\n",
    )?;
    fs::create_dir(root.path().join("lib"))?;
    fs::write(root.path().join("lib.luau"), "return 1")?;
    fs::write(root.path().join("lib/dep.luau"), "return 2")?;
    let project = Project::load(root.path())?;
    let mut store = SourceStore::default();
    let facts = facts(
        &mut store,
        &root.path().join("main.luau"),
        "return require('@shared/dep')",
    )?;
    let resolver = Resolver::new(&project, &mut store)?;
    assert!(matches!(
        resolver.resolve(&mut store, &facts, 0)?,
        Resolution::Ambiguous(_)
    ));

    for name in [".", ".."] {
        let root = tempfile::tempdir()?;
        fs::write(
            root.path().join("instar.toml"),
            format!("[aliases]\n'{name}' = 'lib'\n"),
        )?;
        let project = Project::load(root.path())?;
        let mut store = SourceStore::default();
        assert!(matches!(
            Resolver::new(&project, &mut store),
            Err(ResolveError::Alias(alias)) if alias == name
        ));
    }
    Ok(())
}

#[test]
fn graph_retains_cycles_revisions_and_unresolved_edges() -> TestResult {
    let root = tempfile::tempdir()?;
    fs::write(root.path().join("instar.toml"), "exclude = ['dep.luau']")?;
    fs::write(root.path().join("dep.luau"), "broken disk contents")?;
    let project = Project::load(root.path())?;
    let mut store = SourceStore::default();
    let entry = store.open(
        &root.path().join("main.luau"),
        1,
        "return require('./dep'), require(dynamic)",
    )?;
    let dep = store.open(&root.path().join("dep.luau"), 1, "return require('./main')")?;
    let resolver = Resolver::new(&project, &mut store)?;
    let graph = resolver.graph(&mut store, &["main.luau".into()])?;
    assert_eq!(graph.modules.len(), 2);
    assert!(!graph.is_complete());
    assert_eq!(graph.configurations.len(), 1);
    assert_eq!(
        graph.modules[dep.path()]
            .semantics
            .parse()
            .source()
            .revision(),
        dep.revision()
    );
    assert!(
        matches!(&graph.modules[dep.path()].dependencies[0].resolution, Resolution::Resolved(source) if Arc::ptr_eq(source, &entry))
    );
    store.update(&entry, 2, "return 0")?;
    assert_eq!(
        graph.modules[entry.path()]
            .semantics
            .parse()
            .source()
            .revision(),
        entry.revision()
    );
    Ok(())
}

#[test]
fn roblox_mapping_uses_project_base_and_lexical_context() -> TestResult {
    let root = tempfile::tempdir()?;
    fs::create_dir(root.path().join("maps"))?;
    fs::write(
        root.path().join("instar.toml"),
        "[roblox]\nproject = 'game/default.project.json'\nsourcemap = 'maps/tree.json'",
    )?;
    fs::write(
        root.path().join("maps/tree.json"),
        r#"{
        "name":"Game", "className":"DataModel", "children":[
            {"name":"Storage", "className":"ReplicatedStorage", "children":[
                {"name":"Main", "className":"ModuleScript", "filePaths":["main.luau"]},
                {"name":"Dep", "className":"ModuleScript", "filePaths":["dep.luau", "dep.meta.json"]}
            ]}
        ]
    }"#,
    )?;
    let project = Project::load(root.path())?;
    let mut store = SourceStore::default();
    let target = store.open(&root.path().join("game/dep.luau"), 1, "return 42")?;
    let facts = facts(
        &mut store,
        &root.path().join("game/main.luau"),
        "local a = require(script.Parent.Dep); local b = require(game:GetService('ReplicatedStorage'):WaitForChild('Dep')); local c = require(script.Parent['Dep']); do local script = {}; require(script.Parent.Dep) end",
    )?;
    let resolver = Resolver::new(&project, &mut store)?;
    for site in 0..3 {
        assert!(
            matches!(resolver.resolve(&mut store, &facts, site)?, Resolution::Resolved(source) if Arc::ptr_eq(&source, &target))
        );
    }
    assert!(matches!(
        resolver.resolve(&mut store, &facts, 3)?,
        Resolution::Dynamic
    ));

    let text = "prefix require(script.Parent.Dep) suffix";
    let source = store.update(facts.parse().source(), 2, text)?;
    let start = text.find("require").ok_or("require start")?;
    let end = start + "require(script.Parent.Dep)".len();
    let parsed = Parse::fragment(
        source,
        text_size::TextRange::new(u32::try_from(start)?.into(), u32::try_from(end)?.into()),
        ParseOptions::default(),
        EntryPoint::Expression,
    )?;
    let fragment = Semantics::new(parsed);
    assert!(
        matches!(resolver.resolve(&mut store, &fragment, 0)?, Resolution::Resolved(source) if Arc::ptr_eq(&source, &target))
    );
    Ok(())
}

#[test]
fn context_dynamic_and_invalid_literal_outcomes_are_explicit() -> TestResult {
    let root = tempfile::tempdir()?;
    let project = Project::load(root.path())?;
    let mut store = SourceStore::default();
    let facts = facts(
        &mut store,
        &root.path().join("main.luau"),
        r"return require(script.Parent.Dep), require(12), require('\xFF'), require('./a\0b')",
    )?;
    let resolver = Resolver::new(&project, &mut store)?;
    assert!(matches!(
        resolver.resolve(&mut store, &facts, 0)?,
        Resolution::ContextRequired
    ));
    assert!(matches!(
        resolver.resolve(&mut store, &facts, 1)?,
        Resolution::Dynamic
    ));
    for site in 2..4 {
        assert!(matches!(
            resolver.resolve(&mut store, &facts, site)?,
            Resolution::Unsupported(_)
        ));
    }
    let malformed = Parse::new(store.open(
        &root.path().join("malformed.luau"),
        1,
        r"return require('\999')",
    )?)?;
    assert_ne!(malformed.errors(), []);
    let malformed = Semantics::new(malformed);
    assert!(!malformed.is_complete());
    assert_eq!(malformed.requires().len(), 0);
    Ok(())
}

#[test]
fn decoded_require_literals_and_incomplete_graphs() -> TestResult {
    let root = tempfile::tempdir()?;
    let project = Project::load(root.path())?;
    let mut store = SourceStore::default();
    let target = store.open(
        &root.path().join("dep.luau"),
        1,
        "declare function unsupported()",
    )?;
    store.open(
        &root.path().join("main.luau"),
        1,
        "return require('\\x2e/dep'), require([=[\ndep]=])",
    )?;
    let resolver = Resolver::new(&project, &mut store)?;
    let graph = resolver.graph(&mut store, &["main.luau".into()])?;
    assert!(graph.modules.contains_key(target.path()));
    assert!(!graph.modules[target.path()].semantics.is_complete());
    assert!(!graph.is_complete());
    Ok(())
}
