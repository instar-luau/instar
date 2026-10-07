//! Synthetic source, configuration, graph and sourcemap lifecycle fixtures.
use std::{collections::BTreeSet, fs, io, rc::Rc, time::Duration};

use instar_analysis::Options;

use instar_core::{
    native,
    project::Change,
    resolve::{Identity, Request},
};

use crate::support::{Directory, project};

#[test]
fn overlays_and_cached_settings() -> io::Result<()> {
    let directory = Directory::new(None)?;

    let caller = directory.file(
        "caller.luau",
        include_str!("../fixtures/lifecycle/overlays/caller.luau"),
    )?;

    let target = directory.file("target.luau", "return 1")?;

    let aliases = directory.file(
        ".luaurc",
        include_str!("../fixtures/lifecycle/overlays/.luaurc"),
    )?;

    let mut project = project();
    let settings = project.configuration(directory.path())?;

    assert!(Rc::ptr_eq(
        &settings,
        &project.configuration(directory.path())?
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
        &project.configuration(directory.path())?
    ));

    let overlay = project.source(&caller)?;
    assert!(overlay.revision > original.revision);

    assert_eq!(
        &*original.text,
        include_str!("../fixtures/lifecycle/overlays/caller.luau")
    );

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

    assert_eq!(
        &*project.source(&caller)?.text,
        include_str!("../fixtures/lifecycle/overlays/caller.luau")
    );

    Ok(())
}

