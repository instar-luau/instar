use std::io;

use instar_core::{
    analysis::Entry, configuration::Configuration, documentation::Index, project::Change, roblox,
};

use crate::support::{Directory, options, project};

#[test]
fn configuration_rejects_reserved_namespaces_and_invalid_flags() {
    for source in [
        include_str!("../fixtures/environment/configuration/luau.toml"),
        include_str!("../fixtures/environment/configuration/roblox.toml"),
        include_str!("../fixtures/environment/configuration/prefix.toml"),
        include_str!("../fixtures/environment/configuration/duplicate.toml"),
        include_str!("../fixtures/environment/configuration/missing.toml"),
        include_str!("../fixtures/environment/configuration/boolean.toml"),
        include_str!("../fixtures/environment/configuration/string.toml"),
        include_str!("../fixtures/environment/configuration/overflow.toml"),
    ] {
        assert!(Configuration::parse(source).is_err(), "{source}");
    }
}

#[test]
fn documentation_lookup_tracks_overlays_and_rejects_invalid_entries() -> io::Result<()> {
    let directory = Directory::new(None)?;

    let configuration = directory.file(
        "instar.toml",
        include_str!("../fixtures/environment/documentation/instar.toml"),
    )?;

    let entry = directory.file("entry.luau", "return 1")?;

    let documentation = directory.file(
        "docs.json",
        include_str!("../fixtures/environment/documentation/docs.json"),
    )?;

    let mut project = project();

    assert_eq!(
        project
            .documentation(&entry, &options())?
            .get("@custom/global/value")
            .expect("symbol")
            .documentation
            .as_deref(),
        Some("first")
    );

    project.change(Change::Overlay {
        path: documentation.clone(),
        text: Some(include_str!("../fixtures/environment/documentation/input.json").to_owned()),
    })?;

    assert_eq!(
        project
            .documentation(&entry, &options())?
            .get("@custom/global/value")
            .expect("symbol")
            .documentation
            .as_deref(),
        Some("updated")
    );

    for text in [
        include_str!("../fixtures/environment/documentation/replacement.json"),
        include_str!("../fixtures/environment/documentation/invalid.json"),
        include_str!("../fixtures/environment/documentation/alternate.json"),
        include_str!("../fixtures/environment/documentation/final.json"),
    ] {
        project.change(Change::Overlay {
            path: documentation.clone(),
            text: Some(text.to_owned()),
        })?;

        assert!(project.documentation(&entry, &options()).is_err(), "{text}");
    }

    project.change(Change::Overlay {
        path: configuration,
        text: Some(include_str!("../fixtures/environment/documentation/input.toml").to_owned()),
    })?;

    assert!(
        project
            .documentation(&entry, &options())?
            .get("@custom/global/value")
            .is_none()
    );

    Ok(())
}

#[test]
fn project_flags_follow_configuration_changes() -> io::Result<()> {
    let directory = Directory::new(None)?;
    let configuration = directory.file("instar.toml", "")?;
    let source = format!("return {}1{}", "(".repeat(40), ")".repeat(40));
    let entry = directory.file("entry.luau", &source)?;
    let mut project = project();
    let entries = [Entry::new(entry)];
    assert_eq!(project.check(&entries, &options())?.diagnostics, Vec::new());

    project.change(Change::Overlay {
        path: configuration.clone(),
        text: Some(include_str!("../fixtures/environment/project/input.toml").to_owned()),
    })?;

    assert_ne!(project.check(&entries, &options())?.diagnostics, Vec::new());

    project.change(Change::Overlay {
        path: configuration,
        text: Some(String::new()),
    })?;

    assert_eq!(project.check(&entries, &options())?.diagnostics, Vec::new());

    Ok(())
}

#[test]
fn parent_environment_paths_invalidate_consumers() -> io::Result<()> {
    let directory = Directory::new(None)?;

    directory.file(
        "nested/instar.toml",
        include_str!("../fixtures/environment/parent/nested/instar.toml"),
    )?;

    let entry = directory.file(
        "nested/entry.luau",
        include_str!("../fixtures/environment/parent/nested/entry.luau"),
    )?;

    let definitions = directory.file(
        "types.luau",
        include_str!("../fixtures/environment/parent/types.luau"),
    )?;

    let documentation = directory.file("docs.json", "{}")?;
    let mut project = project();

    for path in [definitions, documentation] {
        let identity = project.links(&entry, None)?.module.identity.clone();
        assert!(project.watch_inputs().contains(&path));
        let affected = project.change(Change::Disk(path))?;
        assert!(affected.contains(&identity));
    }

    Ok(())
}

#[test]
fn cancelled_preparation_and_documentation_do_not_load_sources() -> io::Result<()> {
    let directory = Directory::new(None)?;
    let entry = directory.file("entry.luau", "return 1")?;
    let mut project = project();
    let limits = options();
    limits.cancellation.cancel();

    assert_eq!(
        project
            .prepare(&entry, None, &limits)
            .expect_err("cancelled preparation")
            .kind(),
        io::ErrorKind::Interrupted
    );

    assert!(project.graph().is_empty());

    assert_eq!(
        project
            .documentation(&entry, &limits)
            .expect_err("cancelled documentation")
            .kind(),
        io::ErrorKind::Interrupted
    );

    Ok(())
}

#[test]
fn builtin_documentation_loads_without_roblox_assets() -> io::Result<()> {
    let directory = Directory::new(None)?;

    directory.file(
        "instar.toml",
        include_str!("../fixtures/environment/builtin/instar.toml"),
    )?;

    let entry = directory.file(
        "entry.luau",
        include_str!("../fixtures/environment/builtin/entry.luau"),
    )?;

    let mut project = project();

    project.change(Change::Overlay {
        path: roblox::cache_directory()?.join("metadata.json"),
        text: Some("invalid Roblox metadata".to_owned()),
    })?;

    let documentation = project.documentation(&entry, &options())?;

    assert_eq!(
        documentation
            .get("@luau/global/print")
            .expect("builtin documentation")
            .documentation
            .as_deref(),
        Some("Prints values.")
    );

    assert!(documentation.get("@roblox/global/game").is_none());

    Ok(())
}

#[test]
fn deserialization_enforces_documentation_semantics() {
    for source in [
        include_str!("../fixtures/environment/documentation/alternate.json"),
        include_str!("../fixtures/environment/deserialization/input.json"),
        include_str!("../fixtures/environment/deserialization/replacement.json"),
        include_str!("../fixtures/environment/deserialization/invalid.json"),
        include_str!("../fixtures/environment/deserialization/alternate.json"),
    ] {
        assert!(serde_json::from_str::<Index>(source).is_err(), "{source}");
    }
}
