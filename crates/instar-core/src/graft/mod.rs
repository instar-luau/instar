mod install;
mod layout;
mod luau;
mod native;
mod wasm;

pub use install::install;

use crate::configuration::format::Options;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use std::{
    collections::{BTreeMap, BTreeSet},
    fs, io,
    path::{Component, Path, PathBuf},
};

const RESPONSE_LIMIT: usize = 64 * 1024 * 1024;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
/// Execution environment selected by a graft manifest.
pub enum Runtime {
    /// Invoke a native executable through the JSON protocol.
    Native,

    /// Execute Luau code in an isolated virtual machine.
    Luau,

    /// Execute a WebAssembly module in the graft runtime.
    Wasm,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
/// Graft identity, entry point, exported hooks, and default configuration.
pub struct Manifest {
    /// Graft identifier used by project configuration.
    pub name: String,

    /// Graft protocol version.
    pub protocol: u32,

    /// Runtime responsible for executing the entry point.
    pub runtime: Runtime,

    /// Entry path relative to the manifest.
    #[serde(default)]
    pub entry: Option<PathBuf>,

    /// Whether the graft exports a formatting hook.
    #[serde(default)]
    pub format: bool,

    /// Whether the graft exports a lint hook.
    #[serde(default)]
    pub lint: bool,

    /// Whether the graft exports a compilation hook.
    #[serde(default)]
    pub compile: bool,

    /// Input extensions owned by this graft, without a leading dot.
    #[serde(default)]
    pub extensions: Vec<String>,

    /// Default settings passed to exported hooks.
    #[serde(default)]
    pub configuration: BTreeMap<String, serde_json::Value>,
}

impl Manifest {
    /// # Errors
    /// Returns malformed TOML or invalid graft metadata.
    pub fn parse(source: &str) -> io::Result<Self> {
        let manifest: Self = toml_edit::de::from_str(source).map_err(io::Error::other)?;
        manifest.validate()?;

        Ok(manifest)
    }

    fn read(path: &Path) -> io::Result<Self> {
        Self::parse(&fs::read_to_string(path)?)
    }

    fn validate(&self) -> io::Result<()> {
        component(&self.name)?;

        if self.protocol != 1 || !self.format && !self.lint && !self.compile {
            return Err(io::Error::other("graft protocol or hooks are invalid"));
        }

        let mut extensions = BTreeSet::new();

        if self.extensions.iter().any(|extension| {
            extension.is_empty()
                || matches!(extension.as_str(), "lua" | "luau")
                || !extension.bytes().all(|byte| byte.is_ascii_alphanumeric())
                || !extensions.insert(extension)
        }) || !extensions.is_empty() && !self.compile
        {
            return Err(io::Error::other("graft extensions are invalid"));
        }

        match (&self.runtime, &self.entry) {
            (Runtime::Native, Some(_)) => {
                return Err(io::Error::other("native grafts must not define an entry"));
            }

            (Runtime::Native, None) | (Runtime::Luau | Runtime::Wasm, Some(_)) => {}

            (Runtime::Luau | Runtime::Wasm, None) => {
                return Err(io::Error::other(
                    "Luau and WebAssembly grafts require an entry",
                ));
            }
        }

        if let Some(entry) = &self.entry
            && (entry.as_os_str().is_empty()
                || entry
                    .components()
                    .any(|part| !matches!(part, Component::Normal(_) | Component::CurDir)))
        {
            return Err(io::Error::other(
                "graft entry must remain inside its project directory",
            ));
        }

        Ok(())
    }
}

/// A local graft project or an installed GitHub release requirement.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(untagged, deny_unknown_fields)]
pub enum Dependency {
    /// A project directory relative to the declaring configuration.
    Local {
        /// Directory containing the graft project's graft.toml.
        path: PathBuf,
        /// Project settings merged over the graft's defaults.
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        configuration: BTreeMap<String, serde_json::Value>,
    },

