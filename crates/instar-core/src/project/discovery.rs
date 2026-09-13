use super::selection::{Scope, Selection};
use crate::{configuration::InstarConfig, luau, source::absolute};

use std::{
    collections::{BTreeMap, BTreeSet},
    fs, io,
    path::{Path, PathBuf},
};

#[derive(Clone)]
pub(crate) struct Configuration {
    pub path: PathBuf,
    pub bytes: Vec<u8>,
    aliases: Option<BTreeMap<String, String>>,
}

#[derive(Clone)]
pub(super) struct Alias {
    name: String,
    pub(super) configuration: PathBuf,
    pub(super) value: String,
}

#[derive(Default)]
pub(crate) struct Discovery {
    configurations: BTreeMap<PathBuf, Vec<Configuration>>,
    aliases: BTreeMap<PathBuf, BTreeMap<String, Alias>>,
    contents: BTreeMap<PathBuf, Option<Vec<u8>>>,
    projects: BTreeMap<PathBuf, InstarConfig>,
    definitions: BTreeMap<PathBuf, Vec<PathBuf>>,
    documentation: BTreeMap<PathBuf, Vec<PathBuf>>,
    constants: BTreeMap<PathBuf, Vec<u8>>,
    environments: BTreeMap<PathBuf, Option<crate::configuration::RobloxConfig>>,
}

impl Discovery {
    fn contents(&mut self, path: &Path) -> io::Result<Option<&[u8]>> {
        if !self.contents.contains_key(path) {
            let exists = match fs::symlink_metadata(path) {
                Ok(_) => true,

                Err(error)
                    if matches!(
                        error.kind(),
                        io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
                    ) =>
                {
                    false
                }

                Err(error) => {
                    return Err(io::Error::new(
                        error.kind(),
                        format!("{}: {error}", path.display()),
                    ));
                }
            };

            let contents = if exists {
                Some(fs::read(path).map_err(|error| {
                    io::Error::new(error.kind(), format!("{}: {error}", path.display()))
                })?)
            } else {
                None
            };

            self.contents.insert(path.to_owned(), contents);
        }

        Ok(self.contents[path].as_deref())
    }

    fn project(&mut self, path: &Path) -> io::Result<Option<&InstarConfig>> {
        if !self.projects.contains_key(path) {
            let Some(contents) = self.contents(path)? else {
                return Ok(None);
            };

            let configuration = InstarConfig::parse(
                std::str::from_utf8(contents)
                    .map_err(|error| io::Error::other(format!("{}: {error}", path.display())))?,
            )
            .map_err(|error| io::Error::other(format!("{}: {error}", path.display())))?;

            self.projects.insert(path.to_owned(), configuration);
        }

        Ok(self.projects.get(path))
    }

    pub(crate) fn configurations(&mut self, from: &Path) -> io::Result<&[Configuration]> {
        let directory = from
            .parent()
            .ok_or_else(|| io::Error::other("module has no parent directory"))?;

        if !self.configurations.contains_key(directory) {
            let mut configurations = Vec::new();

            for ancestor in directory.ancestors().collect::<Vec<_>>().into_iter().rev() {
                self.project_aliases(ancestor)?;

                let mut candidates = Vec::new();

                for name in [".luaurc", ".config.luau", "config.luau"] {
                    let path = ancestor.join(name);

                    if self.contents(&path)?.is_some() {
                        candidates.push(path);
                    }
                }

                if candidates.len() > 1 {
                    return Err(io::Error::other(format!(
                        "{}: ambiguous upstream configuration: {}",
                        ancestor.display(),
                        candidates
                            .iter()
                            .map(|path| path.display().to_string())
                            .collect::<Vec<_>>()
                            .join(", ")
                    )));
                }

                if let Some(path) = candidates.pop() {
                    let bytes = self.contents(&path)?.expect("discovered configuration");

                    let aliases =
                        if path.file_name().and_then(|name| name.to_str()) == Some(".luaurc") {
                            Some(luau::aliases(bytes, false).map_err(|error| {
                                io::Error::other(format!("{}: {error}", path.display()))
                            })?)
                        } else {
                            None
                        };

                    configurations.push(Configuration {
                        path,
                        bytes: bytes.to_vec(),
                        aliases,
                    });
                }

                let path = ancestor.join("instar.toml");

                if let Some(mode) = self
                    .project(&path)?
                    .and_then(|configuration| configuration.analyze.as_ref())
                    .and_then(|configuration| configuration.mode)
                {
                    configurations.push(Configuration {
                        path,
                        bytes: serde_json::to_vec(&serde_json::json!({"languageMode": mode}))?,
                        aliases: Some(BTreeMap::new()),
                    });
                }
            }

            self.configurations
                .insert(directory.to_owned(), configurations);
        }

        Ok(&self.configurations[directory])
    }

