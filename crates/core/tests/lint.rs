//! Exact-output checks for lint fixtures.

use std::{fs, path::Path};

use datatest_stable::{Result, harness};

use instar_core::{
    lint::{Failure, Prepared, Symbols, lint},
    settings::Configuration,
};

fn fixtures(input: &Path) -> Result<()> {
    let source = fs::read_to_string(input)?;
    let directory = input.parent().expect("fixture directory");
    let path = directory.join("configuration.toml");

    let configuration = if path.try_exists()? {
        toml::from_str(&fs::read_to_string(path)?)?
    } else {
        Configuration::default()
    };

    let configuration = Prepared::new(&configuration.lint, configuration.format.indent_width)?;
    let expected = fs::read_to_string(directory.join("expected.txt"))?;

    let output =
        lint(&source, &configuration, &Symbols::default()).map_err(|failure| match failure {
            Failure::Syntax(error) => format!("syntax: {error}"),
            Failure::Directive(error) => format!("lint: {error}"),
        })?;

    assert_eq!(format!("{output:#?}\n"), expected, "{}", input.display());

    Ok(())
}

harness! {
    { test = fixtures, root = "tests/fixtures/lint", pattern = r"(^|/)input\.luau$" },
}
