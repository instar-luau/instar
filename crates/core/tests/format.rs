//! Exact-output and idempotence checks for formatter fixtures.

use std::{error::Error, fs, path::PathBuf};

use instar_core::{
    format::{Prepared, format},
    settings::format::Configuration,
};

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
    let mut failures = Vec::new();

    for input in &cases {
        let directory = input.parent().expect("fixture directory");
        let path = directory.join("configuration.toml");

        let configuration = if path.try_exists()? {
            toml::from_str(&fs::read_to_string(path)?)?
        } else {
            Configuration::default()
        };

        let configuration = Prepared::new(&configuration)?;

        let source = fs::read_to_string(input)?;
        let expected = fs::read_to_string(directory.join("expected.luau"))?;

        let output = match format(&source, &configuration) {
            Ok(output) => output,

            Err(error) => {
                failures.push(format!("{}: {error}", input.display()));
                continue;
            }
        };

        if output != expected {
            failures.push(format!(
                "{}:\nexpected: {expected:?}\nactual:   {output:?}",
                input.display()
            ));
        }

        match format(&output, &configuration) {
            Ok(repeated) if repeated == output => {}

            Ok(repeated) => failures.push(format!(
                "idempotence: {}:\nfirst:  {output:?}\nsecond: {repeated:?}",
                input.display()
            )),

            Err(error) => failures.push(format!("idempotence: {}: {error}", input.display())),
        }
    }

    assert!(failures.is_empty(), "{}", failures.join("\n\n"));

    eprintln!("{} formatter fixtures passed", cases.len());

    Ok(())
}
