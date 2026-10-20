use std::{
    fs,
    path::{Path, PathBuf, absolute},
};

use figment::{
    Error, Figment, Metadata, Profile, Provider,
    providers::Serialized,
    value::{Dict, Map},
};

use serde_json::Value;

use super::{Configuration, lint, paths, resolution, source::Source};

struct File<'path>(&'path Path);

impl Provider for File<'_> {
    fn metadata(&self) -> Metadata {
        Metadata::from("TOML configuration", self.0)
    }

    fn data(&self) -> Result<Map<Profile, Dict>, Error> {
        let text = fs::read_to_string(self.0).map_err(|error| Error::from(error.to_string()))?;
        let mut values = toml::from_str(&text).map_err(|error| Error::from(error.to_string()))?;
        resolution::normalize(&mut values).map_err(|error| *error)?;
        lint::normalize(&mut values);

        Ok(Profile::Default.collect(values))
    }
}

struct Override<'input>(&'input str);

impl Provider for Override<'_> {
    fn metadata(&self) -> Metadata {
        Metadata::named("configuration override")
    }

    fn data(&self) -> Result<Map<Profile, Dict>, Error> {
        let (field, input) = self
            .0
            .split_once('=')
            .ok_or_else(|| Error::from("Expected an override in `FIELD=JSON` form."))?;

        if field
            .split('.')
            .any(|part| part.is_empty() || part.trim() != part)
        {
            return Err(Error::from(
                "Override field components must be nonempty and have no leading or trailing whitespace.",
            ));
        }

        let value: Value = serde_json::from_str(input)
            .map_err(|error| Error::from(error.to_string()).with_path(field))?;

        let mut values = Serialized::default(field, value).data()?;

        for dictionary in values.values_mut() {
            resolution::normalize(dictionary).map_err(|error| *error)?;
            lint::normalize(dictionary);
        }

        Ok(values)
    }
}

/// Loads ordered TOML files and `FIELD=JSON` overrides over configuration defaults.
///
/// Later files and then later overrides win. Tables merge recursively; arrays replace.
/// Relative file names and override paths use `base`. File-provided paths and selection
/// patterns use their defining file's directory. Every supplied file must be readable.
/// File discovery belongs to the caller; an empty file list selects defaults and overrides.
///
/// # Errors
/// Returns source-attributed errors for file access, decoding, merging, or path conversion.
pub fn load(
    base: &Path,
    files: &[PathBuf],
    overrides: &[String],
) -> Result<Configuration, Box<Error>> {
    let base = absolute(base).map_err(|error| Box::new(Error::from(error.to_string())))?;
    let mut figment = defaults();

    for path in files {
        figment = layer(figment, &base.join(path));
    }

    resolve(&base, figment, overrides)
}

/// Create the default configuration layer.
#[must_use]
pub fn defaults() -> Figment {
    Figment::from(Serialized::defaults(Configuration::default()))
}

/// Merge a TOML file into an existing layer, retaining its source metadata.
#[must_use]
pub fn layer(figment: Figment, path: &Path) -> Figment {
    figment.merge(File(path))
}

/// Resolve layered settings and overrides relative to their defining sources.
///
/// # Errors
/// Returns source-attributed configuration and path failures.
pub fn resolve(
    base: &Path,
    mut figment: Figment,
    overrides: &[String],
) -> Result<Configuration, Box<Error>> {
    for assignment in overrides {
        figment = figment.merge(Override(assignment));
    }

    let mut configuration: Configuration = figment.extract()?;
    let values = figment.find_value("")?;
    let source = Source::new(&figment, &values);
    paths::resolve(&mut configuration, &source, base)?;

    Ok(configuration)
}