    pub(crate) fn definitions(&mut self, from: &Path) -> io::Result<Vec<PathBuf>> {
        let directory = from
            .parent()
            .ok_or_else(|| io::Error::other("module has no parent"))?;

        if let Some(definitions) = self.definitions.get(directory) {
            return Ok(definitions.clone());
        }

        let mut definitions = Vec::new();

        for ancestor in directory.ancestors().collect::<Vec<_>>().into_iter().rev() {
            let path = ancestor.join("instar.toml");

            let Some(configuration) = self.project(&path)? else {
                continue;
            };

            if let Some(configured) = configuration
                .analyze
                .as_ref()
                .and_then(|configuration| configuration.definitions.as_ref())
            {
                definitions = configured
                    .iter()
                    .map(|path| absolute(&ancestor.join(path)).map_err(io::Error::other))
                    .collect::<io::Result<Vec<_>>>()?;
            }
        }

        let mut seen = std::collections::BTreeSet::new();
        definitions.retain(|path| seen.insert(path.clone()));

        self.definitions
            .insert(directory.to_owned(), definitions.clone());

        Ok(definitions)
    }

    pub(crate) fn documentation(&mut self, from: &Path) -> io::Result<Vec<PathBuf>> {
        let directory = from
            .parent()
            .ok_or_else(|| io::Error::other("module has no parent"))?;

        if let Some(documentation) = self.documentation.get(directory) {
            return Ok(documentation.clone());
        }

        let mut documentation = Vec::new();

        for ancestor in directory.ancestors().collect::<Vec<_>>().into_iter().rev() {
            let path = ancestor.join("instar.toml");

            let Some(configuration) = self.project(&path)? else {
                continue;
            };

            if let Some(configured) = configuration
                .analyze
                .as_ref()
                .and_then(|configuration| configuration.documentation.as_ref())
            {
                documentation = configured
                    .iter()
                    .map(|path| absolute(&ancestor.join(path)).map_err(io::Error::other))
                    .collect::<io::Result<Vec<_>>>()?;
            }
        }

        let mut seen = std::collections::BTreeSet::new();
        documentation.retain(|path| seen.insert(path.clone()));

        self.documentation
            .insert(directory.to_owned(), documentation.clone());

        Ok(documentation)
    }

    pub(crate) fn constants(&mut self, from: &Path) -> io::Result<Vec<u8>> {
        let directory = from
            .parent()
            .ok_or_else(|| io::Error::other("module has no parent"))?;

        if let Some(constants) = self.constants.get(directory) {
            return Ok(constants.clone());
        }

        let mut constants = BTreeMap::new();

        for ancestor in directory.ancestors().collect::<Vec<_>>().into_iter().rev() {
            let path = ancestor.join("instar.toml");

            let Some(configuration) = self.project(&path)? else {
                continue;
            };

            if let Some(build) = &configuration.build {
                constants.extend(build.constants.clone());
            }
        }

        let constants = crate::build::configuration::constant_definitions(&constants)?;

        self.constants
            .insert(directory.to_owned(), constants.clone());

        Ok(constants)
    }

    pub(crate) fn roblox(
        &mut self,
        from: &Path,
    ) -> io::Result<Option<crate::configuration::RobloxConfig>> {
        let directory = from
            .parent()
            .ok_or_else(|| io::Error::other("module has no parent"))?;

        if let Some(environment) = self.environments.get(directory) {
            return Ok(environment.clone());
        }

        let mut result = None;

        for ancestor in directory.ancestors().collect::<Vec<_>>().into_iter().rev() {
            let path = ancestor.join("instar.toml");

            let Some(configuration) = self.project(&path)? else {
                continue;
            };

            if let Some(configuration) = configuration
                .analyze
                .as_ref()
                .and_then(|configuration| configuration.roblox.as_ref())
            {
                let merged = result.get_or_insert_with(crate::configuration::RobloxConfig::default);
                ancestor.clone_into(&mut merged.root);

                for (target, value) in [
                    (&mut merged.project, &configuration.project),
                    (&mut merged.sourcemap, &configuration.sourcemap),
                ] {
                    if let Some(value) = value {
                        *target = Some(absolute(&ancestor.join(value)).map_err(io::Error::other)?);
                    }
                }

                if configuration.level.is_some() {
                    merged.level = configuration.level;
                }
            }
        }

        self.environments
            .insert(directory.to_owned(), result.clone());

        Ok(result)
    }

