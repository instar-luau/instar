//! Synthetic source, configuration, graph and sourcemap lifecycle fixtures.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs, io,
    path::{Path, PathBuf},
};

use instar_luau::{FlagValue, ImportOutcome, ImportRequest, ModuleIdentity};

use instar_project::{
    ChangeBatch, CheckOverrides, CheckReport, CheckRequest, CheckingMode, CheckingPatch,
    ExplanationSelection, ImportExplanation, Input, InputIdentity, InputState, InputVersion,
    Invocation, LanguagePatch, PositionQuery, PositionQueryKind, Project, ProjectChange, Query,
    QueryRequest, SourceChange, SourceOrigin,
};

use instar_shared::{ContextIdentity, DiagnosticCategory};
use serde::Deserialize;

use crate::support::{Directory, complete, options, project, replace};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LexicalCase {
    source: String,
    requests: Vec<Option<String>>,

    #[serde(default)]
    flags: BTreeMap<String, FlagValue>,

    mode: Option<CheckingMode>,
}

fn analyze(
    project: &Project,
    path: &Path,
    module: Option<ModuleIdentity>,
) -> io::Result<CheckReport> {
    complete(project.check(
        CheckRequest {
            invocation: Invocation {
                inputs: vec![module.map_or_else(|| Input::Path(path.to_path_buf()), Input::Module)],
                explanations: ExplanationSelection {
                    selection: true,
                    settings: true,
                    dependencies: true,
                },
                ..Invocation::default()
            },
            ..CheckRequest::default()
        },
        options(),
    ))
    .map_err(io::Error::from)
}

fn sites<'report>(report: &'report CheckReport, path: &Path) -> Vec<&'report ImportExplanation> {
    report
        .imports
        .iter()
        .filter(|site| {
            report
                .snapshot
                .origin(site.importer.source)
                .and_then(SourceOrigin::path)
                == Some(path)
        })
        .collect()
}

fn target(report: &CheckReport, site: &ImportExplanation) -> Option<PathBuf> {
    let ImportOutcome::Module(module) = site.outcome else {
        return None;
    };

    report
        .snapshot
        .origin(module.source)
        .and_then(SourceOrigin::path)
        .map(Path::to_path_buf)
}

fn contexts(report: &CheckReport, path: &Path) -> Vec<ModuleIdentity> {
    report
        .modules
        .iter()
        .copied()
        .filter(|module| {
            report
                .snapshot
                .origin(module.source)
                .and_then(SourceOrigin::path)
                == Some(path)
        })
        .collect()
}

fn consulted(report: &CheckReport, path: &Path) -> bool {
    report.inputs.iter().any(|input| match &input.identity {
        InputIdentity::Path(candidate) | InputIdentity::Directory(candidate) => candidate == path,

        InputIdentity::Source(identity)
        | InputIdentity::Configuration(identity)
        | InputIdentity::Sourcemap(identity) => {
            report
                .snapshot
                .origin(*identity)
                .and_then(SourceOrigin::path)
                == Some(path)
        }

        _ => false,
    })
}

fn invalidate(project: &Project, path: &Path) -> io::Result<()> {
    let identity = InputIdentity::Path(path.to_path_buf());
    let expected = project.snapshot().input_versions.get(&identity).cloned();

    let revision = expected.as_ref().map_or(1, |input| {
        input.revision + u64::from(input.state != InputState::Stale)
    });

    project.change(ChangeBatch {
        changes: vec![ProjectChange::Input {
            expected,
            current: InputVersion {
                identity,
                revision,
                state: InputState::Stale,
            },
        }],
    })?;

    Ok(())
}

fn close(project: &Project, path: &Path) -> io::Result<()> {
    let expected = project
        .snapshot()
        .source_at(path)
        .expect("open source")
        .version;

    project.change(ChangeBatch {
        changes: vec![ProjectChange::Source(SourceChange::Close { expected })],
    })?;

    Ok(())
}

