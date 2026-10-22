//! Exact-output and idempotence checks for formatter fixtures.

use std::{fs, path::Path};

use datatest_stable::{Result, harness};

use instar_core::{
    format::{Prepared, format},
    lint::{self, Symbols},
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

    let formatter = Prepared::new(&configuration.format)?;
    let expected = fs::read_to_string(directory.join("expected.luau"))?;
    let output = format(&source, &formatter)?;
    assert_eq!(output, expected, "{}", input.display());

    assert_eq!(
        format(&output, &formatter)?,
        output,
        "idempotence: {}",
        input.display()
    );

    let path = directory.join("expected.txt");

    if path.try_exists()? {
        let configuration =
            lint::Prepared::new(&configuration.lint, configuration.format.indent_width)?;

        let expected = fs::read_to_string(path)?;

        let output =
            lint::lint(&output, &configuration, &Symbols::default()).map_err(|failure| {
                match failure {
                    lint::Failure::Syntax(error) => format!("syntax: {error}"),
                    lint::Failure::Directive(error) => format!("lint: {error}"),
                }
            })?;

        assert_eq!(
            format!("{output:#?}\n"),
            expected,
            "lint: {}",
            input.display()
        );
    }

    Ok(())
}

harness! {
    { test = fixtures, root = "tests/fixtures/format", pattern = r"(^|/)input\.luau$" },
}
