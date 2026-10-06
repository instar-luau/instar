//! Fixture-backed Instar configuration and schema contract.

use std::{error::Error, fs, path::Path};

use instar_core::configuration::{Configuration, schema};
use serde_json::{Value, json};

#[test]
fn fixtures() -> Result<(), Box<dyn Error>> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/configuration");

    let mut cases = fs::read_dir(root)?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<Result<Vec<_>, _>>()?;

    cases.sort();
    assert!(!cases.is_empty(), "configuration fixtures must exist");

    for case in cases {
        let source = fs::read_to_string(case.join("input.toml"))?;
        let expected = case.join("expected.json");

        if expected.exists() {
            let expected: Value = serde_json::from_str(&fs::read_to_string(expected)?)?;
            let configuration = Configuration::parse(&source)?;

            assert_eq!(
                serde_json::to_value(configuration)?,
                expected,
                "{}",
                case.display()
            );
        } else {
            let expected = fs::read_to_string(case.join("error.txt"))?;

            let error = Configuration::parse(&source)
                .expect_err("invalid configuration must fail")
                .to_string();

            assert!(
                error.contains(expected.trim()),
                "{}: expected {:?}, got {error}",
                case.display(),
                expected.trim()
            );
        }
    }

    Ok(())
}

fn definition<'a>(schema: &'a Value, node: &'a Value) -> &'a Value {
    if let Some(reference) = node.get("$ref").and_then(Value::as_str) {
        schema
            .pointer(
                reference
                    .strip_prefix('#')
                    .expect("schema reference must be local"),
            )
            .expect("schema reference must resolve")
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
            .expect("enum must describe its alternatives")
            .iter()
            .map(|variant| &variant["const"])
            .collect()
    };

    let mut values = values
        .into_iter()
        .map(|value| {
            value
                .as_str()
                .expect("enum values must be strings")
                .to_owned()
        })
        .collect::<Vec<_>>();

    values.sort();

    values
}

#[test]
fn schema_contract() -> Result<(), Box<dyn Error>> {
    let schema = serde_json::to_value(schema())?;
    let properties = schema["properties"].as_object().expect("root properties");

    assert_eq!(
        properties.keys().map(String::as_str).collect::<Vec<_>>(),
        [
            "check",
            "editor",
            "environment",
            "exclude",
            "format",
            "include",
            "lint",
            "roblox"
        ]
    );

    let check = definition(&schema, &properties["check"]);
    let format = definition(&schema, &properties["format"]);
    let lint = definition(&schema, &properties["lint"]);
    let editor = definition(&schema, &properties["editor"]);
    let requires = definition(&schema, &format["properties"]["requires"]);
    let warning = definition(&schema, &lint["properties"]["almost_swapped"]);
    let allowed = definition(&schema, &lint["properties"]["implicit_any_local"]);
    let complexity = definition(&schema, &lint["properties"]["high_cyclomatic_complexity"]);
    let index = definition(&schema, &editor["properties"]["index"]);
    let imports = definition(&schema, &editor["properties"]["imports"]);

    for object in [
        &schema,
        check,
        format,
        lint,
        editor,
        requires,
        warning,
        allowed,
        complexity,
        index,
        imports,
        definition(&schema, &properties["environment"]),
        definition(&schema, &properties["roblox"]),
    ] {
        assert_eq!(object["additionalProperties"], false);
    }

    assert_eq!(
        check["properties"]
            .as_object()
            .expect("checker properties")
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        ["exclude", "include"]
    );

    assert!(format["properties"].get("tables").is_none());
    assert!(lint["properties"].get("UnknownGlobal").is_none());
    assert!(lint["properties"].get("unknown_global").is_none());

    for object in [&schema, check, format, lint, index, imports] {
        for field in ["include", "exclude"] {
            assert_eq!(object["properties"][field]["type"], "array");
        }
    }

    for field in ["width", "indent_width"] {
        assert_eq!(format["properties"][field]["type"], "integer");
        assert_eq!(format["properties"][field]["minimum"], 1);
    }

    assert_eq!(
        complexity["properties"]["maximum_complexity"]["type"],
        "integer"
    );

    assert_eq!(complexity["properties"]["maximum_complexity"]["minimum"], 1);

    for (node, expected) in [
        (
            &format["properties"]["indent_style"],
            json!(["spaces", "tabs"]),
        ),
        (&format["properties"]["line_ending"], json!(["crlf", "lf"])),
        (
            &format["properties"]["quote_style"],
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
            &warning["properties"]["level"],
            json!(["allow", "deny", "info", "warn"]),
        ),
    ] {
        assert_eq!(serde_json::to_value(variants(&schema, node))?, expected);
    }

    Ok(())
}
