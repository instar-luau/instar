//! Exact resolution and import-generation checks for workspace fixtures.

use std::{collections::BTreeMap, fs, path::Path};

use datatest_stable::{Result, harness};
use instar_bridge::{Context, Frontend};
use instar_core::workspace::Workspace;
use serde::Deserialize;

#[derive(Debug, Deserialize, Eq, PartialEq)]
struct Expected {
    is_roblox: bool,
    imports: BTreeMap<u32, Vec<String>>,
    suggestions: BTreeMap<String, Option<String>>,
}

fn fixtures(input: &Path) -> Result<()> {
    let directory = input.parent().expect("fixture directory");

    let expected: Expected =
        serde_json::from_str(&fs::read_to_string(directory.join("expected.json"))?)?;

    let workspace = Workspace::new(directory, &["configuration.toml".into()], &[])?;
    let module = workspace.module(Path::new("input.luau"))?;
    let context = Context::new(&workspace);
    let mut frontend = Frontend::new(&context, false)?;
    let mut imports: BTreeMap<u32, Vec<String>> = BTreeMap::new();

    for import in frontend.imports(&module)? {
        let target = Path::new(&import.target)
            .strip_prefix(workspace.directory())
            .map_err(|error| {
                format!(
                    "Unexpected import target `{}` on line {}: {error}",
                    import.target,
                    import.location.range.start.line + 1
                )
            })?
            .to_str()
            .ok_or("non-UTF-8 fixture target")?
            .replace('\\', "/");

        imports
            .entry(import.location.range.start.line + 1)
            .or_default()
            .push(target);
    }

    for targets in imports.values_mut() {
        targets.sort();
    }

    let mut suggestions = BTreeMap::new();

    for target in expected.suggestions.keys() {
        suggestions.insert(target.clone(), workspace.import(&module, target, false)?);
    }

    let actual = Expected {
        is_roblox: workspace.is_roblox(&module)?,
        imports,
        suggestions,
    };

    assert_eq!(actual, expected, "{}", input.display());

    Ok(())
}

harness! {
    { test = fixtures, root = "tests/fixtures/resolution", pattern = r"(^|/)input\.luau$" },
}
