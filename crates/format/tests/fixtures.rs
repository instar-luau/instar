//! Exact-output and idempotence checks for formatter fixtures.

use std::{error::Error, fs, path::PathBuf};

use instar_configuration::formatting::Configuration;
use instar_format::format;

#[test]
fn fixtures() -> Result<(), Box<dyn Error>> {
    let mut pending = vec![PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")];
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
    assert!(!cases.is_empty(), "formatter fixtures must exist");

    for input in &cases {
        let directory = input.parent().expect("fixture directory");
        let path = directory.join("configuration.toml");

        let configuration = if path.try_exists()? {
            toml::from_str(&fs::read_to_string(path)?)?
        } else {
            Configuration::default()
        };

        let configuration = configuration.prepare()?;

        let source = fs::read_to_string(input)?;
        let expected = fs::read_to_string(directory.join("expected.luau"))?;

        let output = format(&source, &configuration)
            .unwrap_or_else(|error| panic!("{}: {error}", input.display()));

        assert_eq!(output, expected, "{}", input.display());

        assert_eq!(
            format(&output, &configuration)
                .unwrap_or_else(|error| panic!("{}: {error}", input.display())),
            output,
            "idempotence: {}",
            input.display()
        );
    }

    eprintln!("{} formatter fixtures passed", cases.len());

    Ok(())
}
