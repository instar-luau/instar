mod bundle;

/// Build settings and profile overrides.
pub mod configuration;

mod graph;
mod mapping;
mod output;
mod paths;
mod project;
mod rules;
mod syntax;
mod transform;

use crate::{
    analysis,
    source::{Source, SourceStore},
};

use configuration::{Configuration, Shape, Target};
pub use graph::{Classification, Dependency};
use mapping::Text;
pub use output::Outcome;
use serde::Serialize;

use std::{
    collections::{BTreeMap, BTreeSet},
    fs, io,
    path::{Path, PathBuf},
    sync::Arc,
};

#[derive(Clone, Debug, Serialize)]
/// A build finding associated with a source byte range.
pub struct Diagnostic {
    /// Source containing the finding.
    pub path: PathBuf,

    /// Starting byte offset.
    pub start: usize,

    /// Exclusive ending byte offset.
    pub end: usize,

    /// Whether this finding prevents publication.
    pub error: bool,

    /// Explanation of the finding.
    pub message: String,
}

#[derive(Clone, Debug, Serialize)]
/// A source module and its classified require dependencies.
pub struct Module {
    /// Logical source path of the module.
    pub path: PathBuf,

    /// Require dependencies discovered in this module.
    pub dependencies: Vec<Dependency>,
}

#[derive(Clone, Debug, Serialize)]
/// Destination and size of a prepared output artifact.
pub struct Artifact {
    /// Artifact path relative to the publication directory.
    pub path: PathBuf,

    /// Length of the prepared contents in bytes.
    pub bytes: usize,
}

#[derive(Clone, Serialize)]
/// Prepared build outputs and the input snapshots required to publish them.
pub struct Plan {
    /// Absolute build project root.
    pub root: PathBuf,

    /// Requested output directory or bundle path.
    pub output: PathBuf,

    /// Directory or bundle output organization.
    pub shape: Shape,

    /// Runtime require representation.
    pub target: Target,

    /// Selected named build profile, if any.
    pub profile: Option<String>,

    /// Transformation stages selected for this build.
    pub stages: Vec<String>,

    /// Modules included in the dependency graph.
    pub modules: Vec<Module>,

    /// Prepared output destinations and byte counts.
    pub artifacts: Vec<Artifact>,

    /// Findings collected while constructing the plan.
    pub diagnostics: Vec<Diagnostic>,

    #[serde(skip)]
    prepared: BTreeMap<PathBuf, Vec<u8>>,

    #[serde(skip)]
    snapshots: BTreeMap<PathBuf, Vec<u8>>,

    #[serde(skip)]
    directory: PathBuf,

    #[serde(skip)]
    state: PathBuf,
}

impl Plan {
    #[must_use]
    /// Whether build findings prevent publication.
    pub fn has_errors(&self) -> bool {
        self.diagnostics.iter().any(|diagnostic| diagnostic.error)
    }

    #[must_use]
    /// Prepared contents for an artifact path, without publishing it.
    pub fn contents(&self, path: &Path) -> Option<&[u8]> {
        self.prepared.get(path).map(Vec::as_slice)
    }

    /// # Errors
    /// Returns serialization errors for the build plan.
    pub fn json(&self) -> io::Result<String> {
        serde_json::to_string_pretty(self).map_err(io::Error::other)
    }

    /// # Errors
    /// Returns build diagnostics, changed inputs, ownership violations, or publication errors.
    pub fn publish(&self) -> io::Result<Outcome> {
        output::publish(self)
    }

    #[must_use]
    /// Whether prepared contents, input snapshots, and output destinations match.
    pub fn equivalent(&self, other: &Self) -> bool {
        self.prepared == other.prepared
            && self.snapshots == other.snapshots
            && self.output == other.output
    }

    #[must_use]
    /// Paths whose snapshots must remain unchanged before publication.
    pub fn inputs(&self) -> Vec<PathBuf> {
        self.snapshots.keys().cloned().collect()
    }
}

#[derive(Clone)]
struct Cached {
    input: Vec<u8>,
    configuration: Vec<u8>,
    dependencies: BTreeMap<PathBuf, Vec<u8>>,
    text: Text,
}

