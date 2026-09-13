use crate::{configuration::InstarConfig, source::absolute};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs, io,
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
#[schemars(rename = "BuildSettings")]
/// Build inputs, outputs, transformations, and profile overrides.
pub struct Settings {
    /// Input files and directories relative to the build configuration. Required for directory output.
    pub inputs: Vec<PathBuf>,

    /// Output directory or bundle file relative to the build configuration. Required.
    pub output: Option<PathBuf>,

    /// Directory output retains project-relative paths; bundle output follows the entry's dependencies.
    pub shape: Shape,

    /// Bundle entry relative to the build configuration. Required for bundle output.
    pub entry: Option<PathBuf>,

    /// Require output syntax. Unset uses Roblox strings when Roblox is enabled, otherwise relative paths.
    pub target: Option<Target>,

    /// Exact require specifiers supplied by the deployment runtime rather than this build.
    pub external: Vec<String>,

    /// Replace reads of these unshadowed global names with scalar constants. Assignments to constants are rejected.
    pub constants: BTreeMap<String, Constant>,

    /// Remove type-only syntax, assertions, and const declarations while retaining Luau runtime syntax.
    pub lower: bool,

    /// Remove comments and unnecessary whitespace. Literal contents and token boundaries are preserved.
    pub minify: bool,

    /// Input extensions mapped to configured compiling graft names. A leading dot is omitted.
    pub languages: BTreeMap<String, String>,

    /// Named overrides for build configuration. Paths remain relative to this configuration.
    pub profiles: BTreeMap<String, Profile>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
#[schemars(rename = "BuildShape")]
/// Organization of compiled output artifacts.
pub enum Shape {
    /// Emit separate files using project-relative paths.
    #[default]
    Directory,

    /// Combine reachable modules into a single bundle.
    Bundle,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename = "BuildTarget")]
/// Runtime representation of rewritten require targets.
pub enum Target {
    /// Relative filesystem paths.
    Path,

    /// Roblox string require paths.
    RobloxString,

    /// Roblox instance expressions.
    RobloxInstance,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(untagged)]
#[schemars(rename = "BuildConstant")]
/// Scalar value substituted for an unshadowed global read.
pub enum Constant {
    /// A boolean literal.
    Boolean(bool),

    /// A numeric literal.
    Number(f64),

    /// A string literal.
    String(String),
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
#[schemars(rename = "BuildProfile")]
/// Named overrides applied to the base build settings.
pub struct Profile {
    /// Override the output destination.
    pub output: Option<PathBuf>,

    /// Override the bundle entry.
    pub entry: Option<PathBuf>,

    /// Override the output shape.
    pub shape: Option<Shape>,

    /// Override the require target.
    pub target: Option<Target>,

    /// Override constants by name.
    pub constants: BTreeMap<String, Constant>,

    /// Override type lowering.
    pub lower: Option<bool>,

    /// Override minification.
    pub minify: Option<bool>,
}

pub(super) struct Configuration {
    pub root: PathBuf,
    pub settings: Settings,
    pub files: BTreeMap<PathBuf, Vec<u8>>,
    pub grafts: BTreeMap<String, crate::graft::Graft>,
    pub project: Option<PathBuf>,
    pub sourcemap: Option<PathBuf>,
}

pub(super) fn discover(from: &Path, profile: Option<&str>) -> io::Result<Configuration> {
    let from = absolute(from).map_err(io::Error::other)?;

    if from.is_file() && from.file_name().and_then(|name| name.to_str()) != Some("instar.toml") {
        return Err(io::Error::other("build configuration must be instar.toml"));
    }

    let candidates = if from.is_file() {
        vec![from.clone()]
    } else {
        from.ancestors()
            .map(|directory| directory.join("instar.toml"))
            .collect()
    };

    let mut selected = None;
    let mut files = BTreeMap::new();

    for path in candidates {
        let bytes = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error),
        };

        let parsed = InstarConfig::parse(std::str::from_utf8(&bytes).map_err(io::Error::other)?)
            .map_err(io::Error::other)?;

        files.insert(path.clone(), bytes);