#[test]
fn missing_ambiguous_and_replaced_targets_invalidate_links() -> io::Result<()> {
    let directory = Directory::new(None)?;

    let caller = directory.file(
        "caller.luau",
        include_str!("../fixtures/lifecycle/missing/caller.luau"),
    )?;

    let mut project = project();
    let missing = project.links(&caller, None)?;
    assert!(missing.sites[0].failure.is_some());
    let target = directory.path().join("target.luau");
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

    let cases: Vec<(String, Vec<Option<String>>)> =
        serde_json::from_str(include_str!("../fixtures/lifecycle/lexical/cases.json"))?;

    assert_eq!(cases.len(), 14);

    let caller = directory.file("caller.luau", "")?;
    let mut project = project();

    for (source, expected) in cases {
        project.change(Change::Overlay {
            path: caller.clone(),
            text: Some(source.clone()),
        })?;

        let node = project.links(&caller, None)?;
        assert!(node.problems.is_empty(), "{source}: {:?}", node.problems);

        let requests = node
            .sites
            .iter()
            .map(|site| match &site.request {
                Some(Request::String(text)) => Some(text.clone()),
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

    let first = directory.file(
        "first.luau",
        include_str!("../fixtures/lifecycle/cycles/first.luau"),
    )?;

    let second = directory.file(
        "second.luau",
        include_str!("../fixtures/lifecycle/cycles/second.luau"),
    )?;

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

    let caller = directory.file(
        "caller.luau",
        include_str!("../fixtures/lifecycle/duplicate/caller.luau"),
    )?;

    let first = directory.file("first.luau", "return 1")?;
    let second = directory.file("second.luau", "return 2")?;

    directory.file(
        "sourcemap.json",
        include_str!("../fixtures/lifecycle/maps/original.json"),
    )?;

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

    let caller = directory.file(
        "caller.luau",
        include_str!("../fixtures/lifecycle/duplicate/caller.luau"),
    )?;

    directory.file("first.luau", "return 1")?;
    let second = directory.file("second.luau", "return 2")?;

    let location = directory.file(
        "sourcemap.json",
        include_str!("../fixtures/lifecycle/maps/original.json"),
    )?;

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
        text: Some(include_str!("../fixtures/lifecycle/maps/moved.json").to_owned()),
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

    let caller = directory.file(
        "nested/caller.luau",
        include_str!("../fixtures/lifecycle/nearest/nested/caller.luau"),
    )?;

    let outer = directory.file(
        "sourcemap.json",
        include_str!("../fixtures/lifecycle/nearest/sourcemap.json"),
    )?;

    let mut project = project();

    assert!(
        matches!(project.module(&caller,None)?.identity, Identity::Instance { map, .. } if map == outer)
    );

    let inner = directory.file(
        "nested/sourcemap.json",
        include_str!("../fixtures/lifecycle/nearest/nested.json"),
    )?;

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

    let manifest = directory.path().join("nested/instar.toml");

    project.change(Change::Overlay {
        path: manifest.clone(),
        text: Some(include_str!("../fixtures/lifecycle/nearest/input.toml").to_owned()),
    })?;

    assert!(matches!(
        project.module(&caller, None)?.identity,
        Identity::Filesystem(_)
    ));

    project.change(Change::Overlay {
        path: manifest.clone(),
        text: Some(include_str!("../fixtures/lifecycle/nearest/replacement.toml").to_owned()),
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
        include_str!("../fixtures/lifecycle/overlapping/caller.luau"),
    )?;

    directory.file("target.luau", "return 1")?;

    let source = include_str!("../fixtures/lifecycle/overlapping/sourcemap.json").to_owned();

    directory.file("one.json", &source)?;
    directory.file("two.json", &source)?;

    directory.file(
        "instar.toml",
        include_str!("../fixtures/lifecycle/overlapping/instar.toml"),
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

    let caller = directory.file(
        "caller.luau",
        include_str!("../fixtures/lifecycle/configuration/caller.luau"),
    )?;

    directory.file("target.luau", "return 1")?;

    directory.file(
        ".luaurc",
        include_str!("../fixtures/lifecycle/overlays/.luaurc"),
    )?;

    let mut project = project();
    let node = project.links(&caller, None)?;
    let executable = directory.path().join(".config.luau");
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
        include_str!("../fixtures/lifecycle/replicated/sourcemap.json"),
    )?;

    let mut project = project();

    let cases: Vec<(String, bool, bool)> =
        serde_json::from_str(include_str!("../fixtures/lifecycle/services/cases.json"))?;

    assert_eq!(cases.len(), 13);

    for (source, known, resolved) in cases {
        project.change(Change::Overlay {
            path: caller.clone(),
            text: Some(source.clone()),
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
        include_str!("../fixtures/lifecycle/native/caller.luau"),
    )?;

    let target = directory.file("target.luau", "return 1")?;
    let mut project = project();
    let node = project.links(&caller, None)?;
    assert_eq!(project.graph().len(), 1);
    let links = project.prepare(&caller, None, &Options::new(Duration::from_secs(5)))?;
    assert_eq!(links.len(), 1);

    assert_eq!(links[0].module, native::name(&node.module.identity));

    assert_eq!(links[0].revision, node.document.revision);
    assert_eq!(links[0].argument, node.sites[0].range);

    assert_eq!(
        links[0].target,
        native::name(&node.sites[0].target.as_ref().expect("Rust target").identity)
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

    let location = directory.file(
        "sourcemap.json",
        include_str!("../fixtures/lifecycle/native/sourcemap.json"),
    )?;

    project.change(Change::Disk(location))?;

    project.change(Change::Overlay {
        path: caller.clone(),
        text: Some(include_str!("../fixtures/lifecycle/native/input.luau").to_owned()),
    })?;

    let links = project.prepare(&caller, None, &Options::new(Duration::from_secs(5)))?;
    assert_eq!(links.len(), 1);
    let node = project.links(&caller, None)?;

    assert_eq!(
        links[0].target,
        native::name(
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

    let caller = directory.file(
        "place/caller.luau",
        include_str!("../fixtures/lifecycle/duplicate/caller.luau"),
    )?;

    let target = directory.file(
        "shared/target.luau",
        include_str!("../fixtures/lifecycle/mapped/shared/target.luau"),
    )?;

    let other = directory.file("shared/other.luau", "return 1")?;

    directory.file(
        "shared/instar.toml",
        include_str!("../fixtures/lifecycle/nearest/input.toml"),
    )?;

    let location = directory.file(
        "place/sourcemap.json",
        include_str!("../fixtures/lifecycle/mapped/sourcemap.json"),
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
    let caller = directory.path().join("virtual/caller.luau");
    let target = directory.path().join("virtual/target.luau");
    let aliases = directory.path().join("virtual/.luaurc");
    let mut project = project();

    project.change(Change::Overlay {
        path: caller.clone(),
        text: Some(include_str!("../fixtures/lifecycle/overlays/caller.luau").to_owned()),
    })?;

    project.change(Change::Overlay {
        path: target.clone(),
        text: Some("return 1".to_owned()),
    })?;

    project.change(Change::Overlay {
        path: aliases,
        text: Some(include_str!("../fixtures/lifecycle/overlays/.luaurc").to_owned()),
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

    let caller = directory.file(
        "caller.luau",
        include_str!("../fixtures/lifecycle/instance/caller.luau"),
    )?;

    directory.file(".config.luau", "return {}")?;

    directory.file(
        "sourcemap.json",
        include_str!("../fixtures/lifecycle/instance/sourcemap.json"),
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

    let caller = directory.file(
        "caller.luau",
        include_str!("../fixtures/lifecycle/file/caller.luau"),
    )?;

    let mut project = project();
    let missing = project.links(&caller, None)?;
    assert!(missing.sites[0].failure.is_some());
    assert!(missing.inputs.contains(&directory.path().join("created")));
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

#[test]
fn object_references_are_dynamic_but_named_children_resolve() -> io::Result<()> {
    let directory = Directory::new(None)?;

    let caller = directory.file(
        "caller.luau",
        include_str!("../fixtures/lifecycle/reference/caller.luau"),
    )?;

    let target = directory.file("target.luau", "return 1")?;

    directory.file(
        "sourcemap.json",
        include_str!("../fixtures/lifecycle/reference/sourcemap.json"),
    )?;

    let mut project = project();
    let node = project.links(&caller, None)?;
    assert_eq!(node.sites.len(), 7);

    for site in &node.sites[..5] {
        assert!(site.request.is_none());
        assert!(site.target.is_none());

        assert_eq!(
            site.failure.as_ref().expect("dynamic request").kind,
            io::ErrorKind::Unsupported
        );
    }

    for site in &node.sites[5..] {
        assert!(site.request.is_some());
        assert!(site.failure.is_none());
        assert_eq!(site.target.as_ref().expect("named child").source, target);
    }

    Ok(())
}