#[derive(Default)]
/// Reusable compilation state for build plans and watch rebuilds.
pub struct Session {
    sources: SourceStore,
    analysis: analysis::Session,
    version: i32,
    compiled: BTreeMap<PathBuf, Cached>,
}

struct Graph {
    modules: BTreeMap<PathBuf, graph::Module>,
    diagnostics: Vec<Diagnostic>,
}

struct Inputs {
    snapshots: BTreeMap<PathBuf, Vec<u8>>,
    key: Vec<u8>,
    texts: BTreeMap<PathBuf, Text>,
    originals: BTreeMap<PathBuf, String>,
    prepared: BTreeMap<PathBuf, Vec<u8>>,
    virtuals: BTreeMap<PathBuf, PathBuf>,
}

impl Session {
    /// # Errors
    /// Returns invalid configuration, inputs, transformations, or graph construction errors without publishing outputs.
    pub fn plan(&mut self, from: &Path, profile: Option<&str>) -> io::Result<Plan> {
        self.sources = SourceStore::default();
        let mut configuration = configuration::discover(from, profile)?;

        let (output, directory, state) = Self::destination(&configuration)?;

        let mut inputs = self.inputs(&mut configuration, &output)?;

        let entry = configuration
            .settings
            .entry
            .as_ref()
            .map(|path| {
                paths::absolute(&configuration.root.join(path))
                    .map(|path| logical(&path, &configuration))
            })
            .transpose()?;

        let origin = entry
            .as_ref()
            .or_else(|| inputs.texts.keys().next())
            .cloned()
            .unwrap_or_else(|| configuration.root.join("source.luau"));

        self.analysis.refresh();
        let environment = self.analysis.environment(&mut self.sources, &origin)?;

        let target = configuration
            .settings
            .target
            .unwrap_or(if environment.enabled {
                Target::RobloxString
            } else {
                Target::Path
            });

        if target != Target::Path && !environment.enabled {
            return Err(io::Error::other(
                "Roblox build targets require the roblox opt-in",
            ));
        }

        let pending = match configuration.settings.shape {
            Shape::Directory => inputs.texts.keys().cloned().collect::<Vec<_>>(),

            Shape::Bundle => vec![
                entry
                    .clone()
                    .ok_or_else(|| io::Error::other("bundle entry missing"))?,
            ],
        };

        let graph = self.graph(&configuration, &mut inputs, &environment, target, pending)?;

        Self::artifacts(
            &configuration,
            &mut inputs,
            &graph,
            &environment,
            target,
            entry.as_deref(),
            (&output, &directory),
        )?;

        self.compiled
            .retain(|path, _| inputs.snapshots.contains_key(path));

        let mut stages = vec!["compile".into(), "constants".into()];

        if configuration.settings.rules != configuration::Rules::default() {
            stages.push("rules".into());
        }

        stages.extend(["resolve".into(), "validate".into(), "rewrite".into()]);

        if configuration.settings.shape == Shape::Bundle {
            stages.push("bundle".into());
        }

        if configuration.settings.lower || configuration.settings.rules.remove_types {
            stages.push("lower".into());
        }

        if configuration.settings.minify {
            stages.push("minify".into());
        }

        stages.push("map".into());

        Ok(Plan {
            root: configuration.root,
            output,
            shape: configuration.settings.shape,
            target,
            profile: profile.map(str::to_owned),
            stages,
            modules: graph
                .modules
                .into_iter()
                .map(|(path, module)| Module {
                    path,
                    dependencies: module.dependencies,
                })
                .collect(),
            artifacts: inputs
                .prepared
                .iter()
                .map(|(path, bytes)| Artifact {
                    path: path.clone(),
                    bytes: bytes.len(),
                })
                .collect(),
            diagnostics: graph.diagnostics,
            prepared: inputs.prepared,
            snapshots: inputs.snapshots,
            directory,
            state,
        })
    }

