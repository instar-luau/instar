//! Exact-output checks for lint fixtures.

use std::{error::Error, fs, path::PathBuf};

use instar_core::{
    lint::{Failure, Prepared, Symbols, lint},
    settings::Configuration,
};

#[test]
fn fixtures() -> Result<(), Box<dyn Error>> {
    let mut pending = vec![PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/lint")];
    let mut cases = Vec::new();

    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(directory)? {
            let entry = entry?;
            let path = entry.path();

            if entry.file_type()?.is_dir() {
                pending.push(path);
            } else if path.file_name().is_some_and(|name| name == "input.luau") {
                cases.push(path);
            }
        }
    }

    cases.sort();
    assert!(!cases.is_empty(), "lint fixtures must exist");
    let mut failures = Vec::new();
    let symbols = Symbols::default();

    for input in &cases {
        let directory = input.parent().expect("fixture directory");
        let path = directory.join("configuration.toml");

        let configuration = if path.try_exists()? {
            toml::from_str(&fs::read_to_string(path)?)?
        } else {
            Configuration::default()
        };

        let configuration = Prepared::new(&configuration.lint, configuration.format.indent_width)?;

        let source = fs::read_to_string(input)?;
        let expected = fs::read_to_string(directory.join("expected.txt"))?;

        let output = match lint(&source, &configuration, &symbols) {
            Ok(output) => format!("{output:#?}\n"),

            Err(failure) => {
                let (code, error) = match failure {
                    Failure::Syntax(error) => ("syntax", error),
                    Failure::Directive(error) => ("lint", error),
                };

                failures.push(format!("{}: {code}: {error}", input.display()));
                continue;
            }
        };

        if output != expected {
            failures.push(format!(
                "{}:\nexpected: {expected:?}\nactual:   {output:?}",
                input.display()
            ));
        }
    }

    assert!(failures.is_empty(), "{}", failures.join("\n\n"));

    eprintln!("{} lint fixtures passed", cases.len());

    Ok(())
}
