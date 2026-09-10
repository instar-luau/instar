mod host;
mod layout;

use crate::format::Options;
use serde::Deserialize;
use std::{
    collections::BTreeMap,
    fs, io,
    path::{Component, Path, PathBuf},
};

const RESPONSE_LIMIT: usize = 64 * 1024 * 1024;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Runtime {
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
pub struct Graft {
    path: PathBuf,
    bytes: Vec<u8>,
    format: bool,
    lint: bool,
    configuration: BTreeMap<String, serde_json::Value>,
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

        let Runtime::Wasm = manifest.runtime;
        let bytes = fs::read(entry)?;
        host::Host::load(&bytes, manifest.format, manifest.lint, &manifest.configuration)?;

        Ok(Self {
            path: path.to_owned(),
            bytes,
            format: manifest.format,
            lint: manifest.lint,
            configuration: manifest.configuration,
        })
    }

    fn invoke(&self, hook: &str, source: &[u8]) -> io::Result<Vec<u8>> {
        let result = host::Host::load(&self.bytes, self.format, self.lint, &self.configuration)
            .and_then(|mut host| host.invoke(&format!("instar_{hook}"), source))
            .and_then(|reply| reply.ok_or_else(|| io::Error::other("graft hook is absent")));

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
            &self.invoke("format", source)?,
            options,
        )
    }

    /// # Errors
    /// Returns execution failures, malformed findings, or invalid source ranges.
    pub fn lint(&self, source: &[u8]) -> io::Result<Vec<Finding>> {
        if !self.lint {
            return Ok(Vec::new());
        }

        let reply = self.invoke("lint", source)?;
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