    fn destination(configuration: &Configuration) -> io::Result<(PathBuf, PathBuf, PathBuf)> {
        let output = paths::absolute(
            &configuration.root.join(
                configuration
                    .settings
                    .output
                    .as_ref()
                    .ok_or_else(|| io::Error::other("output missing"))?,
            ),
        )?;

        let (directory, state) = match configuration.settings.shape {
            Shape::Directory => (output.clone(), PathBuf::from(".instar")),

            Shape::Bundle => (
                output
                    .parent()
                    .ok_or_else(|| io::Error::other("bundle output has no parent"))?
                    .to_owned(),
                PathBuf::from(".instar").join(
                    output
                        .file_name()
                        .ok_or_else(|| io::Error::other("bundle output has no filename"))?,
                ),
            ),
        };

        Ok((output, directory, state))
    }

    fn inputs(&mut self, configuration: &mut Configuration, output: &Path) -> io::Result<Inputs> {
        let inputs = inputs(configuration, output)?;
        let mut snapshots = configuration.files.clone();

        let key = serde_json::to_vec(&(&configuration.settings, &configuration.files))
            .map_err(io::Error::other)?;

        let mut texts = BTreeMap::new();
        let mut originals = BTreeMap::new();
        let mut prepared = BTreeMap::new();
        let mut virtuals = BTreeMap::new();

        for path in &inputs {
            let bytes = fs::read(path)?;
            snapshots.insert(path.clone(), bytes.clone());

            if language(path, configuration) {
                let logical = logical(path, configuration);

                if virtuals.insert(logical.clone(), path.clone()).is_some() {
                    return Err(io::Error::other("compilation destination collision"));
                }

                originals.insert(
                    path.clone(),
                    String::from_utf8(snapshots[path].clone()).map_err(io::Error::other)?,
                );

                if configuration.settings.shape == Shape::Bundle && &logical != path {
                    self.snapshot(&logical, "return nil")?;
                } else {
                    let text = self.compile(path, bytes, configuration, &key, &mut snapshots)?;
                    self.snapshot(&logical, &text.text)?;
                    texts.insert(logical, text);
                }
            } else if configuration.settings.shape == Shape::Directory
                && Some(path) != configuration.project.as_ref()
            {
                insert(
                    &mut prepared,
                    path.strip_prefix(&configuration.root)
                        .map_err(io::Error::other)?
                        .to_owned(),
                    bytes,
                )?;
            }
        }

        if configuration.has_frontends() {
            for path in [&configuration.project, &configuration.sourcemap]
                .into_iter()
                .flatten()
            {
                self.snapshot(path, &project::compile(configuration, path)?)?;
            }
        }

        Ok(Inputs {
            snapshots,
            key,
            texts,
            originals,
            prepared,
            virtuals,
        })
    }

    fn graph(
        &mut self,
        configuration: &Configuration,
        inputs: &mut Inputs,
        environment: &crate::roblox::Environment,
        target: Target,
        mut pending: Vec<PathBuf>,
    ) -> io::Result<Graph> {
        let Inputs {
            snapshots,
            key,
            texts,
            originals,
            virtuals,
            ..
        } = inputs;

        let mut modules = BTreeMap::new();
        let mut diagnostics = Vec::new();

        while let Some(path) = pending.pop() {
            if modules.contains_key(&path) {
                continue;
            }

            let text = if let Some(text) = texts.remove(&path) {
                text
            } else {
                if !path.starts_with(&configuration.root) {
                    return Err(io::Error::other(format!(
                        "dependency is outside the build project: {}",
                        path.display()
                    )));
                }

                let original = virtuals.get(&path).unwrap_or(&path);

                paths::safe(
                    &configuration.root,
                    original
                        .strip_prefix(&configuration.root)
                        .map_err(io::Error::other)?,
                )?;

                let bytes = fs::read(original)?;

                originals.insert(
                    original.clone(),
                    String::from_utf8(bytes.clone()).map_err(io::Error::other)?,
                );

                snapshots.insert(original.clone(), bytes.clone());

                self.compile(original, bytes, configuration, key, snapshots)?
            };

            let mut module = self.module(&path, &text, configuration, environment)?;

            module.dependencies =
                graph::dependencies(&module, environment, &configuration.settings.external)?;

            diagnostics.extend(graph::validate(
                &module,
                environment,
                configuration.settings.shape,
                target,
            ));

            pending.extend(
                module
                    .dependencies
                    .iter()
                    .filter_map(|dependency| dependency.target.clone()),
            );

            modules.insert(path, module);
        }

        for path in modules.keys() {
            for parent in path.ancestors().skip(1) {
                for name in ["instar.toml", ".luaurc", ".config.luau", "config.luau"] {
                    let path = parent.join(name);

                    if path.is_file() {
                        snapshots.insert(path.clone(), fs::read(path)?);
                    }
                }
            }
        }

        Ok(Graph {
            modules,
            diagnostics,
        })
    }

