use std::{error::Error, fs, path::Path};

use instar_core::{
    configuration::{InstarConfig, format::Whitespace},
    project::{ConfigKind, Configuration, Project, ProjectError},
};

type TestResult = Result<(), Box<dyn Error>>;

#[test]
fn configurations_coexist_without_evaluation_or_merging() -> TestResult {
    let root = tempfile::tempdir()?;

    fs::write(
        root.path().join(".luaurc"),
        br#"{"aliases":{"other":"./other"}}"#,
    )?;

    fs::write(
        root.path().join(".config.luau"),
        b"error('do not execute')\xff",
    )?;

    fs::write(root.path().join("config.luau"), "return {}")?;
    let text = r#"
include = ["src/**/*.luau", "src/**/*.lua"]
exclude = ["dist/**"]
definitions = ["types/environment.d.luau"]
[aliases]
shared = "./src/shared"
packages = "./packages"
[roblox]
project = "default.project.json"
sourcemap = "sourcemap.json"
"#;
    fs::write(root.path().join("instar.toml"), text)?;
    let project = Project::load(root.path())?;
    assert_eq!(project.root(), root.path());
    assert_eq!(project.files().len(), 4);
    let config = project.instar().ok_or("missing Instar config")?;
    assert_eq!(config.include.as_ref().ok_or("missing include")?.len(), 2);

    assert_eq!(
        config.exclude.as_ref().ok_or("missing exclude")?,
        &["dist/**"]
    );

    assert_eq!(
        config.definitions.as_ref().ok_or("missing definitions")?[0],
        Path::new("types/environment.d.luau")
    );

    let aliases = config.aliases.as_ref().ok_or("missing aliases")?;
    assert_eq!(aliases.len(), 2);

    assert_eq!(
        project.root().join(&aliases["shared"]),
        root.path().join("./src/shared")
    );

    let roblox = config.roblox.as_ref().ok_or("missing Roblox config")?;

    assert_eq!(
        roblox.project.as_deref(),
        Some(Path::new("default.project.json"))
    );

    assert_eq!(
        roblox.sourcemap.as_deref(),
        Some(Path::new("sourcemap.json"))
    );

    assert!(
        project
            .files()
            .iter()
            .any(|file| file.bytes == b"error('do not execute')\xff")
    );

    for name in [".luaurc", ".config.luau", "config.luau", "instar.toml"] {
        assert!(ConfigKind::from_path(Path::new(name)).is_some());
    }

    assert_eq!(ConfigKind::from_path(Path::new("other.toml")), None);

    Ok(())
}

#[test]
fn omission_and_invalid_configuration_remain_distinct() -> TestResult {
    let root = tempfile::tempdir()?;
    assert!(Project::load(root.path())?.instar().is_none());
    let empty = InstarConfig::parse("")?;
    assert!(empty.include.is_none() && empty.exclude.is_none() && empty.definitions.is_none());
    assert!(empty.aliases.is_none() && empty.roblox.is_none());

    assert_eq!(
        InstarConfig::parse("include = []")?.include,
        Some(Vec::new())
    );

    for text in [
        "unknown = 1",
        "include = 1",
        "definitions = [1]",
        "[aliases]\nx = false",
        "[roblox]\nunknown = 1",
        "[roblox]\nproject = 3",
        "[broken",
    ] {
        fs::write(root.path().join("instar.toml"), text)?;
        let error = Project::load(root.path()).expect_err("invalid config accepted");
        assert!(matches!(error, ProjectError::Toml { .. }));
        assert!(error.to_string().contains("instar.toml"));
        assert!(error.source().is_some());
    }

    fs::write(root.path().join("instar.toml"), [255])?;

    assert!(matches!(
        Project::load(root.path()),
        Err(ProjectError::Encoding { .. })
    ));

    fs::remove_file(root.path().join("instar.toml"))?;
    fs::create_dir(root.path().join("instar.toml"))?;
    assert!(Project::load(root.path()).is_err());
    assert!(Project::load(&root.path().join("missing")).is_err());

    Ok(())
}

#[test]
fn generated_schema_matches_the_configuration_model() -> TestResult {
    let schema = serde_json::to_value(InstarConfig::schema())?;
    assert_eq!(schema["additionalProperties"], false);

    for field in [
        "format",
        "grafts",
        "include",
        "exclude",
        "definitions",
        "aliases",
        "roblox",
    ] {
        assert!(schema["properties"].get(field).is_some());
    }

    assert!(schema.get("required").is_none());

    assert_eq!(
        schema["$defs"]["RobloxConfig"]["additionalProperties"],
        false
    );

    let definitions = schema["$defs"].as_object().ok_or("missing definitions")?;

    for definition in definitions.values().chain(std::iter::once(&schema)) {
        if let Some(properties) = definition
            .get("properties")
            .and_then(serde_json::Value::as_object)
        {
            for (name, property) in properties {
                let description = property["description"]
                    .as_str()
                    .ok_or("missing description")?;

                assert!(
                    !description.split("\n\nDefault:").next().unwrap().is_empty(),
                    "{name}"
                );

                let default = property.get("default").ok_or("missing default")?;

                let default = if default.is_null() {
                    "unset".to_owned()
                } else {
                    default.to_string()
                };

                assert!(
                    description.ends_with(&format!("Default: `{default}`.")),
                    "{name}"
                );
            }
        }
    }

    let defaults = serde_json::to_value(instar_core::configuration::format::Options::default())?;

    for (name, value) in defaults.as_object().ok_or("missing formatter defaults")? {
        assert_eq!(
            &definitions["Options"]["properties"][name]["default"], value,
            "{name}"
        );
    }

    let parsed = InstarConfig::parse("[format]")?
        .format
        .ok_or("missing format")?;

    assert_eq!(serde_json::to_value(parsed)?, defaults);

    let published: serde_json::Value =
        serde_json::from_str(include_str!("../../../schemas/instar.schema.json"))?;

    assert_eq!(published, schema);

    Ok(())
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

    let configuration = Configuration::discover(&root.join("nested/main.luau"), None)?;
    let options = &configuration.options;
    assert!(!options.final_newline);
    assert!(!options.trailing_separator);
    assert!(!options.spacing.braces);
    assert_eq!(options.functions.parameters.indentation, 2);

    assert_eq!(
        configuration.format(b"function f(a) end")?,
        b"function f(\n\t\ta\n)\nend"
    );

    Ok(())
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
    let options = Configuration::discover(&path, None)?.options;
    assert_eq!(options.column_width, 30);
    assert!(matches!(options.indentation.style, Whitespace::Spaces));

    fs::write(
        root.join("nested/instar.toml"),
        "[format]\ncolumn_width = 20\n",
    )?;

    assert_eq!(
        Configuration::discover(&path, None)?.options.column_width,
        20
    );

    fs::write(
        root.join("nested/instar.toml"),
        "[format]\nunknown = true\n",
    )?;

    assert!(Configuration::discover(&path, None).is_err());

    Ok(())
}