    /// A GitHub project installed into Instar's graft cache.
    Remote {
        /// GitHub owner and repository separated by a slash.
        repo: String,
        /// Exact semantic version or an explicit compatible version requirement.
        version: String,
        /// Project settings merged over the graft's defaults.
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        configuration: BTreeMap<String, serde_json::Value>,
    },
}

fn component(value: &str) -> io::Result<()> {
    let stem = value
        .split('.')
        .next()
        .unwrap_or_default()
        .to_ascii_uppercase();

    if value.is_empty()
        || value == "."
        || value == ".."
        || value.ends_with('.')
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-_.".contains(&byte))
        || matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || (stem.len() == 4
            && (stem.starts_with("COM") || stem.starts_with("LPT"))
            && matches!(stem.as_bytes()[3], b'1'..=b'9'))
    {
        return Err(io::Error::other(format!(
            "invalid graft path component: {value}"
        )));
    }

    Ok(())
}

fn requirement(value: &str) -> io::Result<semver::VersionReq> {
    let value = if let Ok(version) = semver::Version::parse(value) {
        format!("={version}")
    } else {
        value.to_owned()
    };

    semver::VersionReq::parse(&value).map_err(io::Error::other)
}

fn cache() -> io::Result<PathBuf> {
    dirs::cache_dir()
        .map(|path| path.join("instar").join("grafts"))
        .ok_or_else(|| io::Error::other("cannot locate the graft cache directory"))
}

impl Dependency {
    pub(crate) fn validate(&self, name: &str) -> io::Result<()> {
        component(name)?;

        match self {
            Self::Local { path, .. } if path.as_os_str().is_empty() => {
                Err(io::Error::other("graft project path is empty"))
            }

            Self::Local { .. } => Ok(()),

            Self::Remote { repo, version, .. } => {
                let parts = repo.split('/').collect::<Vec<_>>();

                if parts.len() != 2 {
                    return Err(io::Error::other("graft repository must be owner/repo"));
                }

                for part in parts {
                    component(part)?;
                }

                requirement(version)?;

                Ok(())
            }
        }
    }

    pub(crate) fn resolve(&self, directory: &Path, name: &str) -> io::Result<PathBuf> {
        self.validate(name)?;

        match self {
            Self::Local { path, .. } => Ok(directory.join(path).join("graft.toml")),
            Self::Remote { .. } => self.cached(&cache()?, name),
        }
    }

    pub(crate) fn configuration(&self) -> &BTreeMap<String, serde_json::Value> {
        match self {
            Self::Local { configuration, .. } | Self::Remote { configuration, .. } => configuration,
        }
    }

    fn cached(&self, cache: &Path, name: &str) -> io::Result<PathBuf> {
        let Self::Remote { repo, version, .. } = self else {
            return Err(io::Error::other("expected a remote graft"));
        };

        let exact = semver::Version::parse(version).ok();
        let requirement = requirement(version)?;
        let directory = cache.join(repo).join(name);

        let entries = match fs::read_dir(&directory) {
            Ok(entries) => Some(entries),
            Err(error) if error.kind() == io::ErrorKind::NotFound => None,
            Err(error) => return Err(error),
        };

        let mut versions = Vec::new();

        for entry in entries.into_iter().flatten() {
            let entry = entry?;

            if entry.file_type()?.is_dir()
                && let Some(text) = entry.file_name().to_str()
                && let Ok(version) = semver::Version::parse(text)
                && requirement.matches(&version)
                && exact.as_ref().is_none_or(|exact| exact == &version)
            {
                versions.push((version, entry.path()));
            }
        }

        let path = versions
            .into_iter()
            .max()
            .map(|(_, path)| path)
            .ok_or_else(|| {
                io::Error::other(format!(
                    "graft {name} ({repo} {version}) is not cached; run instar graft install"
                ))
            })?
            .join("graft.toml");

        if Manifest::read(&path)?.name != name {
            return Err(io::Error::other(
                "cached graft identity does not match its directory",
            ));
        }

        Ok(path)
    }
}

#[derive(Clone, Debug)]
enum Artifact {
    Native(PathBuf),
    Luau(Vec<u8>),
    Wasm(Vec<u8>),
}

#[derive(Clone, Debug)]
/// A validated graft with its loaded artifact and resolved configuration.
pub struct Graft {
    path: PathBuf,
    entry: PathBuf,
    artifact: Artifact,
    format: bool,
    lint: bool,
    compile: bool,
    extensions: BTreeSet<String>,
    configuration: BTreeMap<String, serde_json::Value>,
}

#[derive(Serialize)]
struct Request<'value> {
    version: u32,
    hook: &'value str,

    #[serde(skip_serializing_if = "Option::is_none")]
    path: Option<&'value Path>,

    source: &'value str,
    configuration: &'value BTreeMap<String, serde_json::Value>,
    settings: Option<&'value Options>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
/// A graft lint finding expressed in source byte offsets.
pub struct Finding {
    /// Rule identifier local to the graft.
    pub rule: String,

    /// Explanation of the finding.
    pub message: String,

    /// Starting byte offset in the original source.
    pub start: usize,

    /// Exclusive ending byte offset in the original source.
    pub end: usize,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
/// Compiled Luau source and its declared dependency and mapping metadata.
pub struct Compilation {
    /// Compilation response protocol version.
    pub version: u32,