    fn artifacts(
        configuration: &Configuration,
        inputs: &mut Inputs,
        graph: &Graph,
        environment: &crate::roblox::Environment,
        target: Target,
        entry: Option<&Path>,
        (output, directory): (&Path, &Path),
    ) -> io::Result<()> {
        let Graph {
            modules,
            diagnostics,
        } = graph;

        let Inputs {
            prepared,
            snapshots,
            originals,
            ..
        } = inputs;

        let destinations = modules
            .keys()
            .map(|path| {
                Ok((
                    path.clone(),
                    directory.join(
                        path.strip_prefix(&configuration.root)
                            .map_err(io::Error::other)?,
                    ),
                ))
            })
            .collect::<io::Result<BTreeMap<_, _>>>()?;

        if !diagnostics.iter().any(|diagnostic| diagnostic.error) {
            match configuration.settings.shape {
                Shape::Directory => {
                    for (path, module) in modules {
                        let text =
                            graph::rewrite(module, &destinations, environment, target, None)?;

                        let path = path
                            .strip_prefix(&configuration.root)
                            .map_err(io::Error::other)?
                            .to_owned();

                        emit(prepared, path, text, configuration, originals)?;
                    }

                    if let Some(path) = &configuration.project {
                        project::emit(configuration, path, prepared, snapshots)?;
                    }
                }

                Shape::Bundle => {
                    let text = bundle::emit(
                        modules,
                        &configuration.root,
                        entry.ok_or_else(|| io::Error::other("bundle entry missing"))?,
                        environment,
                        target,
                    )?;

                    emit(
                        prepared,
                        PathBuf::from(
                            output
                                .file_name()
                                .ok_or_else(|| io::Error::other("bundle filename missing"))?,
                        ),
                        text,
                        configuration,
                        originals,
                    )?;
                }
            }
        }

        Ok(())
    }

    fn snapshot(&mut self, path: &Path, text: &str) -> io::Result<Arc<Source>> {
        if self.sources.is_open(path).map_err(io::Error::other)? {
            let source = self.sources.read(path).map_err(io::Error::other)?;

            if source.bytes() == text.as_bytes() {
                return Ok(source);
            }

            self.version = self
                .version
                .checked_add(1)
                .ok_or_else(|| io::Error::other("build snapshot version exhausted"))?;

            self.analysis.change(path);

            self.sources
                .update(&source, self.version, text)
                .map_err(io::Error::other)
        } else {
            self.version = self
                .version
                .checked_add(1)
                .ok_or_else(|| io::Error::other("build snapshot version exhausted"))?;

            self.analysis.change(path);

            self.sources
                .open(path, self.version, text)
                .map_err(io::Error::other)
        }
    }

    fn module(
        &mut self,
        path: &Path,
        text: &Text,
        configuration: &Configuration,
        environment: &crate::roblox::Environment,
    ) -> io::Result<graph::Module> {
        transform::validate(&text.text)?;
        let source = self.snapshot(path, &text.text)?;
        let document = syntax::parse(&mut self.analysis, &mut self.sources, path)?;
        let transformed = transform::constants(text, &source, &document, &configuration.settings)?;

        let document = if transformed.text == text.text {
            document
        } else {
            self.snapshot(path, &transformed.text)?;

            syntax::parse(&mut self.analysis, &mut self.sources, path)?
        };

        let source = self.snapshot(path, &transformed.text)?;

        let transformed = rules::apply(
            &transformed,
            &source,
            &document,
            &configuration.settings.rules,
            rules::Project {
                path,
                root: &configuration.root,
                environment,
                files: &configuration.files,
            },
        )?;

        let document = if transformed.text == source.text().map_err(io::Error::other)? {
            document
        } else {
            self.snapshot(path, &transformed.text)?;

            syntax::parse(&mut self.analysis, &mut self.sources, path)?
        };

        let text = transformed;
        let source = self.snapshot(path, &text.text)?;

        Ok(graph::Module {
            source,
            text,
            document,
            dependencies: Vec::new(),
        })
    }

