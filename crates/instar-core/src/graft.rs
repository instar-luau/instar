mod host;
mod layout;
mod native;

use crate::format::Options;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs, io,
    path::{Component, Path, PathBuf},
};

const RESPONSE_LIMIT: usize = 64 * 1024 * 1024;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Runtime {
    Native,
    Wasm,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub name: String,
    pub version: u32,
    pub runtime: Runtime,
    pub entry: PathBuf,

    #[serde(default)]
    pub format: bool,

    #[serde(default)]
    pub lint: bool,

    #[serde(default)]
    pub configuration: BTreeMap<String, serde_json::Value>,
}

#[derive(Debug, Clone)]
enum Artifact {
    Native(PathBuf),
    Wasm(Vec<u8>),
}

#[derive(Debug, Clone)]
pub struct Graft {
    path: PathBuf,
    artifact: Artifact,
    format: bool,
    lint: bool,
    configuration: BTreeMap<String, serde_json::Value>,
}

#[derive(Serialize)]
struct Request<'value> {
    version: u32,
    hook: &'value str,
    source: &'value str,
    configuration: &'value BTreeMap<String, serde_json::Value>,
    settings: Option<&'value Options>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Finding {
    pub rule: String,
    pub message: String,
    pub start: usize,
    pub end: usize,
}

impl Graft {
    /// # Errors
    /// Returns invalid manifests, unsupported versions, or inaccessible artifacts.
    pub fn load(path: &Path, name: &str) -> io::Result<Self> {
        let manifest: Manifest =
            toml_edit::de::from_str(&fs::read_to_string(path)?).map_err(io::Error::other)?;

        if manifest.name != name || manifest.version != 1 || !manifest.format && !manifest.lint {
            return Err(io::Error::other(format!(
                "{}: graft name, version, or hooks are invalid",
                path.display()
            )));
        }

        if manifest.entry.as_os_str().is_empty()
            || manifest
                .entry
                .components()
                .any(|component| !matches!(component, Component::Normal(_) | Component::CurDir))
        {
            return Err(io::Error::other(
                "graft entry must remain inside its manifest directory",
            ));
        }

        let directory = path
            .parent()
            .ok_or_else(|| io::Error::other("graft manifest has no parent"))?
            .canonicalize()?;

        let entry = directory.join(&manifest.entry).canonicalize()?;

        if !entry.starts_with(&directory) || !entry.is_file() {
            return Err(io::Error::other(
                "graft entry must be a file inside its manifest directory",
            ));
        }

        let artifact = match manifest.runtime {
            Runtime::Native => {
                native::validate(&entry)?;

                Artifact::Native(entry)
            }

            Runtime::Wasm => {
                let bytes = fs::read(entry)?;

                host::Host::load(
                    &bytes,
                    manifest.format,
                    manifest.lint,
                    &manifest.configuration,
                )?;

                Artifact::Wasm(bytes)
            }
        };

        Ok(Self {
            path: path.to_owned(),
            artifact,
            format: manifest.format,
            lint: manifest.lint,
            configuration: manifest.configuration,
        })
    }

    fn invoke(&self, hook: &str, source: &[u8], settings: Option<&Options>) -> io::Result<Vec<u8>> {
        let request = Request {
            version: 1,
            hook,
            source: std::str::from_utf8(source).map_err(io::Error::other)?,
            configuration: &self.configuration,
            settings,
        };

        let result = match &self.artifact {
            Artifact::Native(entry) => native::invoke(entry, &request),

            Artifact::Wasm(bytes) => {
                host::Host::load(bytes, self.format, self.lint, &self.configuration)
                    .and_then(|mut host| host.invoke(&format!("instar_{hook}"), source))
                    .and_then(|reply| reply.ok_or_else(|| io::Error::other("graft hook is absent")))
            }
        };

        result.map_err(|error| io::Error::other(format!("{}: {error}", self.path.display())))
    }

    /// # Errors
    /// Returns execution failures, malformed layouts, or unsafe formatting output.
    pub fn format(&self, source: &[u8], options: &Options) -> io::Result<Vec<u8>> {
        if !self.format {
            return Ok(source.to_vec());
        }

        layout::format(
            source,
            &self.invoke("format", source, Some(options))?,
            options,
        )
    }

    /// # Errors
    /// Returns execution failures, malformed findings, or invalid source ranges.
    pub fn lint(&self, source: &[u8]) -> io::Result<Vec<Finding>> {
        if !self.lint {
            return Ok(Vec::new());
        }

        let reply = self.invoke("lint", source, None)?;
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
