//! Configuration discovery and independent Instar and Luau inheritance.

use std::{
    fs, io,
    path::{Path, PathBuf},
    time::Duration,
};

use crate::configuration::{Configuration, invalid};

/// Resolved configuration for a source directory.
pub struct Project {
    /// Effective Instar settings with absolute file references and anchored patterns.
    pub configuration: Configuration,

    /// Independently inherited native Luau settings.
    pub native: instar_bridge::Configuration,

    /// Configuration files applied in ancestor-to-descendant order.
    pub files: Vec<PathBuf>,
}

impl Project {
    /// Loads configuration from the directory and its ancestors.
    ///
    /// Native execution receives the supplied per-file timeout. Dependencies are not
    /// excluded from analysis merely because operation-specific selection excludes them.
    ///
    /// # Errors
    /// Returns filesystem, configuration, native execution, or same-directory native
    /// configuration conflict errors. The argument must be an existing directory.
    pub fn load(directory: &Path, timeout: Duration) -> io::Result<Self> {
        let directory = std::path::absolute(directory)?;

        if !directory.is_dir() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "configuration discovery requires a directory",
            ));
        }

        let mut ancestors = directory.ancestors().collect::<Vec<_>>();
        ancestors.reverse();
        let mut merged = toml::Value::Table(toml::map::Map::new());
        let mut native = instar_bridge::Configuration::new()?;
        let mut files = Vec::new();
        let mut discovered_map = None;

        for ancestor in ancestors {
            let manifest = ancestor.join("instar.toml");

            if let Some(source) = read(&manifest)? {
                Configuration::parse(&source).map_err(|error| located(&manifest, error))?;

                let mut layer = toml::from_str::<toml::Value>(&source)
                    .map_err(|error| located(&manifest, error))?;

                anchor(&mut layer, ancestor).map_err(|error| located(&manifest, error))?;
                merge(&mut merged, layer);
                files.push(manifest);
            }

            let json = ancestor.join(".luaurc");
            let luau = ancestor.join(".config.luau");
            let json_source = read(&json)?;
            let luau_source = read(&luau)?;

            match (json_source, luau_source) {
                (Some(_), Some(_)) => {
                    return Err(invalid(format!(
                        "{}: .luaurc and .config.luau cannot coexist in one directory",
                        ancestor.display()
                    )));
                }

                (Some(source), None) => {
                    native.apply(&source, &json, timeout)?;
                    files.push(json);
                }

                (None, Some(source)) => {
                    native.apply(&source, &luau, timeout)?;
                    files.push(luau);
                }

                (None, None) => {}
            }

            let map = ancestor.join("sourcemap.json");

            match fs::metadata(&map) {
                Ok(metadata) if metadata.is_file() => discovered_map = Some(map),
                Ok(_) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(located(&map, error)),
            }
        }

        let mut configuration: Configuration = merged.try_into().map_err(invalid)?;
        configuration.inherit_selection();

        if configuration.roblox.sourcemaps.is_none() {
            configuration.roblox.sourcemaps = Some(discovered_map.into_iter().collect());
        }

        if configuration.roblox.enabled.is_none() {
            configuration.roblox.enabled = Some(
                configuration
                    .roblox
                    .sourcemaps
                    .as_ref()
                    .is_some_and(|maps| !maps.is_empty()),
            );
        }

        configuration.validate()?;

        Ok(Self {
            configuration,
            native,
            files,
        })
    }
}

fn read(path: &Path) -> io::Result<Option<String>> {
    match fs::read_to_string(path) {
        Ok(source) => Ok(Some(source)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(located(path, error)),
    }
}

fn located(path: &Path, error: impl std::fmt::Display) -> io::Error {
    invalid(format!("{}: {error}", path.display()))
}

fn merge(parent: &mut toml::Value, child: toml::Value) {
    match (parent, child) {
        (toml::Value::Table(parent), toml::Value::Table(child)) => {
            for (name, value) in child {
                match parent.get_mut(&name) {
                    Some(inherited) => merge(inherited, value),

                    None => {
                        parent.insert(name, value);
                    }
                }
            }
        }

        (parent, child) => *parent = child,
    }
}

fn anchor(layer: &mut toml::Value, directory: &Path) -> io::Result<()> {
    for section in [
        &[][..],
        &["check"],
        &["format"],
        &["lint"],
        &["editor", "index"],
        &["editor", "imports"],
    ] {
        for field in ["include", "exclude"] {
            anchor_list(layer, section, field, directory, true)?;
        }
    }

    for (section, field) in [
        (&["environment"][..], "definitions"),
        (&["environment"][..], "documentation"),
        (&["roblox"][..], "sourcemaps"),
    ] {
        anchor_list(layer, section, field, directory, false)?;
    }

    Ok(())
}

fn anchor_list(
    layer: &mut toml::Value,
    section: &[&str],
    field: &str,
    directory: &Path,
    pattern: bool,
) -> io::Result<()> {
    let mut value = layer;

    for name in section {
        let Some(child) = value.get_mut(*name) else {
            return Ok(());
        };

        value = child;
    }

    let Some(toml::Value::Array(values)) = value.get_mut(field) else {
        return Ok(());
    };

    for value in values {
        let toml::Value::String(text) = value else {
            return Err(invalid("file references must be strings"));
        };

        if Path::new(text).is_absolute() {
            continue;
        }

        let directory = directory
            .to_str()
            .ok_or_else(|| invalid("configuration directories must be UTF-8"))?;

        let prefix = if pattern {
            glob::Pattern::escape(&directory.replace('\\', "/"))
        } else {
            directory.replace('\\', "/")
        };

        *text = format!("{prefix}/{text}");
    }

    Ok(())
}