    fn compile(
        &mut self,
        path: &Path,
        input: Vec<u8>,
        configuration: &Configuration,
        key: &[u8],
        snapshots: &mut BTreeMap<PathBuf, Vec<u8>>,
    ) -> io::Result<Text> {
        if let Some(cached) = self
            .compiled
            .get(path)
            .filter(|cached| cached.input == input && cached.configuration == key)
        {
            let mut valid = true;

            for (path, bytes) in &cached.dependencies {
                if fs::read(path)? != *bytes {
                    valid = false;
                    break;
                }
            }

            if valid {
                snapshots.extend(cached.dependencies.clone());

                return Ok(cached.text.clone());
            }
        }

        let mut dependencies = BTreeMap::new();

        let text = if let Some(graft) = configuration.frontend(path) {
            let compilation = graft.compile(&input)?;

            for dependency in compilation.dependencies {
                let path = paths::absolute(
                    &path
                        .parent()
                        .ok_or_else(|| io::Error::other("source has no parent"))?
                        .join(dependency),
                )?;

                if !path.starts_with(&configuration.root) {
                    return Err(io::Error::other(
                        "graft dependency is outside the build project",
                    ));
                }

                paths::safe(
                    &configuration.root,
                    path.strip_prefix(&configuration.root)
                        .map_err(io::Error::other)?,
                )?;

                dependencies.insert(path.clone(), fs::read(path)?);
            }

            Text {
                segments: compilation
                    .mappings
                    .into_iter()
                    .map(|range| mapping::Segment {
                        linear: compilation.source.as_bytes().get(range.start..range.end)
                            == input.get(range.original_start..range.original_end),
                        range,
                        source: path.to_owned(),
                    })
                    .collect(),
                text: compilation.source,
            }
        } else {
            let text = String::from_utf8(input.clone()).map_err(io::Error::other)?;

            Text::original(path, text)
        };

        snapshots.extend(dependencies.clone());

        self.compiled.insert(
            path.to_owned(),
            Cached {
                input,
                configuration: key.to_vec(),
                dependencies,
                text: text.clone(),
            },
        );

        Ok(text)
    }
}

/// # Errors
/// Returns inaccessible input or configuration errors. Reads current inputs and previously resolved dependencies.
pub fn observe(
    from: &Path,
    profile: Option<&str>,
    dependencies: &[PathBuf],
) -> io::Result<BTreeMap<PathBuf, Vec<u8>>> {
    let mut configuration = configuration::discover(from, profile)?;

    let output = paths::absolute(
        &configuration.root.join(
            configuration
                .settings
                .output
                .as_ref()
                .ok_or_else(|| io::Error::other("output missing"))?,
        ),
    )?;

    let inputs = inputs(&mut configuration, &output)?;
    let mut observed = configuration.files;

    for path in inputs.iter().chain(dependencies) {
        match fs::read(path) {
            Ok(bytes) => {
                observed.insert(path.clone(), bytes);
            }

            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }

        if !path.starts_with(&configuration.root) {
            continue;
        }

        for ancestor in path.ancestors().skip(1) {
            for name in ["instar.toml", ".luaurc", ".config.luau", "config.luau"] {
                let path = ancestor.join(name);

                if path.is_file() {
                    observed.insert(path.clone(), fs::read(path)?);
                }
            }
        }
    }

    Ok(observed)
}

fn language(path: &Path, configuration: &Configuration) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| matches!(extension, "lua" | "luau"))
        || configuration.frontend(path).is_some()
}