    fn project_aliases(&mut self, directory: &Path) -> io::Result<&BTreeMap<String, Alias>> {
        if !self.aliases.contains_key(directory) {
            let mut aliases = BTreeMap::new();

            let path = directory.join("instar.toml");

            if let Some(configured) = self
                .project(&path)?
                .and_then(|configuration| configuration.analyze.as_ref())
                .and_then(|configuration| configuration.aliases.as_ref())
            {
                let validation = serde_json::to_vec(&serde_json::json!({"aliases": &configured}))?;

                luau::aliases(&validation, false)
                    .map_err(|error| io::Error::other(format!("{}: {error}", path.display())))?;

                for (alias, target) in configured {
                    let key = alias.to_ascii_lowercase();

                    let value = target
                        .to_str()
                        .ok_or_else(|| io::Error::other("alias path requires UTF-8"))?
                        .to_owned();

                    if aliases
                        .insert(
                            key,
                            Alias {
                                name: alias.clone(),
                                configuration: path.clone(),
                                value,
                            },
                        )
                        .is_some()
                    {
                        return Err(io::Error::other(format!(
                            "{}: duplicate case-insensitive alias {alias}",
                            path.display()
                        )));
                    }
                }
            }

            self.aliases.insert(directory.to_owned(), aliases);
        }

        Ok(&self.aliases[directory])
    }

    pub(crate) fn alias_names(
        &mut self,
        from: &Path,
    ) -> io::Result<std::collections::BTreeSet<String>> {
        let directory = from
            .parent()
            .ok_or_else(|| io::Error::other("module has no parent directory"))?;

        self.configurations(from)?;
        let mut names = std::collections::BTreeSet::new();

        for ancestor in directory.ancestors() {
            names.extend(
                self.project_aliases(ancestor)?
                    .values()
                    .map(|alias| alias.name.clone()),
            );
        }

        for configuration in self
            .configurations
            .get_mut(directory)
            .expect("discovered directory")
        {
            if configuration.aliases.is_none() {
                configuration.aliases = Some(luau::aliases(&configuration.bytes, true)?);
            }

            names.extend(
                configuration
                    .aliases
                    .as_ref()
                    .expect("loaded aliases")
                    .keys()
                    .cloned(),
            );
        }

        Ok(names)
    }

    pub(super) fn alias(&mut self, from: &Path, name: &str) -> io::Result<Option<Alias>> {
        let directory = from
            .parent()
            .ok_or_else(|| io::Error::other("module has no parent directory"))?;

        self.configurations(from)?;

        for ancestor in directory.ancestors() {
            if let Some(alias) = self.project_aliases(ancestor)?.get(name) {
                return Ok(Some(alias.clone()));
            }

            if let Some(configuration) = self
                .configurations
                .get_mut(directory)
                .expect("discovered directory")
                .iter_mut()
                .find(|configuration| configuration.path.parent() == Some(ancestor))
            {
                if configuration.aliases.is_none() {
                    configuration.aliases =
                        Some(luau::aliases(&configuration.bytes, true).map_err(|error| {
                            io::Error::other(format!("{}: {error}", configuration.path.display()))
                        })?);
                }

                if let Some(value) = configuration.aliases.as_ref().and_then(|aliases| {
                    aliases
                        .iter()
                        .find(|(alias, _)| alias.eq_ignore_ascii_case(name))
                        .map(|(_, value)| value)
                }) {
                    return Ok(Some(Alias {
                        name: name.into(),
                        configuration: configuration.path.clone(),
                        value: value.clone(),
                    }));
                }
            }
        }

        Ok(None)
    }
}