    /// Generated Luau source.
    pub source: String,

    /// Additional source dependencies declared by the compiler.
    pub dependencies: Vec<PathBuf>,

    /// Generated ranges mapped back to the input source.
    pub mappings: Vec<Mapping>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
/// A generated byte range mapped to an original source byte range.
pub struct Mapping {
    /// First byte in the generated range.
    pub start: usize,

    /// Exclusive end of the generated range.
    pub end: usize,

    /// First byte in the original range.
    pub original_start: usize,

    /// Exclusive end of the original range.
    pub original_end: usize,
}

impl Mapping {
    pub(crate) fn to_original(&self, offset: usize, inclusive_end: bool) -> Option<usize> {
        if offset < self.start || offset > self.end || !inclusive_end && offset == self.end {
            return None;
        }

        Some(
            self.original_start
                + (offset - self.start).min(self.original_end - self.original_start),
        )
    }

    pub(crate) fn to_generated(&self, offset: usize) -> Option<usize> {
        if self.original_start == self.original_end
            || offset < self.original_start
            || offset >= self.original_end
        {
            return None;
        }

        Some(self.start + (offset - self.original_start).min(self.end - self.start))
    }
}

impl Graft {
    #[must_use]
    /// Resolved path of the graft manifest.
    pub fn manifest(&self) -> &Path {
        &self.path
    }

    #[must_use]
    /// Resolved path of the graft entry point.
    pub fn entry(&self) -> &Path {
        &self.entry
    }

    #[must_use]
    /// Whether this graft exports a compilation hook.
    pub const fn compiles(&self) -> bool {
        self.compile
    }

    #[must_use]
    /// Whether this graft owns the input path's extension.
    pub fn owns(&self, path: &Path) -> bool {
        path.extension()
            .and_then(std::ffi::OsStr::to_str)
            .is_some_and(|extension| self.extensions.contains(extension))
    }

    #[must_use]
    /// Whether this graft owns any input extensions.
    pub fn is_frontend(&self) -> bool {
        !self.extensions.is_empty()
    }

    pub(crate) fn extensions(&self) -> impl Iterator<Item = &str> {
        self.extensions.iter().map(String::as_str)
    }

    /// # Errors
    /// Returns execution, syntax, dependency, or source mapping errors.
    pub fn compile(&self, source: &[u8]) -> io::Result<Compilation> {
        self.compile_path(None, source)
    }

    pub(crate) fn compile_at(&self, path: &Path, source: &[u8]) -> io::Result<Compilation> {
        self.compile_path(Some(path), source)
    }

    fn compile_path(&self, path: Option<&Path>, source: &[u8]) -> io::Result<Compilation> {
        if !self.compile {
            return Err(io::Error::other("graft exports no compile hook"));
        }

        let result: Compilation =
            serde_json::from_slice(&self.invoke("compile", path, source, None)?)
                .map_err(io::Error::other)?;

        let original = std::str::from_utf8(source).map_err(io::Error::other)?;

        if result.version != 1
            || result
                .dependencies
                .iter()
                .any(|path| path.as_os_str().is_empty())
        {
            return Err(io::Error::other("invalid graft compilation response"));
        }

        let mut previous = 0;
        let mut previous_original = 0;

        for mapping in &result.mappings {
            if mapping.start < previous
                || mapping.start == mapping.end
                || mapping.original_start < previous_original
                || mapping.original_start == mapping.original_end
                || result.source.get(mapping.start..mapping.end).is_none()
                || original
                    .get(mapping.original_start..mapping.original_end)
                    .is_none()
            {
                return Err(io::Error::other("invalid graft compilation mapping"));
            }

            previous = mapping.end;
            previous_original = mapping.original_end;
        }

        if !vermis::parse(result.source.as_bytes().into())
            .diagnostics
            .is_empty()
        {
            return Err(io::Error::other("graft produced invalid Luau"));
        }

        Ok(result)
    }

    /// # Errors
    /// Returns invalid manifests, unsupported versions, or inaccessible artifacts.
    pub fn load(path: &Path, name: &str) -> io::Result<Self> {
        Self::load_with_configuration(path, name, &BTreeMap::new())
    }

