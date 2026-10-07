//! Synthetic source, configuration, graph and sourcemap lifecycle fixtures.
use std::{collections::BTreeSet, fs, io, rc::Rc, time::Duration};

use instar_analysis::Options;

use instar_core::{
    project::Change,
    resolve::{Identity, Request},
};

use serde_json::json;

use crate::support::{Directory, project};

const SERVICE_CASES: &[(&str, bool, bool)] = &[
    (
        "require(game:GetService('ReplicatedStorage').Library.Target)",
        true,
        true,
    ),
    (
        "require(game:GetService('ReplicatedStorage'):WaitForChild('Library'):WaitForChild('Target', 1))",
        true,
        true,
    ),
    (
        "require(game:GetService('ReplicatedStorage'):WaitForChild('Library', timeout).Target)",
        true,
        true,
    ),
    (
        "require(game:GetService('ReplicatedStorage'):FindFirstChild('Target', true))",
        true,
        true,
    ),
    (
        "require(game:GetService('ReplicatedStorage'):FindFirstChild('Library', false).Target.Parent.Target)",
        true,
        true,
    ),
    (
        "local storage=game:GetService('ReplicatedStorage')\nrequire(storage['Library'].Target)",
        true,
        true,
    ),
    (
        "require(game:GetService('ReplicatedStorage'):FindFirstChild('Target', false))",
        true,
        false,
    ),
    (
        "require(game:GetService(service).Library.Target)",
        false,
        false,
    ),
    (
        "require(game:GetService('ReplicatedStorage'):FindFirstChild('Target', recursive))",
        false,
        false,
    ),
    (
        "local game={}\nrequire(game:GetService('ReplicatedStorage').Library.Target)",
        false,
        false,
    ),
    (
        "local game=game\nlocal function change() game={} end\nrequire(game:GetService('ReplicatedStorage').Library.Target)",
        false,
        false,
    ),
    (
        "local function f(game) require(game:GetService('ReplicatedStorage').Library.Target) end",
        false,
        false,
    ),
    (
        "game={}\nrequire(game:GetService('ReplicatedStorage').Library.Target)",
        false,
        false,
    ),
];

fn map(first: &str, second: &str) -> String {
    json!({"name":"Place","className":"DataModel","children":[
        {"name":"One","className":"Folder","children":[
            {"name":"Caller","className":"ModuleScript","filePaths":["caller.luau"]},
            {"name":"Target","className":"ModuleScript","filePaths":[first]}
        ]},
        {"name":"Two","className":"Folder","children":[
            {"name":"Caller","className":"ModuleScript","filePaths":["caller.luau"]},
            {"name":"Target","className":"ModuleScript","filePaths":[second]}
        ]}
    ]})
    .to_string()
}

