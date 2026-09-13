//! Project configuration parsing, inheritance, and schema consistency.

use std::{error::Error, fs, path::Path};

use instar_core::{
    configuration::{InstarConfig, format::Whitespace},
    project::{ConfigKind, Configuration},
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
[analyze]
definitions = ["types/environment.d.luau"]
[analyze.aliases]
shared = "./src/shared"
packages = "./packages"
[analyze.roblox]
project = "default.project.json"
sourcemap = "sourcemap.json"
"#;
    fs::write(root.path().join("instar.toml"), text)?;
    let config = InstarConfig::parse(text)?;
    Configuration::discover(&root.path().join("src/main.luau"), None)?;
    assert_eq!(config.include.as_ref().ok_or("missing include")?.len(), 2);

    assert_eq!(
        config.exclude.as_ref().ok_or("missing exclude")?,
        &["dist/**"]
    );

    let analyze = config.analyze.as_ref().ok_or("missing analyze")?;

    assert_eq!(
        analyze.definitions.as_ref().ok_or("missing definitions")?[0],
        Path::new("types/environment.d.luau")
    );

    let aliases = analyze.aliases.as_ref().ok_or("missing aliases")?;
    assert_eq!(aliases.len(), 2);

    assert_eq!(
        root.path().join(&aliases["shared"]),
        root.path().join("./src/shared")
    );

    let roblox = analyze.roblox.as_ref().ok_or("missing Roblox config")?;

    assert_eq!(
        roblox.project.as_deref(),
        Some(Path::new("default.project.json"))
    );

    assert_eq!(
        roblox.sourcemap.as_deref(),
        Some(Path::new("sourcemap.json"))
    );

    assert_eq!(
        fs::read(root.path().join(".config.luau"))?,
        b"error('do not execute')\xff"
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
    let source = root.path().join("main.luau");
    Configuration::discover(&source, None)?;
    let empty = InstarConfig::parse("")?;
    assert!(empty.include.is_none() && empty.exclude.is_none() && empty.analyze.is_none());

    assert_eq!(
        InstarConfig::parse("include = []")?.include,
        Some(Vec::new())
    );

    for text in [
        "unknown = 1",
        "include = 1",
        "mode = 'strict'",
        "definitions = []",
        "[roblox]",
        "[analyze]\ndefinitions = [1]",
        "[analyze]\ndocumentation = [1]",
        "[analyze.aliases]\nx = false",
        "[analyze.roblox]\nunknown = 1",
        "[analyze.roblox]\nproject = 3",
        "[analyze.roblox]\ncache = 'cache'",
        "[analyze.roblox]\nrevision = '0000000000000000000000000000000000000000'",
        "[broken",
    ] {
        fs::write(root.path().join("instar.toml"), text)?;
        assert!(InstarConfig::parse(text).is_err());

        let error = Configuration::discover(&source, None)
            .err()
            .ok_or("invalid config accepted")?;

        assert!(error.to_string().contains("instar.toml"));
    }

    fs::write(root.path().join("instar.toml"), [255])?;

    assert_eq!(
        Configuration::discover(&source, None)
            .err()
            .ok_or("invalid encoding accepted")?
            .kind(),
        std::io::ErrorKind::InvalidData
    );

    fs::remove_file(root.path().join("instar.toml"))?;
    fs::create_dir(root.path().join("instar.toml"))?;
    assert!(Configuration::discover(&source, None).is_err());

    Ok(())
}

#[test]
fn generated_schema_matches_the_configuration_model() -> TestResult {
    let schema = serde_json::to_value(InstarConfig::schema())?;
    assert_eq!(schema["additionalProperties"], false);

    for field in [
        "analyze", "build", "format", "graft", "grafts", "include", "exclude", "lint",
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

                if definition["required"].as_array().is_some_and(|required| {
                    required.iter().any(|field| field.as_str() == Some(name))
                }) {
                    assert!(property.get("default").is_none(), "{name}");
                    continue;
                }

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
