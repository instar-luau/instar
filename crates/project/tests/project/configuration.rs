use std::{error::Error, fs, path::Path, sync::Arc};

use instar_luau::LanguageProfile;

use instar_project::{
    CheckOverrides, Input, Invocation, OperationOverrides, Project, SettingsRequest,
};

use serde_json::Value;
use similar_asserts::assert_eq;

use crate::support::{complete, normalize, options};

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
