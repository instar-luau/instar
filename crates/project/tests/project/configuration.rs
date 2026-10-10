use std::{error::Error, fs, path::Path, sync::Arc};

use instar_luau::LanguageProfile;

use instar_project::{
    CheckOverrides, Input, Invocation, OperationOverrides, Project, ProjectSettingsPatch,
    SettingsRequest,
};

use serde_json::{Value, json};
use similar_asserts::assert_eq;

use crate::support::{complete, normalize, options};

fn definition<'schema>(schema: &'schema Value, node: &'schema Value) -> &'schema Value {
    if let Some(reference) = node.get("$ref").and_then(Value::as_str) {
        definition(
            schema,
            schema
                .pointer(reference.strip_prefix('#').expect("local schema reference"))
                .expect("schema reference must resolve"),
        )
    } else if let Some(variants) = node.get("anyOf").and_then(Value::as_array) {
        definition(
            schema,
            variants
                .iter()
                .find(|variant| variant.get("type").and_then(Value::as_str) != Some("null"))
                .expect("non-null schema variant"),
        )
    } else {
        node
    }
}

fn variants(schema: &Value, node: &Value) -> Vec<String> {
    let node = definition(schema, node);

    let values = if let Some(values) = node.get("enum").and_then(Value::as_array) {
        values.iter().collect::<Vec<_>>()
    } else {
        node["oneOf"]
            .as_array()
            .expect("enum alternatives")
            .iter()
            .map(|variant| &variant["const"])
            .collect()
    };

    let mut values = values
        .into_iter()
        .map(|value| value.as_str().expect("enum strings").to_owned())
        .collect::<Vec<_>>();

    values.sort();

    values
}

#[test]
fn fixtures() -> Result<(), Box<dyn Error>> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/configuration");

    let mut cases = fs::read_dir(root)?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<Result<Vec<_>, _>>()?;

    cases.sort();
    assert!(!cases.is_empty(), "configuration fixtures must exist");

    for case in cases {
        let configuration = case.join("input.toml");
        let project = Project::new(vec![case.clone()]);

        let result = complete(project.settings(
            SettingsRequest {
                snapshot: None,
                invocation: Invocation {
                    inputs: vec![Input::Stdin {
                        text: Arc::from("return 1"),
                        context: Some(case.join("src/entry.luau")),
                    }],
                    project_root: Some(case.clone()),
                    configuration: Some(configuration),
                    ..Invocation::default()
                },
                overrides: OperationOverrides::Check(CheckOverrides::default()),
            },
            options(),
        ));

        let expected = case.join("expected.json");

        if expected.exists() {
            let mut expected: Value = serde_json::from_str(&fs::read_to_string(expected)?)?;

            expected["language"]["flags"] = serde_json::to_value(
                LanguageProfile::new(
                    &serde_json::from_value(expected["language"]["flags"].clone())?,
                    64 * 1024 * 1024,
                )?
                .flags,
            )?;

            let report = result?;
            assert_eq!(report.settings.len(), 1, "{}", case.display());
            let mut actual = serde_json::to_value(&report.settings[0].settings)?;
            normalize(&case.to_string_lossy().replace('\\', "/"), &mut actual);
            assert_eq!(actual, expected, "{}", case.display());
        } else {
            let expected = fs::read_to_string(case.join("error.txt"))?;
            let failure = result.expect_err("invalid configuration must fail");

            assert!(
                failure
                    .diagnostics
                    .iter()
                    .any(|diagnostic| diagnostic.message.contains(expected.trim())),
                "{}: expected {:?}, got {failure:?}",
                case.display(),
                expected.trim()
            );
        }
    }

    Ok(())
}

#[test]
fn schema_fields() {
    let schema = Project::configuration_schema();
    let properties = schema["properties"].as_object().expect("root properties");

    assert_eq!(
        properties.keys().map(String::as_str).collect::<Vec<_>>(),
        [
            "checking",
            "editing",
            "environment",
            "formatting",
            "language",
            "linting",
            "resolution",
            "selection"
        ]
    );
}

#[test]
fn schema_contract() -> Result<(), Box<dyn Error>> {
    let schema = Project::configuration_schema();
    let properties = schema["properties"].as_object().expect("root properties");
    let checking = definition(&schema, &properties["checking"]);
    let formatting = definition(&schema, &properties["formatting"]);
    let linting = definition(&schema, &properties["linting"]);
    let editing = definition(&schema, &properties["editing"]);
    let selection = definition(&schema, &properties["selection"]);
    let requires = definition(&schema, &formatting["properties"]["requires"]);
    let rules = definition(&schema, &linting["properties"]["rules"]);
    let policy = definition(&schema, &rules["additionalProperties"]);
    let environment = definition(&schema, &properties["environment"]);
    let groups = definition(&schema, &environment["properties"]["groups"]);

    for object in [
        &schema,
        checking,
        formatting,
        linting,
        editing,
        selection,
        requires,
        policy,
        environment,
        definition(&schema, &groups["items"]),
        definition(&schema, &properties["language"]),
        definition(&schema, &properties["resolution"]),
    ] {
        assert_eq!(object["additionalProperties"], false);
    }

    assert!(formatting["properties"].get("tables").is_none());

    assert!(
        serde_json::from_value::<ProjectSettingsPatch>(
            json!({ "linting": { "rules": { "MissingInstarRule": {} } } })
        )
        .is_err()
    );

    for scope in ["checking", "formatting", "linting", "index", "imports"] {
        let scope = definition(&schema, &selection["properties"][scope]);
        assert_eq!(scope["additionalProperties"], false);

        for field in ["include", "exclude"] {
            let field = definition(&schema, &scope["properties"][field]);
            assert!(field["type"] == "array" || field["type"] == json!(["array", "null"]));
        }
    }

    for field in ["include", "exclude"] {
        let field = definition(&schema, &selection["properties"][field]);
        assert!(field["type"] == "array" || field["type"] == json!(["array", "null"]));
    }

    for field in ["width", "indent_width"] {
        let field = definition(&schema, &formatting["properties"][field]);
        assert!(field["type"] == "integer" || field["type"] == json!(["integer", "null"]));
        assert_eq!(field["minimum"], 1);
    }

    for (node, expected) in [
        (
            &formatting["properties"]["indent_style"],
            json!(["spaces", "tabs"]),
        ),
        (
            &formatting["properties"]["line_ending"],
            json!(["crlf", "lf"]),
        ),
        (
            &formatting["properties"]["quote_style"],
            json!([
                "double",
                "prefer_double",
                "prefer_single",
                "preserve",
                "single"
            ]),
        ),
        (
            &requires["properties"]["order"],
            json!(["alphabetical", "grouped", "preserve"]),
        ),
        (
            &requires["properties"]["blank_lines"],
            json!(["between_groups", "none"]),
        ),
        (
            &policy["properties"]["severity"],
            json!(["Error", "Hint", "Information", "Warning"]),
        ),
    ] {
        assert_eq!(serde_json::to_value(variants(&schema, node))?, expected);
    }

    assert_eq!(policy["properties"]["enabled"]["type"], "boolean");

    Ok(())
}