fn ambiguous(project: &Project, report: &CheckReport, path: &Path) -> bool {
    let source = report
        .snapshot
        .source_at(path)
        .expect("query source")
        .version;

    let result = complete(project.query(
        QueryRequest {
            snapshot: report.snapshot.clone(),
            operation: Query::Inspect(PositionQuery {
                module: ModuleIdentity {
                    context: ContextIdentity::new(0),
                    source: source.identity,
                },
                source,
                offset: 0,
                kind: PositionQueryKind::Hover,
            }),
        },
        options(),
    ));

    result
        .err()
        .is_some_and(|failure| failure.message.contains("ambiguous"))
}

#[test]
fn overlays_and_cached_settings() -> io::Result<()> {
    let directory = Directory::new(None)?;

    let caller = directory.file(
        "caller.luau",
        include_str!("../fixtures/lifecycle/overlays/caller.luau"),
    )?;

    let target_path = directory.file("target.luau", "return 1")?;

    let aliases = directory.file(
        ".luaurc",
        include_str!("../fixtures/lifecycle/overlays/.luaurc"),
    )?;

    let project = project();
    let original = analyze(&project, &caller, None)?;
    let repeated = analyze(&project, &caller, None)?;
    assert_eq!(original.inputs, repeated.inputs);

    assert_eq!(
        serde_json::to_value(&original.settings).map_err(io::Error::other)?,
        serde_json::to_value(&repeated.settings).map_err(io::Error::other)?
    );

    assert_eq!(
        target(&original, sites(&original, &caller)[0]),
        Some(target_path.clone())
    );

    let original_source = original
        .snapshot
        .source_at(&caller)
        .expect("source")
        .clone();

    replace(&project, &caller, "return 2")?;
    let overlay = analyze(&project, &caller, None)?;

    assert_eq!(
        serde_json::to_value(&original.settings[0].settings).map_err(io::Error::other)?,
        serde_json::to_value(&overlay.settings[0].settings).map_err(io::Error::other)?
    );

    let overlay_source = overlay.snapshot.source_at(&caller).expect("source").clone();
    assert!(overlay_source.version.revision > original_source.version.revision);

    assert_eq!(
        original_source.text.as_ref(),
        include_str!("../fixtures/lifecycle/overlays/caller.luau")
    );

    invalidate(&project, &caller)?;
    let disk_event = analyze(&project, &caller, None)?;

    assert_eq!(
        disk_event
            .snapshot
            .source_at(&caller)
            .expect("overlay")
            .version,
        overlay_source.version
    );

    assert_eq!(
        disk_event
            .snapshot
            .source_at(&caller)
            .expect("overlay")
            .text
            .as_ref(),
        "return 2"
    );

    replace(&project, &aliases, "{")?;
    assert!(analyze(&project, &caller, None).is_err());
    assert!(!project.current(&original.inputs));
    close(&project, &aliases)?;
    close(&project, &caller)?;
    let restored = analyze(&project, &caller, None)?;

    assert_eq!(
        target(&restored, sites(&restored, &caller)[0]),
        Some(target_path)
    );

    assert_eq!(
        restored
            .snapshot
            .source_at(&caller)
            .expect("disk source")
            .text
            .as_ref(),
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

    let project = project();
    let missing = analyze(&project, &caller, None)?;
    assert!(sites(&missing, &caller)[0].failure.is_some());
    let target_path = directory.path().join("target.luau");
    assert!(consulted(&missing, &target_path));
    assert_eq!(missing.modules.len(), 1);
    directory.file("target.luau", "return 1")?;
    invalidate(&project, &target_path)?;
    assert!(!project.current(&missing.inputs));
    let resolved = analyze(&project, &caller, None)?;

    assert_eq!(
        target(&resolved, sites(&resolved, &caller)[0]),
        Some(target_path.clone())
    );

    assert_eq!(sites(&resolved, &caller).len(), 1);
    let alternate = directory.file("target.lua", "return 2")?;
    invalidate(&project, &alternate)?;
    let ambiguous = analyze(&project, &caller, None)?;
    assert!(target(&ambiguous, sites(&ambiguous, &caller)[0]).is_none());

    assert!(
        sites(&ambiguous, &caller)[0]
            .failure
            .as_ref()
            .expect("ambiguity")
            .message
            .contains("ambiguous module")
    );

    fs::remove_file(&alternate)?;
    invalidate(&project, &alternate)?;
    let restored = analyze(&project, &caller, None)?;
    let caller_module = contexts(&restored, &caller)[0];
    let target_module = contexts(&restored, &target_path)[0];

    assert!(
        restored
            .dependencies
            .iter()
            .any(|edge| edge.importer == caller_module && edge.target == target_module)
    );

    let replacement = directory.file("replacement.luau", "return 3")?;
    fs::remove_file(&target_path)?;
    fs::rename(replacement, &target_path)?;
    invalidate(&project, &target_path)?;
    assert!(!project.current(&restored.inputs));
    let replaced = analyze(&project, &caller, None)?;

    assert_eq!(
        replaced
            .snapshot
            .source(target_module.source)
            .expect("reloaded target")
            .text
            .as_ref(),
        "return 3"
    );

    assert!(
        replaced
            .snapshot
            .source(target_module.source)
            .expect("reloaded target")
            .version
            .revision
            > restored
                .snapshot
                .source(target_module.source)
                .expect("old target")
                .version
                .revision
    );

    fs::remove_file(&target_path)?;
    invalidate(&project, &target_path)?;
    let removed = analyze(&project, &caller, None)?;
    assert!(target(&removed, sites(&removed, &caller)[0]).is_none());

    Ok(())
}

#[test]
fn lexical_sites_keep_shadowing_mutations_and_revisions() -> io::Result<()> {
    let directory = Directory::new(None)?;
    directory.file("target.luau", "return 1")?;

    let cases: Vec<LexicalCase> =
        serde_json::from_str(include_str!("../fixtures/lifecycle/lexical/cases.json"))?;

    assert_eq!(cases.len(), 14);
    let caller = directory.file("caller.luau", "")?;
    let project = project();

    for LexicalCase {
        source,
        requests: expected,
        flags,
        mode,
    } in cases
    {
        replace(&project, &caller, &source)?;

        let report = complete(project.check(
            CheckRequest {
                invocation: Invocation {
                    inputs: vec![Input::Path(caller.clone())],
                    explanations: ExplanationSelection {
                        selection: true,
                        settings: true,
                        dependencies: true,
                    },
                    ..Invocation::default()
                },
                overrides: CheckOverrides {
                    checking: CheckingPatch {
                        mode,
                        ..CheckingPatch::default()
                    },
                    language: LanguagePatch {
                        flags,
                        ..LanguagePatch::default()
                    },
                    ..CheckOverrides::default()
                },
            },
            options(),
        ))
        .map_err(io::Error::from)?;

        assert!(
            !report
                .findings
                .iter()
                .any(|finding| finding.category == DiagnosticCategory::Syntax),
            "{source}: {:?}",
            report.findings
        );

        let sites = sites(&report, &caller);

        let requests = sites
            .iter()
            .map(|site| match &site.site.request {
                ImportRequest::Static(text) => Some(text.clone()),
                _ => None,
            })
            .collect::<Vec<_>>();

        assert_eq!(requests, expected, "{source}");
        let document = report.snapshot.source_at(&caller).expect("caller");

        for site in sites {
            assert_eq!(site.site.argument.source, document.version);
            assert!(site.site.argument.span.end <= document.text.len());
            let constant = matches!(site.site.request, ImportRequest::Static(_));
            assert_eq!(target(&report, site).is_some(), constant, "{source}");
            assert!(site.failure.is_none(), "{source}");

            assert_eq!(
                matches!(site.outcome, ImportOutcome::Nonconstant),
                !constant,
                "{source}"
            );
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

    let project = project();
    let original = analyze(&project, &first, None)?;
    assert_eq!(original.modules.len(), 2);
    let reverse = analyze(&project, &second, None)?;
    assert_eq!(original.modules, reverse.modules);
    assert_eq!(original.dependencies.len(), 2);
    replace(&project, &second, "return 2")?;
    assert!(!project.current(&original.inputs));
    assert!(!project.current(&reverse.inputs));
    let changed = analyze(&project, &first, None)?;
    assert_eq!(changed.modules, original.modules);
    assert_eq!(changed.dependencies.len(), 1);

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

    let project = project();
    let original = analyze(&project, &caller, None)?;
    assert!(ambiguous(&project, &original, &caller));
    let contexts = contexts(&original, &caller);
    assert_eq!(contexts.len(), 2);
    assert_ne!(contexts[0], contexts[1]);
    let mut targets = BTreeSet::new();

    for context in contexts {
        let report = analyze(&project, &caller, Some(context))?;
        let site = sites(&report, &caller)[0];
        assert_eq!(site.importer, context);
        assert_ne!(context.context, report.snapshot.context(context.source));
        targets.insert(target(&report, site).expect("instance target"));
    }

    assert_eq!(targets, BTreeSet::from([first, second]));
    assert_eq!(sites(&original, &caller).len(), 2);

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

    let project = project();
    let original = analyze(&project, &caller, None)?;
    let context = contexts(&original, &caller)[0];
    assert!(target(&original, sites(&original, &caller)[0]).is_some());
    replace(&project, &location, "{")?;
    assert!(!project.current(&original.inputs));
    assert!(analyze(&project, &caller, None).is_err());
    assert!(consulted(&original, &location));

    replace(
        &project,
        &location,
        include_str!("../fixtures/lifecycle/maps/moved.json"),
    )?;

    assert!(analyze(&project, &caller, Some(context)).is_err());
    let moved = analyze(&project, &caller, None)?;
    let moved_context = contexts(&moved, &caller)[0];
    assert_ne!(moved_context, context);
    assert_eq!(target(&moved, sites(&moved, &caller)[0]), Some(second));
    close(&project, &location)?;
    fs::remove_file(&location)?;
    invalidate(&project, &location)?;
    let unmapped = analyze(&project, &caller, None)?;
    let module = contexts(&unmapped, &caller)[0];
    assert_eq!(module.context, unmapped.snapshot.context(module.source));
    assert!(target(&unmapped, sites(&unmapped, &caller)[0]).is_none());
    assert!(sites(&unmapped, &caller)[0].failure.is_some());

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

    let project = project();
    let original = analyze(&project, &caller, None)?;
    let original_context = contexts(&original, &caller)[0];
    assert!(consulted(&original, &outer));

    assert_ne!(
        original_context.context,
        original.snapshot.context(original_context.source)
    );

    let inner = directory.file(
        "nested/sourcemap.json",
        include_str!("../fixtures/lifecycle/nearest/nested.json"),
    )?;

    assert!(consulted(&original, &inner));
    invalidate(&project, &inner)?;
    let nested = analyze(&project, &caller, None)?;
    assert!(consulted(&nested, &inner));
    assert_ne!(contexts(&nested, &caller)[0], original_context);
    fs::remove_file(&inner)?;
    invalidate(&project, &inner)?;
    let restored = analyze(&project, &caller, None)?;
    assert_eq!(contexts(&restored, &caller)[0], original_context);
    let manifest = directory.path().join("nested/instar.toml");

    for text in [
        include_str!("../fixtures/lifecycle/nearest/input.toml"),
        include_str!("../fixtures/lifecycle/nearest/replacement.toml"),
    ] {
        replace(&project, &manifest, text)?;
        let disabled = analyze(&project, &caller, None)?;
        let context = contexts(&disabled, &caller)[0];
        assert_eq!(context.context, disabled.snapshot.context(context.source));
    }

    close(&project, &manifest)?;
    let restored = analyze(&project, &caller, None)?;
    let context = contexts(&restored, &caller)[0];
    assert_ne!(context.context, restored.snapshot.context(context.source));

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
    let source = include_str!("../fixtures/lifecycle/overlapping/sourcemap.json");
    directory.file("one.json", source)?;
    directory.file("two.json", source)?;

    directory.file(
        "instar.toml",
        include_str!("../fixtures/lifecycle/overlapping/instar.toml"),
    )?;

    let project = project();
    let original = analyze(&project, &caller, None)?;
    assert!(ambiguous(&project, &original, &caller));
    let contexts = contexts(&original, &caller);
    assert_eq!(contexts.len(), 2);

    for context in contexts {
        let report = analyze(&project, &caller, Some(context))?;
        let site = sites(&report, &caller)[0];
        assert!(target(&report, site).is_none());

        assert!(
            site.failure
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

    directory.file("instar.toml", "")?;
    let project = project();
    let original = analyze(&project, &caller, None)?;
    let executable = directory.path().join("instar.luau");
    assert!(consulted(&original, &executable));
    replace(&project, &executable, "return {}")?;
    assert!(analyze(&project, &caller, None).is_err());
    assert!(!project.current(&original.inputs));
    close(&project, &executable)?;
    let restored = analyze(&project, &caller, None)?;
    assert!(target(&restored, sites(&restored, &caller)[0]).is_some());

    Ok(())
}

#[test]
fn replicated_storage_service_chains_keep_static_and_lexical_context() -> io::Result<()> {
    let directory = Directory::new(None)?;
    let caller = directory.file("caller.luau", "")?;
    let target_path = directory.file("target.luau", "return 1")?;

    directory.file(
        "sourcemap.json",
        include_str!("../fixtures/lifecycle/replicated/sourcemap.json"),
    )?;

    let project = project();

    let cases: Vec<(String, bool, bool)> =
        serde_json::from_str(include_str!("../fixtures/lifecycle/services/cases.json"))?;

    assert_eq!(cases.len(), 13);

    for (source, known, resolved) in cases {
        replace(&project, &caller, &source)?;
        let report = analyze(&project, &caller, None)?;

        assert!(
            !report
                .findings
                .iter()
                .any(|finding| finding.category == DiagnosticCategory::Syntax),
            "{source}"
        );

        let sites = sites(&report, &caller);
        assert_eq!(sites.len(), 1, "{source}");

        assert_eq!(
            !matches!(sites[0].site.request, ImportRequest::Nonconstant),
            known,
            "{source}"
        );

        assert_eq!(target(&report, sites[0]).is_some(), resolved, "{source}");

        if resolved {
            assert_eq!(target(&report, sites[0]), Some(target_path.clone()));
        } else if known {
            assert!(sites[0].failure.is_some(), "{source}");
        } else {
            assert!(
                matches!(sites[0].outcome, ImportOutcome::Nonconstant),
                "{source}"
            );
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

    let target_path = directory.file("target.luau", "return 1")?;
    let project = project();
    let original = analyze(&project, &caller, None)?;
    assert_eq!(sites(&original, &caller).len(), 2);
    assert_eq!(original.dependencies.len(), 1);

    assert!(matches!(
        sites(&original, &caller)[1].outcome,
        ImportOutcome::Nonconstant
    ));

    let site = sites(&original, &caller)[0];
    assert_eq!(original.dependencies[0].importer, site.importer);
    assert_eq!(original.dependencies[0].site, site.site);

    let ImportOutcome::Module(target_module) = site.outcome else {
        panic!("Rust target");
    };

    assert_eq!(original.dependencies[0].target, target_module);

    assert_eq!(
        site.site.argument.source,
        original
            .snapshot
            .source_at(&caller)
            .expect("caller")
            .version
    );

    let repeated = analyze(&project, &caller, None)?;

    assert_eq!(
        serde_json::to_value(&original.dependencies).map_err(io::Error::other)?,
        serde_json::to_value(&repeated.dependencies).map_err(io::Error::other)?
    );

    let hidden = directory.path().join("hidden.luau");
    fs::rename(&target_path, &hidden)?;
    invalidate(&project, &target_path)?;
    assert_eq!(analyze(&project, &caller, None)?.dependencies.len(), 0);
    fs::rename(&hidden, &target_path)?;
    invalidate(&project, &target_path)?;
    assert_eq!(analyze(&project, &caller, None)?.dependencies.len(), 1);

    let location = directory.file(
        "sourcemap.json",
        include_str!("../fixtures/lifecycle/native/sourcemap.json"),
    )?;

    invalidate(&project, &location)?;

    replace(
        &project,
        &caller,
        include_str!("../fixtures/lifecycle/native/input.luau"),
    )?;

    let mapped = analyze(&project, &caller, None)?;
    assert_eq!(mapped.dependencies.len(), 1);

    let ImportOutcome::Module(target_module) = sites(&mapped, &caller)[0].outcome else {
        panic!("instance target");
    };

    assert_eq!(mapped.dependencies[0].target, target_module);

    Ok(())
}

#[test]
fn mapped_sources_outside_map_ancestors_keep_explicit_context() -> io::Result<()> {
    let directory = Directory::new(None)?;

    let caller = directory.file(
        "place/caller.luau",
        include_str!("../fixtures/lifecycle/duplicate/caller.luau"),
    )?;

    let target_path = directory.file(
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

    let project = project();
    let standalone = analyze(&project, &target_path, None)?;
    let context = contexts(&standalone, &target_path)[0];
    assert_eq!(context.context, standalone.snapshot.context(context.source));
    let mapped = analyze(&project, &caller, None)?;
    let site = sites(&mapped, &caller)[0];
    assert_eq!(target(&mapped, site), Some(target_path.clone()));

    let ImportOutcome::Module(context) = site.outcome else {
        panic!("mapped target");
    };

    assert_ne!(context.context, mapped.snapshot.context(context.source));
    assert!(consulted(&mapped, &location));
    let dependency = analyze(&project, &target_path, Some(context))?;

    assert_eq!(
        target(&dependency, sites(&dependency, &target_path)[0]),
        Some(other)
    );

    assert_eq!(mapped.modules.len(), 3);
    assert_eq!(mapped.dependencies.len(), 2);
    replace(&project, &location, "{")?;
    assert!(analyze(&project, &target_path, Some(context)).is_err());
    assert!(!project.current(&mapped.inputs));

    Ok(())
}

#[test]
fn overlay_only_sources_and_configuration_need_no_disk_directories() -> io::Result<()> {
    let directory = Directory::new(None)?;
    let caller = directory.path().join("virtual/caller.luau");
    let target_path = directory.path().join("virtual/target.luau");
    let aliases = directory.path().join("virtual/.luaurc");
    let project = project();

    replace(
        &project,
        &caller,
        include_str!("../fixtures/lifecycle/overlays/caller.luau"),
    )?;

    replace(&project, &target_path, "return 1")?;

    replace(
        &project,
        &aliases,
        include_str!("../fixtures/lifecycle/overlays/.luaurc"),
    )?;

    assert!(!caller.parent().expect("virtual directory").exists());
    let report = analyze(&project, &caller, None)?;

    assert_eq!(
        target(&report, sites(&report, &caller)[0]),
        Some(target_path)
    );

    assert_eq!(report.dependencies.len(), 1);
    close(&project, &caller)?;
    assert!(analyze(&project, &caller, None).is_err());

    Ok(())
}

#[test]
fn instance_requests_cannot_import_native_configuration_sources() -> io::Result<()> {
    let directory = Directory::new(None)?;

    let caller = directory.file(
        "caller.luau",
        include_str!("../fixtures/lifecycle/instance/caller.luau"),
    )?;

    directory.file("instar.luau", "return {}")?;

    directory.file(
        "sourcemap.json",
        include_str!("../fixtures/lifecycle/instance/sourcemap.json"),
    )?;

    let project = project();
    let report = analyze(&project, &caller, None)?;
    let site = sites(&report, &caller)[0];
    assert!(target(&report, site).is_none());

    assert!(
        site.failure
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

    let project = project();
    let missing = analyze(&project, &caller, None)?;
    assert!(sites(&missing, &caller)[0].failure.is_some());
    assert!(consulted(&missing, &directory.path().join("created")));
    let target_path = directory.file("created/nested/target.luau", "return 1")?;
    invalidate(&project, &target_path)?;
    assert!(!project.current(&missing.inputs));
    let resolved = analyze(&project, &caller, None)?;

    assert_eq!(
        target(&resolved, sites(&resolved, &caller)[0]),
        Some(target_path)
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

    let target_path = directory.file("target.luau", "return 1")?;

    directory.file(
        "sourcemap.json",
        include_str!("../fixtures/lifecycle/reference/sourcemap.json"),
    )?;

    let project = project();
    let report = analyze(&project, &caller, None)?;
    let sites = sites(&report, &caller);
    assert_eq!(sites.len(), 7);

    for site in &sites[..5] {
        assert!(target(&report, site).is_none());
        assert!(matches!(site.outcome, ImportOutcome::Nonconstant));
        assert!(site.failure.is_none());
    }

    for site in &sites[5..] {
        assert!(!matches!(site.site.request, ImportRequest::Nonconstant));
        assert!(site.failure.is_none());
        assert_eq!(target(&report, site), Some(target_path.clone()));
    }

    Ok(())
}
