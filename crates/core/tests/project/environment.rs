use crate::support::{Directory, options, project};
use instar_core::{analysis::Entry, configuration::Configuration, project::Change};
use std::io;

#[test]
fn configuration_rejects_reserved_namespaces_and_invalid_flags() {
    for source in [
        "[[environment]]\nnamespace='@luau'",
        "[[environment]]\nnamespace='@roblox'",
        "[[environment]]\nnamespace='missing-prefix'",
        "[[environment]]\nnamespace='@custom'\n[[environment]]\nnamespace='@custom'",
        "[luau.flags]\nMissingInstarFlag=true",
        "[luau.flags]\nLuauRecursionLimit=true",
        "[luau.flags]\nLuauRecursionLimit='20'",
        "[luau.flags]\nLuauRecursionLimit=2147483648",
    ] {
        assert!(Configuration::parse(source).is_err(), "{source}");
    }
}

#[test]
fn documentation_lookup_tracks_overlays_and_rejects_invalid_entries() -> io::Result<()> {
    let directory = Directory::new(None)?;

    let configuration = directory.file(
        "instar.toml",
        "[[environment]]\nnamespace='@custom'\ndocumentation=['docs.json']",
    )?;

    let entry = directory.file("entry.luau", "return 1")?;

    let documentation = directory.file(
        "docs.json",
        r#"{"@custom/global/value":{"documentation":"first"}}"#,
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
        text: Some(r#"{"@custom/global/value":{"documentation":"updated"}}"#.to_owned()),
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
        r#"{"@wrong/global/value":{"documentation":"wrong namespace"}}"#,
        r#"{"@custom/global/value":{"documentation":3}}"#,
        r#"{"@custom/global/value":{}}"#,
        r#"{"@custom/global/value":{"documentation":"first"},"@custom/global/value":{"documentation":"duplicate"}}"#,
    ] {
        project.change(Change::Overlay {
            path: documentation.clone(),
            text: Some(text.to_owned()),
        })?;

        assert!(project.documentation(&entry, &options()).is_err(), "{text}");
    }

    project.change(Change::Overlay {
        path: configuration,
        text: Some("environment=[]".to_owned()),
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
        text: Some("[luau.flags]\nLuauRecursionLimit=20".to_owned()),
    })?;

    assert_ne!(project.check(&entries, &options())?.diagnostics, Vec::new());

    project.change(Change::Overlay {
        path: configuration,
        text: Some(String::new()),
    })?;

    assert_eq!(project.check(&entries, &options())?.diagnostics, Vec::new());

    Ok(())
}