fn logical(path: &Path, configuration: &Configuration) -> PathBuf {
    if configuration.frontend(path).is_some() {
        path.with_extension("luau")
    } else {
        path.to_owned()
    }
}

fn insert(
    artifacts: &mut BTreeMap<PathBuf, Vec<u8>>,
    path: PathBuf,
    bytes: Vec<u8>,
) -> io::Result<()> {
    paths::relative(&path)?;

    if path.starts_with(".instar")
        || artifacts.keys().any(|other| {
            other == &path
                || other.starts_with(&path)
                || path.starts_with(other)
                || (cfg!(windows)
                    && other
                        .to_string_lossy()
                        .eq_ignore_ascii_case(&path.to_string_lossy()))
        })
    {
        return Err(io::Error::other(format!(
            "build destination collision: {}",
            path.display()
        )));
    }

    artifacts.insert(path, bytes);

    Ok(())
}

fn emit(
    artifacts: &mut BTreeMap<PathBuf, Vec<u8>>,
    path: PathBuf,
    mut text: Text,
    configuration: &Configuration,
    originals: &BTreeMap<PathBuf, String>,
) -> io::Result<()> {
    if configuration.settings.lower || configuration.settings.rules.remove_types {
        text = transform::lower(text)?;
    }

    if configuration.settings.minify {
        text = transform::minify(&text)?;
    }

    transform::validate(&text.text)?;
    let mut mapping = path.as_os_str().to_owned();
    mapping.push(".map");

    let destination = paths::absolute(
        &configuration.root.join(
            configuration
                .settings
                .output
                .as_ref()
                .ok_or_else(|| io::Error::other("output missing"))?,
        ),
    )?;

    let destination = if configuration.settings.shape == Shape::Directory {
        destination.join(&path)
    } else {
        destination
    };

    insert(
        artifacts,
        mapping.into(),
        text.map(&configuration.root, &destination, originals)?,
    )?;

    insert(artifacts, path, text.text.into_bytes())
}

fn inputs(configuration: &mut Configuration, output: &Path) -> io::Result<Vec<PathBuf>> {
    let mut roots = configuration
        .settings
        .inputs
        .iter()
        .map(|path| paths::absolute(&configuration.root.join(path)))
        .collect::<io::Result<Vec<_>>>()?;

    if let Some(entry) = &configuration.settings.entry {
        roots.push(paths::absolute(&configuration.root.join(entry))?);
    }

    if let Ok(relative) = output.strip_prefix(&configuration.root) {
        paths::safe(&configuration.root, relative)?;
    }

    let output_physical = paths::physical(output)?;

    if paths::physical(&configuration.root)?.starts_with(&output_physical) {
        return Err(io::Error::other("build output overlaps the project root"));
    }

    let mut files = BTreeSet::new();

    for root in roots {
        if !root.starts_with(&configuration.root) {
            return Err(io::Error::other("build input is outside the project root"));
        }

        paths::safe(
            &configuration.root,
            root.strip_prefix(&configuration.root)
                .map_err(io::Error::other)?,
        )?;

        let physical = paths::physical(&root)?;

        if output_physical.starts_with(&physical) || physical.starts_with(&output_physical) {
            return Err(io::Error::other("build output overlaps an input"));
        }

        for path in paths::walk(&root)? {
            if root.is_file()
                || crate::project::selection::Selection::discover(
                    &path,
                    crate::project::selection::Scope::Build,
                )?
                .includes(&path)?
            {
                files.insert(path);
            }
        }
    }

    for directory in files
        .iter()
        .filter_map(|path| path.parent())
        .flat_map(Path::ancestors)
        .collect::<BTreeSet<_>>()
    {
        for name in ["instar.toml", ".luaurc", ".config.luau", "config.luau"] {
            let path = directory.join(name);

            if path.is_file() {
                configuration.files.insert(path.clone(), fs::read(path)?);
            }
        }
    }

    if let Some(path) = &configuration.project {
        configuration.files.insert(path.clone(), fs::read(path)?);
    }

    Ok(files.into_iter().collect())
}