        if let Some(settings) = parsed.build {
            selected = Some((path, settings));
            break;
        }
    }

    let (path, settings) =
        selected.ok_or_else(|| io::Error::other("build requires an instar.toml build table"))?;

    let root = path
        .parent()
        .ok_or_else(|| io::Error::other("configuration has no parent"))?
        .to_owned();

    let settings = select(settings, profile)?;
    let mut manifests = BTreeMap::new();

    for parent in root.ancestors().collect::<Vec<_>>().into_iter().rev() {
        let path = parent.join("instar.toml");

        if path.is_file() {
            let bytes = fs::read(&path)?;

            let parsed =
                InstarConfig::parse(std::str::from_utf8(&bytes).map_err(io::Error::other)?)
                    .map_err(io::Error::other)?;

            files.insert(path, bytes);

            for (name, path) in parsed.grafts.unwrap_or_default() {
                manifests.insert(name, super::paths::absolute(&parent.join(path))?);
            }
        }
    }

    let mut grafts = BTreeMap::new();

    for (name, path) in manifests {
        let graft = crate::graft::Graft::load(&path, &name)?;
        files.insert(path, fs::read(graft.manifest())?);
        files.insert(graft.entry().to_owned(), fs::read(graft.entry())?);
        grafts.insert(name, graft);
    }

    for (extension, name) in &settings.languages {
        if extension.is_empty()
            || !extension.bytes().all(|byte| byte.is_ascii_alphanumeric())
            || matches!(extension.as_str(), "lua" | "luau")
            || !grafts.get(name).is_some_and(crate::graft::Graft::compiles)
        {
            return Err(io::Error::other(format!(
                "invalid build language or compiling graft: {extension} = {name}"
            )));
        }
    }

    let mut sources = crate::source::SourceStore::default();
    let mut resolver = crate::project::resolution::Resolver::new(&mut sources);
    let roblox = resolver.discovery.roblox(&path)?;

    for path in resolver.discovery.definitions(&path)? {
        files.insert(path.clone(), fs::read(path)?);
    }

    let (project, sourcemap) = if let Some(roblox) = roblox {
        for path in [&roblox.project, &roblox.sourcemap].into_iter().flatten() {
            files.insert(path.clone(), fs::read(path)?);
        }

        (roblox.project, roblox.sourcemap)
    } else {
        (None, None)
    };

    Ok(Configuration {
        root,
        settings,
        files,
        grafts,
        project,
        sourcemap,
    })
}

fn select(mut settings: Settings, profile: Option<&str>) -> io::Result<Settings> {
    if let Some(profile) = profile {
        let profile = settings
            .profiles
            .get(profile)
            .ok_or_else(|| io::Error::other(format!("unknown build profile: {profile}")))?
            .clone();

        if let Some(value) = profile.output {
            settings.output = Some(value);
        }

        if let Some(value) = profile.entry {
            settings.entry = Some(value);
        }

        if let Some(value) = profile.shape {
            settings.shape = value;
        }

        if let Some(value) = profile.target {
            settings.target = Some(value);
        }

        if let Some(value) = profile.lower {
            settings.lower = value;
        }

        if let Some(value) = profile.minify {
            settings.minify = value;
        }

        settings.constants.extend(profile.constants);
    }

    if settings.output.is_none()
        || (settings.shape == Shape::Directory && settings.inputs.is_empty())
        || (settings.shape == Shape::Bundle && settings.entry.is_none())
    {
        return Err(io::Error::other(
            "build requires output and directory inputs or a bundle entry",
        ));
    }

    for (name, constant) in &settings.constants {
        let tokens = vermis::tokenize(name.as_bytes().into());

        if !matches!(
            tokens.as_slice(),
            [
                vermis::Token {
                    kind: vermis::TokenKind::Name,
                    ..
                },
                vermis::Token {
                    kind: vermis::TokenKind::Eof,
                    ..
                }
            ]
        ) || matches!(name.as_str(), "require" | "script")
            || matches!(constant, Constant::Number(value) if !value.is_finite())
        {
            return Err(io::Error::other(format!("invalid build constant: {name}")));
        }
    }

    Ok(settings)
}