#[test]
fn overlays_and_cached_settings() -> io::Result<()> {
    let directory = Directory::new(None)?;
    let caller = directory.file("caller.luau", "return require('@named')")?;
    let target = directory.file("target.luau", "return 1")?;
    let aliases = directory.file(".luaurc", r#"{"aliases":{"named":"./target"}}"#)?;
    let mut project = project();
    let settings = project.configuration(&directory.path)?;

    assert!(Rc::ptr_eq(
        &settings,
        &project.configuration(&directory.path)?
    ));

    assert_eq!(
        project.resolve_source(&caller, "@named").result?.source,
        target
    );

    let original = project.source(&caller)?;

    project.change(Change::Overlay {
        path: caller.clone(),
        text: Some("return 2".to_owned()),
    })?;

    assert!(Rc::ptr_eq(
        &settings,
        &project.configuration(&directory.path)?
    ));

    let overlay = project.source(&caller)?;
    assert!(overlay.revision > original.revision);
    assert_eq!(&*original.text, "return require('@named')");
    project.change(Change::Disk(caller.clone()))?;
    assert_eq!(project.source(&caller)?.revision, overlay.revision + 1);
    assert_eq!(&*project.source(&caller)?.text, "return 2");

    project.change(Change::Overlay {
        path: aliases.clone(),
        text: Some("{".to_owned()),
    })?;

    assert!(project.resolve_source(&caller, "@named").result.is_err());
    assert!(project.watch_inputs().contains(&aliases));
    project.change(Change::Close(aliases))?;

    assert_eq!(
        project.resolve_source(&caller, "@named").result?.source,
        target
    );

    project.change(Change::Close(caller.clone()))?;
    assert_eq!(&*project.source(&caller)?.text, "return require('@named')");

    Ok(())
}

#[test]
fn missing_ambiguous_and_replaced_targets_invalidate_links() -> io::Result<()> {
    let directory = Directory::new(None)?;
    let caller = directory.file("caller.luau", "return require('./target')")?;
    let mut project = project();
    let missing = project.links(&caller, None)?;
    assert!(missing.sites[0].failure.is_some());
    let target = directory.path.join("target.luau");
    assert!(missing.sites[0].inputs.contains(&target));
    assert_eq!(project.graph().len(), 1);
    directory.file("target.luau", "return 1")?;

    assert!(
        project
            .change(Change::Disk(target.clone()))?
            .contains(&missing.module.identity)
    );

    let resolved = project.links(&caller, None)?;

    assert_eq!(
        resolved.sites[0]
            .target
            .as_ref()
            .expect("resolved target")
            .source,
        target
    );

    assert_eq!(project.graph().len(), 1);
    let alternate = directory.file("target.lua", "return 2")?;
    project.change(Change::Disk(alternate.clone()))?;
    let ambiguous = project.links(&caller, None)?;
    assert!(ambiguous.sites[0].target.is_none());

    assert!(
        ambiguous.sites[0]
            .failure
            .as_ref()
            .expect("ambiguity")
            .message
            .contains("ambiguous module")
    );

    fs::remove_file(&alternate)?;
    project.change(Change::Disk(alternate))?;
    project.discover(&caller, None)?;
    let target_module = project.module(&target, None)?;

    assert!(
        project
            .graph()
            .dependents(&target_module.identity)
            .contains(&resolved.module.identity)
    );

    let replacement = directory.file("replacement.luau", "return 3")?;
    fs::remove_file(&target)?;
    fs::rename(replacement, &target)?;
    let invalidated = project.change(Change::Disk(target.clone()))?;
    assert!(invalidated.contains(&target_module.identity));
    assert!(invalidated.contains(&resolved.module.identity));
    project.discover(&caller, None)?;

    assert_eq!(
        &*project
            .graph()
            .node(&target_module.identity)
            .expect("reloaded target")
            .document
            .text,
        "return 3"
    );

    fs::remove_file(&target)?;
    project.change(Change::Disk(target))?;
    assert!(project.links(&caller, None)?.sites[0].target.is_none());

    Ok(())
}

#[test]
fn lexical_sites_keep_shadowing_mutations_and_revisions() -> io::Result<()> {
    let directory = Directory::new(None)?;
    directory.file("target.luau", "return 1")?;

    let cases: &[(&str, &[Option<&str>])] = &[
        (
            "local loader=require\nloader('./target')",
            &[Some("./target")],
        ),
        ("local require=function() end\nrequire('./target')", &[]),
        (
            "do local require=function() end require('./target') end\nrequire('./target')",
            &[Some("./target")],
        ),
        (
            "local path='./target'\nlocal function change() path..='x' end\nrequire(path)",
            &[None],
        ),
        (
            "local path='./target'\ndo local path='other' path='changed' end\nrequire(path)",
            &[Some("./target")],
        ),
        (
            "local path='./target'\nlocal function change(path) path='other' end\nrequire(path)",
            &[Some("./target")],
        ),
        (
            "function require(value) return value end\nrequire('./target')",
            &[None],
        ),
        (
            "local function f(require, ...: typeof(require('./target'))) end",
            &[],
        ),
        (
            "local function f(...: typeof(require('./target'))) end",
            &[Some("./target")],
        ),
        (
            "repeat local path='./target' require(path) until path",
            &[Some("./target")],
        ),
        (
            "for require in pairs({}) do require('./target') end\nrequire('./target')",
            &[Some("./target")],
        ),
        (
            "local path='./target'\nlocal result=if local path=path then require(path) else require(path)\nrequire(path)",
            &[Some("./target"), Some("./target"), Some("./target")],
        ),
        (
            "require([=[./target]=])\nrequire('./tar'..'get')\nrequire('./tar\\x67et')",
            &[Some("./target"), Some("./target"), Some("./target")],
        ),
        (
            "require(compute())\nrequire()\nrequire('./target', 1)",
            &[None, None, None],
        ),
    ];

    let caller = directory.file("caller.luau", "")?;
    let mut project = project();

    for &(source, expected) in cases {
        project.change(Change::Overlay {
            path: caller.clone(),
            text: Some(source.to_owned()),
        })?;

        let node = project.links(&caller, None)?;
        assert!(node.problems.is_empty(), "{source}: {:?}", node.problems);

        let requests = node
            .sites
            .iter()
            .map(|site| match &site.request {
                Some(Request::String(text)) => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>();

        assert_eq!(requests, expected, "{source}");

        for site in &node.sites {
            assert_eq!(site.revision, node.document.revision);
            assert!(site.range[1] <= node.document.text.len());
            assert_eq!(site.target.is_some(), site.request.is_some(), "{source}");
            assert_eq!(site.failure.is_some(), site.request.is_none(), "{source}");
        }
    }

    Ok(())
}

#[test]
fn cycles_expand_once_and_reverse_edges_invalidate() -> io::Result<()> {
    let directory = Directory::new(None)?;
    let first = directory.file("first.luau", "return require('./second')")?;
    let second = directory.file("second.luau", "return require('./first')")?;
    let mut project = project();
    let identities = project.discover(&first, None)?;
    assert_eq!(identities.len(), 2);
    assert_eq!(project.graph().len(), 2);
    assert_eq!(project.discover(&second, None)?, identities);

    assert_eq!(
        project.change(Change::Overlay {
            path: second,
            text: Some("return 2".to_owned())
        })?,
        identities
    );

    assert!(project.graph().is_empty());

    Ok(())
}

#[test]
fn duplicate_sources_are_distinct_instance_contexts() -> io::Result<()> {
    let directory = Directory::new(None)?;
    let caller = directory.file("caller.luau", "return require(script.Parent.Target)")?;
    let first = directory.file("first.luau", "return 1")?;
    let second = directory.file("second.luau", "return 2")?;
    directory.file("sourcemap.json", &map("first.luau", "second.luau"))?;
    let mut project = project();

    assert!(
        project
            .module(&caller, None)
            .expect_err("ambiguous entry")
            .to_string()
            .contains("multiple instances")
    );

    let contexts = project.contexts(&caller)?;
    assert_eq!(contexts.len(), 2);
    assert_ne!(contexts[0].identity, contexts[1].identity);
    let mut targets = BTreeSet::new();

    for context in contexts {
        let node = project.links(&caller, Some(&context.identity))?;

        targets.insert(
            node.sites[0]
                .target
                .as_ref()
                .expect("instance target")
                .source
                .clone(),
        );

        assert!(matches!(node.module.identity, Identity::Instance { .. }));
    }

    assert_eq!(targets, BTreeSet::from([first, second]));
    assert_eq!(project.graph().len(), 2);

    Ok(())
}

#[test]
fn malformed_deleted_and_moved_maps_never_reuse_targets() -> io::Result<()> {
    let directory = Directory::new(None)?;
    let caller = directory.file("caller.luau", "return require(script.Parent.Target)")?;
    directory.file("first.luau", "return 1")?;
    let second = directory.file("second.luau", "return 2")?;
    let location = directory.file("sourcemap.json", &map("first.luau", "second.luau"))?;
    let mut project = project();
    let original = project.contexts(&caller)?.remove(0);
    let node = project.links(&caller, Some(&original.identity))?;
    assert!(node.sites[0].target.is_some());

    project.change(Change::Overlay {
        path: location.clone(),
        text: Some("{".to_owned()),
    })?;

    assert!(project.graph().is_empty());
    assert!(project.contexts(&caller).is_err());
    assert!(project.watch_inputs().contains(&location));

    project.change(Change::Overlay {
        path: location.clone(),
        text: Some(map("second.luau", "first.luau")),
    })?;

    assert!(project.links(&caller, Some(&original.identity)).is_err());
    let moved = project.contexts(&caller)?.remove(0);
    assert_ne!(moved.identity, original.identity);

    assert_eq!(
        project.links(&caller, Some(&moved.identity))?.sites[0]
            .target
            .as_ref()
            .expect("moved target")
            .source,
        second
    );

    project.change(Change::Overlay {
        path: location,
        text: None,
    })?;

    let unmapped = project.links(&caller, None)?;
    assert!(matches!(unmapped.module.identity, Identity::Filesystem(_)));
    assert!(unmapped.sites[0].target.is_none());
    assert!(unmapped.sites[0].failure.is_some());

    Ok(())
}

#[test]
fn nearest_maps_and_explicit_disabling_follow_configuration_events() -> io::Result<()> {
    let directory = Directory::new(None)?;
    let caller = directory.file("nested/caller.luau", "return require(game.GetService)")?;
    let outer = directory.file("sourcemap.json", &json!({"name":"Outer","className":"DataModel","children":[{"name":"Caller","className":"ModuleScript","filePaths":["nested/caller.luau"]}]}).to_string())?;
    let mut project = project();

    assert!(
        matches!(project.module(&caller,None)?.identity, Identity::Instance { map, .. } if map == outer)
    );

    let inner = directory.file("nested/sourcemap.json", &json!({"name":"Inner","className":"DataModel","children":[{"name":"Caller","className":"ModuleScript","filePaths":["caller.luau"]}]}).to_string())?;
    assert!(project.watch_inputs().contains(&inner));
    project.change(Change::Disk(inner.clone()))?;

    assert!(
        matches!(project.module(&caller,None)?.identity, Identity::Instance { map, .. } if map == inner)
    );

    fs::remove_file(&inner)?;
    project.change(Change::Disk(inner))?;

    assert!(
        matches!(project.module(&caller,None)?.identity, Identity::Instance { map, .. } if map == outer)
    );

    let manifest = directory.path.join("nested/instar.toml");

    project.change(Change::Overlay {
        path: manifest.clone(),
        text: Some("[roblox]\nsourcemaps=[]".to_owned()),
    })?;

    assert!(matches!(
        project.module(&caller, None)?.identity,
        Identity::Filesystem(_)
    ));

    project.change(Change::Overlay {
        path: manifest.clone(),
        text: Some("[roblox]\nenabled=false".to_owned()),
    })?;

    assert!(matches!(
        project.module(&caller, None)?.identity,
        Identity::Filesystem(_)
    ));

    project.change(Change::Close(manifest))?;

    assert!(matches!(
        project.module(&caller, None)?.identity,
        Identity::Instance { .. }
    ));

    Ok(())
}

#[test]
fn overlapping_maps_need_explicit_context_and_nonmodules_fail() -> io::Result<()> {
    let directory = Directory::new(None)?;

    let caller = directory.file(
        "caller.luau",
        "require(game:GetService('Workspace'):WaitForChild('Target'))",
    )?;

    directory.file("target.luau", "return 1")?;

    let source = json!({"name":"Place","className":"DataModel","children":[
        {"name":"Caller","className":"ModuleScript","filePaths":["caller.luau"]},
        {"name":"World","className":"Workspace","children":[{"name":"Target","className":"Script","filePaths":["target.luau"]}]}
    ]}).to_string();

    directory.file("one.json", &source)?;
    directory.file("two.json", &source)?;

    directory.file(
        "instar.toml",
        "[roblox]\nsourcemaps=['one.json','two.json']",
    )?;

    let mut project = project();
    assert!(project.module(&caller, None).is_err());
    let contexts = project.contexts(&caller)?;
    assert_eq!(contexts.len(), 2);

    for context in contexts {
        let node = project.links(&caller, Some(&context.identity))?;
        assert!(node.sites[0].target.is_none());

        assert!(
            node.sites[0]
                .failure
                .as_ref()
                .expect("nonmodule failure")
                .message
                .contains("not a ModuleScript")
        );
    }

    Ok(())
}

#[test]
fn configuration_failures_track_absent_conflicting_candidates() -> io::Result<()> {
    let directory = Directory::new(None)?;
    let caller = directory.file("caller.luau", "require('@named')")?;
    directory.file("target.luau", "return 1")?;
    directory.file(".luaurc", r#"{"aliases":{"named":"./target"}}"#)?;
    let mut project = project();
    let node = project.links(&caller, None)?;
    let executable = directory.path.join(".config.luau");
    assert!(node.inputs.contains(&executable));

    project.change(Change::Overlay {
        path: executable.clone(),
        text: Some("return {}".to_owned()),
    })?;

    assert!(project.links(&caller, None).is_err());
    assert!(project.watch_inputs().contains(&executable));
    project.change(Change::Close(executable))?;
    assert!(project.links(&caller, None)?.sites[0].target.is_some());

    Ok(())
}

#[test]
fn replicated_storage_service_chains_keep_static_and_lexical_context() -> io::Result<()> {
    let directory = Directory::new(None)?;
    let caller = directory.file("caller.luau", "")?;
    let target = directory.file("target.luau", "return 1")?;

    directory.file(
        "sourcemap.json",
        &json!({
            "name":"Place","className":"DataModel","children":[
                {"name":"Caller","className":"ModuleScript","filePaths":["caller.luau"]},
                {"name":"Storage","className":"ReplicatedStorage","children":[
                    {"name":"Library","className":"Folder","children":[
                        {"name":"Target","className":"ModuleScript","filePaths":["target.luau"]}
                    ]}
                ]}
            ]
        })
        .to_string(),
    )?;

    let mut project = project();

    for &(source, known, resolved) in SERVICE_CASES {
        project.change(Change::Overlay {
            path: caller.clone(),
            text: Some(source.to_owned()),
        })?;

        let node = project.links(&caller, None)?;
        assert!(node.problems.is_empty(), "{source}");
        assert_eq!(node.sites.len(), 1, "{source}");
        assert_eq!(node.sites[0].request.is_some(), known, "{source}");
        assert_eq!(node.sites[0].target.is_some(), resolved, "{source}");

        if resolved {
            assert_eq!(
                node.sites[0]
                    .target
                    .as_ref()
                    .expect("service target")
                    .source,
                target
            );
        } else {
            assert!(node.sites[0].failure.is_some(), "{source}");
        }
    }

    Ok(())
}

#[test]
fn native_adapter_uses_rust_results_for_aliases_instances_and_lifecycle() -> io::Result<()> {
    let directory = Directory::new(None)?;

    let caller = directory.file(
        "caller.luau",
        "local loader=require\nloader('./target')\nrequire(compute())",
    )?;

    let target = directory.file("target.luau", "return 1")?;
    let mut project = project();
    let node = project.links(&caller, None)?;
    assert_eq!(project.graph().len(), 1);
    let links = project.prepare(&caller, None, &Options::new(Duration::from_secs(5)))?;
    assert_eq!(links.len(), 1);

    assert_eq!(
        links[0].module,
        instar_core::native::name(&node.module.identity)
    );

    assert_eq!(links[0].revision, node.document.revision);
    assert_eq!(links[0].argument, node.sites[0].range);

    assert_eq!(
        links[0].target,
        instar_core::native::name(&node.sites[0].target.as_ref().expect("Rust target").identity)
    );

    assert_eq!(
        project.prepare(&caller, None, &Options::new(Duration::from_secs(5)))?,
        links
    );

    project.change(Change::Overlay {
        path: target.clone(),
        text: None,
    })?;

    assert_eq!(
        project
            .prepare(&caller, None, &Options::new(Duration::from_secs(5)))?
            .len(),
        0
    );

    project.change(Change::Close(target))?;

    assert_eq!(
        project
            .prepare(&caller, None, &Options::new(Duration::from_secs(5)))?
            .len(),
        1
    );

    let location = directory.file("sourcemap.json", &json!({"name":"Place","className":"DataModel","children":[
        {"name":"Caller","className":"ModuleScript","filePaths":["caller.luau"]},
        {"name":"Storage","className":"ReplicatedStorage","children":[{"name":"Target","className":"ModuleScript","filePaths":["target.luau"]}]}
    ]}).to_string())?;

    project.change(Change::Disk(location))?;

    project.change(Change::Overlay {
        path: caller.clone(),
        text: Some(
            "local storage=game:GetService('ReplicatedStorage')\nrequire(storage.Target)"
                .to_owned(),
        ),
    })?;

    let links = project.prepare(&caller, None, &Options::new(Duration::from_secs(5)))?;
    assert_eq!(links.len(), 1);
    let node = project.links(&caller, None)?;

    assert_eq!(
        links[0].target,
        instar_core::native::name(
            &node.sites[0]
                .target
                .as_ref()
                .expect("instance target")
                .identity
        )
    );

    Ok(())
}

#[test]
fn mapped_sources_outside_map_ancestors_keep_explicit_context() -> io::Result<()> {
    let directory = Directory::new(None)?;
    let caller = directory.file("place/caller.luau", "return require(script.Parent.Target)")?;
    let target = directory.file("shared/target.luau", "return require(script.Parent.Other)")?;
    let other = directory.file("shared/other.luau", "return 1")?;
    directory.file("shared/instar.toml", "[roblox]\nsourcemaps=[]")?;

    let location = directory.file(
        "place/sourcemap.json",
        &json!({
            "name":"Place","className":"DataModel","children":[
                {"name":"Caller","className":"ModuleScript","filePaths":["caller.luau"]},
                {"name":"Target","className":"ModuleScript","filePaths":["../shared/target.luau"]},
                {"name":"Other","className":"ModuleScript","filePaths":["../shared/other.luau"]}
            ]
        })
        .to_string(),
    )?;

    let mut project = project();

    assert!(matches!(
        project.module(&target, None)?.identity,
        Identity::Filesystem(_)
    ));

    let node = project.links(&caller, None)?;

    let mapped = node.sites[0]
        .target
        .as_ref()
        .expect("mapped target")
        .clone();

    assert_eq!(mapped.source, target);
    assert!(matches!(&mapped.identity, Identity::Instance { map, .. } if map == &location));
    let dependency = project.links(&target, Some(&mapped.identity))?;

    assert_eq!(
        dependency.sites[0]
            .target
            .as_ref()
            .expect("mapped sibling")
            .source,
        other
    );

    assert_eq!(project.discover(&caller, None)?.len(), 3);

    assert_eq!(
        project
            .prepare(&caller, None, &Options::new(Duration::from_secs(5)))?
            .len(),
        2
    );

    project.change(Change::Overlay {
        path: location,
        text: Some("{".to_owned()),
    })?;

    assert!(project.links(&target, Some(&mapped.identity)).is_err());
    assert!(project.graph().is_empty());

    Ok(())
}

#[test]
fn overlay_only_sources_and_configuration_need_no_disk_directories() -> io::Result<()> {
    let directory = Directory::new(None)?;
    let caller = directory.path.join("virtual/caller.luau");
    let target = directory.path.join("virtual/target.luau");
    let aliases = directory.path.join("virtual/.luaurc");
    let mut project = project();

    project.change(Change::Overlay {
        path: caller.clone(),
        text: Some("return require('@named')".to_owned()),
    })?;

    project.change(Change::Overlay {
        path: target.clone(),
        text: Some("return 1".to_owned()),
    })?;

    project.change(Change::Overlay {
        path: aliases,
        text: Some(r#"{"aliases":{"named":"./target"}}"#.to_owned()),
    })?;

    assert!(!caller.parent().expect("virtual directory").exists());

    assert_eq!(
        project.links(&caller, None)?.sites[0]
            .target
            .as_ref()
            .expect("overlay target")
            .source,
        target
    );

    assert_eq!(
        project
            .prepare(&caller, None, &Options::new(Duration::from_secs(5)))?
            .len(),
        1
    );

    project.change(Change::Close(caller.clone()))?;
    assert!(project.links(&caller, None).is_err());

    Ok(())
}

#[test]
fn instance_requests_cannot_import_native_configuration_sources() -> io::Result<()> {
    let directory = Directory::new(None)?;
    let caller = directory.file("caller.luau", "require(script.Parent.Configuration)")?;
    directory.file(".config.luau", "return {}")?;

    directory.file(
        "sourcemap.json",
        &json!({"name":"Place","className":"DataModel","children":[
            {"name":"Caller","className":"ModuleScript","filePaths":["caller.luau"]},
            {"name":"Configuration","className":"ModuleScript","filePaths":[".config.luau"]}
        ]})
        .to_string(),
    )?;

    let mut project = project();
    let node = project.links(&caller, None)?;
    assert!(node.sites[0].target.is_none());

    assert!(
        node.sites[0]
            .failure
            .as_ref()
            .expect("reserved native source")
            .message
            .contains("not an importable module")
    );

    Ok(())
}

#[test]
fn file_creation_below_missing_navigation_directories_retries_failures() -> io::Result<()> {
    let directory = Directory::new(None)?;
    let caller = directory.file("caller.luau", "require('./created/nested/target')")?;
    let mut project = project();
    let missing = project.links(&caller, None)?;
    assert!(missing.sites[0].failure.is_some());
    assert!(missing.inputs.contains(&directory.path.join("created")));
    let target = directory.file("created/nested/target.luau", "return 1")?;
    let affected = project.change(Change::Disk(target.clone()))?;
    assert!(affected.contains(&missing.module.identity));
    let resolved = project.links(&caller, None)?;

    assert_eq!(
        resolved.sites[0]
            .target
            .as_ref()
            .expect("created nested target")
            .source,
        target
    );

    Ok(())
}