impl Selection {
    /// # Errors
    /// Returns filesystem, encoding, configuration or glob failures.
    pub fn discover(path: &Path, scope: Scope) -> io::Result<Self> {
        let path = absolute(path).map_err(io::Error::other)?;

        let directory = path
            .parent()
            .ok_or_else(|| io::Error::other("source has no parent"))?;

        let mut selection = Self::default();

        for ancestor in directory.ancestors().collect::<Vec<_>>().into_iter().rev() {
            let configuration = ancestor.join("instar.toml");

            let text = match fs::read_to_string(&configuration) {
                Ok(text) => text,
                Err(error) if error.kind() == io::ErrorKind::NotFound => continue,

                Err(error) => {
                    return Err(io::Error::new(
                        error.kind(),
                        format!("{}: {error}", configuration.display()),
                    ));
                }
            };

            (|| {
                InstarConfig::parse(&text).map_err(io::Error::other)?;
                let value = toml_edit::de::from_str(&text).map_err(io::Error::other)?;

                selection.merge(&value, &configuration, scope)
            })()
            .map_err(|error| io::Error::other(format!("{}: {error}", configuration.display())))?;
        }

        Ok(selection)
    }
}

impl super::Configuration {
    /// # Errors
    /// Returns filesystem, encoding, configuration, or glob failures.
    pub fn discover(path: &Path, explicit: Option<&Path>) -> io::Result<Self> {
        Self::discover_with_selection(path, explicit, true)
    }

    /// Discover graft frontends without validating formatting selection.
    ///
    /// # Errors
    /// Returns filesystem, encoding, configuration, or graft failures.
    pub fn discover_frontends(path: &Path) -> io::Result<Self> {
        Self::discover_with_selection(path, None, false)
    }

    fn discover_with_selection(
        path: &Path,
        explicit: Option<&Path>,
        select: bool,
    ) -> io::Result<Self> {
        let path = crate::source::absolute(path).map_err(io::Error::other)?;
        let mut merged = serde_json::Map::new();
        let mut selection = Selection::default();
        let mut grafts = BTreeMap::new();

        let directory = path
            .parent()
            .ok_or_else(|| io::Error::other("source has no parent"))?;

        if explicit.is_none() {
            for ancestor in directory.ancestors().collect::<Vec<_>>().into_iter().rev() {
                let configuration = ancestor.join("instar.toml");

                match fs::read_to_string(&configuration) {
                    Ok(text) => {
                        merge(
                            &mut merged,
                            &text,
                            select.then_some(&mut selection),
                            &mut grafts,
                            &configuration,
                        )
                        .map_err(|error| {
                            io::Error::other(format!("{}: {error}", configuration.display()))
                        })?;
                    }

                    Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                    Err(error) => return Err(error),
                }
            }
        }

        if let Some(explicit) = explicit {
            let explicit = crate::source::absolute(explicit).map_err(io::Error::other)?;

            merge(
                &mut merged,
                &fs::read_to_string(&explicit)?,
                select.then_some(&mut selection),
                &mut grafts,
                &explicit,
            )?;
        }

        let grafts = grafts
            .into_iter()
            .map(|(name, (dependency, directory))| {
                crate::graft::Graft::load_with_configuration(
                    &dependency.resolve(&directory, &name)?,
                    &name,
                    dependency.configuration(),
                )
            })
            .collect::<io::Result<Vec<_>>>()?;

        let mut extensions = BTreeSet::new();

        for graft in &grafts {
            for extension in graft.extensions() {
                if !extensions.insert(extension) {
                    return Err(io::Error::other(format!(
                        "multiple grafts own .{extension}"
                    )));
                }
            }
        }

        Ok(Self {
            options: serde_json::from_value(merged.into()).map_err(io::Error::other)?,
            selection,
            grafts,
        })
    }
}