    pub(crate) fn load_with_configuration(
        path: &Path,
        name: &str,
        project_configuration: &BTreeMap<String, serde_json::Value>,
    ) -> io::Result<Self> {
        let manifest = Manifest::read(path)?;

        if manifest.name != name {
            return Err(io::Error::other(format!(
                "{}: graft name does not match {name}",
                path.display()
            )));
        }

        let directory = path
            .parent()
            .ok_or_else(|| io::Error::other("graft manifest has no parent"))?
            .canonicalize()?;

        let entry = match manifest.entry {
            Some(entry) => directory.join(entry).canonicalize()?,
            None => native::entry(&directory).canonicalize()?,
        };

        if !entry.starts_with(&directory) || !entry.is_file() {
            return Err(io::Error::other(
                "graft entry must be a file inside its manifest directory",
            ));
        }

        let mut configuration = manifest
            .configuration
            .clone()
            .into_iter()
            .collect::<serde_json::Map<_, _>>();

        crate::configuration::overlay(
            &mut configuration,
            project_configuration.clone().into_iter().collect(),
        );

        let configuration = configuration.into_iter().collect();

        let artifact = match manifest.runtime {
            Runtime::Native => {
                native::validate(&entry)?;

                Artifact::Native(entry.clone())
            }

            Runtime::Luau => {
                let bytes = fs::read(&entry)?;
                luau::validate(&bytes, manifest.format, manifest.lint, manifest.compile)?;

                Artifact::Luau(bytes)
            }

            Runtime::Wasm => {
                let bytes = fs::read(&entry)?;

                wasm::Host::load(
                    &bytes,
                    manifest.format,
                    manifest.lint,
                    manifest.compile,
                    &configuration,
                )?;

                Artifact::Wasm(bytes)
            }
        };

        Ok(Self {
            path: path.to_owned(),
            entry,
            artifact,
            format: manifest.format,
            lint: manifest.lint,
            compile: manifest.compile,
            extensions: manifest.extensions.into_iter().collect(),
            configuration,
        })
    }

    fn invoke(
        &self,
        hook: &str,
        path: Option<&Path>,
        source: &[u8],
        settings: Option<&Options>,
    ) -> io::Result<Vec<u8>> {
        let request = Request {
            version: 1,
            hook,
            path,
            source: std::str::from_utf8(source).map_err(io::Error::other)?,
            configuration: &self.configuration,
            settings,
        };

        let result = match &self.artifact {
            Artifact::Native(entry) => native::invoke(entry, &request),

            Artifact::Luau(bytes) => {
                luau::invoke(bytes, &request, self.format, self.lint, self.compile)
            }

            Artifact::Wasm(bytes) => wasm::Host::load(
                bytes,
                self.format,
                self.lint,
                self.compile,
                &self.configuration,
            )
            .and_then(|mut host| host.invoke(&format!("instar_{hook}"), source))
            .and_then(|reply| reply.ok_or_else(|| io::Error::other("graft hook is absent"))),
        };

        result.map_err(|error| io::Error::other(format!("{}: {error}", self.path.display())))
    }

    /// # Errors
    /// Returns execution failures, malformed layouts, or unsafe formatting output.
    pub fn format(&self, source: &[u8], options: &Options) -> io::Result<Vec<u8>> {
        self.format_path(None, source, options)
    }

    pub(crate) fn format_at(
        &self,
        path: &Path,
        source: &[u8],
        options: &Options,
    ) -> io::Result<Vec<u8>> {
        self.format_path(Some(path), source, options)
    }

    fn format_path(
        &self,
        path: Option<&Path>,
        source: &[u8],
        options: &Options,
    ) -> io::Result<Vec<u8>> {
        if !self.format {
            return Ok(source.to_vec());
        }

        layout::format(
            source,
            &self.invoke("format", path, source, Some(options))?,
            options,
            !self.is_frontend(),
        )
    }

    /// # Errors
    /// Returns execution failures, malformed findings, or invalid source ranges.
    pub fn lint(&self, source: &[u8]) -> io::Result<Vec<Finding>> {
        self.lint_path(None, source)
    }

    pub(crate) fn lint_at(&self, path: &Path, source: &[u8]) -> io::Result<Vec<Finding>> {
        self.lint_path(Some(path), source)
    }

    fn lint_path(&self, path: Option<&Path>, source: &[u8]) -> io::Result<Vec<Finding>> {
        if !self.lint {
            return Ok(Vec::new());
        }

        let reply = self.invoke("lint", path, source, None)?;
        let findings: Vec<Finding> = serde_json::from_slice(&reply).map_err(io::Error::other)?;
        let source = std::str::from_utf8(source).map_err(io::Error::other)?;

        for finding in &findings {
            if finding.rule.is_empty()
                || finding.message.is_empty()
                || source.get(finding.start..finding.end).is_none()
            {
                return Err(io::Error::other("graft returned an invalid finding"));
            }
        }

        Ok(findings)
    }
}
