use super::{configuration::Configuration, insert, paths};
use serde_json::Value;

use std::{
    collections::BTreeMap,
    fs, io,
    path::{Path, PathBuf},
};

pub(super) fn compile(configuration: &Configuration, path: &Path) -> io::Result<String> {
    let mut document: Value =
        serde_json::from_slice(&configuration.files[path]).map_err(io::Error::other)?;

    references(&mut document, configuration)?;

    serde_json::to_string(&document).map_err(io::Error::other)
}

fn references(value: &mut Value, configuration: &Configuration) -> io::Result<()> {
    match value {
        Value::Object(fields) => {
            for (name, value) in fields {
                match name.as_str() {
                    "$path" => extension(source(value)?, configuration)?,

                    "filePaths" => {
                        for value in value.as_array_mut().ok_or_else(|| {
                            io::Error::other("instance filePaths must be an array")
                        })? {
                            extension(value, configuration)?;
                        }
                    }

                    _ => references(value, configuration)?,
                }
            }
        }

        Value::Array(values) => {
            for value in values {
                references(value, configuration)?;
            }
        }

        _ => {}
    }

    Ok(())
}

fn source(value: &mut Value) -> io::Result<&mut Value> {
    if value.is_string() {
        Ok(value)
    } else {
        value
            .get_mut("optional")
            .ok_or_else(|| io::Error::other("unsupported project path"))
    }
}

fn extension(value: &mut Value, configuration: &Configuration) -> io::Result<()> {
    let path = Path::new(
        value
            .as_str()
            .ok_or_else(|| io::Error::other("project source must be a string"))?,
    );

    if path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| configuration.settings.languages.contains_key(extension))
    {
        *value = Value::String(path.with_extension("luau").to_string_lossy().into_owned());
    }

    Ok(())
}

pub(super) fn emit(
    configuration: &Configuration,
    path: &Path,
    artifacts: &mut BTreeMap<PathBuf, Vec<u8>>,
    snapshots: &mut BTreeMap<PathBuf, Vec<u8>>,
) -> io::Result<()> {
    let bytes = fs::read(path)?;
    let mut document: Value = serde_json::from_slice(&bytes).map_err(io::Error::other)?;
    snapshots.insert(path.to_owned(), bytes);

    let relative = path
        .strip_prefix(&configuration.root)
        .map_err(io::Error::other)?;

    let parent = path
        .parent()
        .ok_or_else(|| io::Error::other("project has no parent"))?;

    rewrite(&mut document, parent, configuration, artifacts)?;
    let mut bytes = serde_json::to_vec_pretty(&document).map_err(io::Error::other)?;
    bytes.push(b'\n');

    insert(artifacts, relative.to_owned(), bytes)
}

fn rewrite(
    value: &mut Value,
    parent: &Path,
    configuration: &Configuration,
    artifacts: &BTreeMap<PathBuf, Vec<u8>>,
) -> io::Result<()> {
    match value {
        Value::Object(fields) => {
            if let Some(value) = fields.get_mut("$path") {
                let optional = !value.is_string();
                let path = source(value)?;

                let original = paths::absolute(
                    &parent.join(
                        path.as_str()
                            .ok_or_else(|| io::Error::other("unsupported project path"))?,
                    ),
                )?;

                let mut relative = original
                    .strip_prefix(&configuration.root)
                    .map_err(io::Error::other)?
                    .to_owned();

                if original
                    .extension()
                    .and_then(|extension| extension.to_str())
                    .is_some_and(|extension| {
                        configuration.settings.languages.contains_key(extension)
                    })
                {
                    relative.set_extension("luau");
                }

                let represented = artifacts
                    .keys()
                    .any(|artifact| artifact == &relative || artifact.starts_with(&relative));

                if !represented && !optional {
                    return Err(io::Error::other(format!(
                        "project mount has no built artifacts: {}",
                        original.display()
                    )));
                }

                let translated = configuration.root.join(relative);
                *path = Value::String(paths::between(parent, &translated)?);
            }

            for (name, value) in fields {
                if name != "$path" {
                    rewrite(value, parent, configuration, artifacts)?;
                }
            }
        }

        Value::Array(values) => {
            for value in values {
                rewrite(value, parent, configuration, artifacts)?;
            }
        }

        _ => {}
    }

    Ok(())
}