fn merge(
    merged: &mut serde_json::Map<String, serde_json::Value>,
    text: &str,
    selection: Option<&mut Selection>,
    grafts: &mut BTreeMap<String, (crate::graft::Dependency, PathBuf)>,
    configuration: &Path,
) -> io::Result<()> {
    let parsed = crate::configuration::InstarConfig::parse(text).map_err(io::Error::other)?;
    let mut value: serde_json::Value = toml_edit::de::from_str(text).map_err(io::Error::other)?;

    if let Some(selection) = selection {
        selection.merge(&value, configuration, Scope::Format)?;
    }

    for (name, dependency) in parsed.grafts.unwrap_or_default() {
        let directory = configuration
            .parent()
            .ok_or_else(|| io::Error::other("configuration has no parent"))?;

        grafts.insert(name, (dependency, directory.to_owned()));
    }

    value = value
        .get_mut("format")
        .map_or(serde_json::Value::Null, serde_json::Value::take);

    if let serde_json::Value::Object(fields) = value {
        crate::configuration::overlay(merged, fields);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discovery_reuses_configuration_snapshots_and_directory_settings() -> io::Result<()> {
        let directory = tempfile::tempdir()?;
        let root = directory.path();
        let nested = root.join("nested");
        fs::create_dir(&nested)?;

        fs::write(
            root.join("instar.toml"),
            "[analyze]\nmode = 'strict'\ndefinitions = ['shared.d.luau', 'shared.d.luau']\n[analyze.aliases]\nmodules = 'modules'\n[analyze.roblox]\nlevel = 'None'\n",
        )?;

        fs::write(
            nested.join("instar.toml"),
            "[analyze]\ndefinitions = []\n[analyze.roblox]\nsourcemap = 'map.json'\n",
        )?;

        let mut discovery = Discovery::default();
        let started = std::time::Instant::now();

        for index in 0..100 {
            let path = root.join(format!("source{index}.luau"));
            assert_eq!(discovery.definitions(&path)?, [root.join("shared.d.luau")]);
            let environment = discovery.roblox(&path)?.expect("environment");
            assert_eq!(environment.root, root);

            assert_eq!(
                environment.level,
                Some(crate::configuration::RobloxLevel::None)
            );

            assert_eq!(environment.sourcemap, None);
            let path = nested.join(format!("source{index}.luau"));
            assert_eq!(discovery.definitions(&path)?, Vec::<PathBuf>::new());
            let environment = discovery.roblox(&path)?.expect("environment");
            assert_eq!(environment.root, nested);
            assert_eq!(environment.sourcemap, Some(nested.join("map.json")));
        }

        eprintln!("configuration discovery: {:?}", started.elapsed());
        assert_eq!(discovery.projects.len(), 2);
        assert_eq!(discovery.definitions.len(), 2);
        assert_eq!(discovery.environments.len(), 2);

        let path = nested.join("main.luau");
        assert!(discovery.alias_names(&path)?.contains("modules"));
        let configurations = discovery.configurations(&path)?;

        assert!(
            configurations
                .iter()
                .any(|configuration| configuration.bytes == br#"{"languageMode":"strict"}"#)
        );

        fs::write(
            nested.join("instar.toml"),
            "[analyze]\ndefinitions = ['replacement.d.luau']\n[analyze.roblox]\n",
        )?;

        assert_eq!(discovery.definitions(&path)?, Vec::<PathBuf>::new());

        assert_eq!(
            discovery.roblox(&path)?.expect("environment").sourcemap,
            Some(nested.join("map.json"))
        );

        let mut refreshed = Discovery::default();

        assert_eq!(
            refreshed.definitions(&path)?,
            [nested.join("replacement.d.luau")]
        );

        let environment = refreshed.roblox(&path)?.expect("environment");
        assert_eq!(environment.root, nested);
        assert_eq!(environment.sourcemap, None);

        Ok(())
    }

    #[test]
    fn discovery_refreshes_missing_and_invalid_configurations() -> io::Result<()> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("main.luau");
        let configuration = directory.path().join("instar.toml");
        let mut discovery = Discovery::default();
        assert_eq!(discovery.roblox(&path)?, None);
        assert_eq!(discovery.definitions(&path)?, Vec::<PathBuf>::new());
        fs::write(&configuration, "[analyze.roblox]\n")?;
        assert_eq!(discovery.roblox(&path)?, None);
        assert!(Discovery::default().roblox(&path)?.is_some());

        fs::write(&configuration, "[analyze]\ndefinitions = 1\n")?;
        let mut invalid = Discovery::default();

        for error in [
            invalid.definitions(&path).unwrap_err(),
            invalid.roblox(&path).unwrap_err(),
        ] {
            assert!(
                error
                    .to_string()
                    .contains(&configuration.display().to_string()),
                "{error}"
            );
        }

        fs::write(&configuration, "[analyze]\ndefinitions = []\n")?;

        assert_eq!(
            Discovery::default().definitions(&path)?,
            Vec::<PathBuf>::new()
        );

        Ok(())
    }
}
