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

    /// Source patterns included only during builds.
    pub include: Vec<String>,

    /// Source patterns excluded only during builds.
    pub exclude: Vec<String>,

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

    /// Source transformations applied before require resolution and output generation.
    pub rules: Rules,

    /// Remove comments and unnecessary whitespace. Literal contents and token boundaries are preserved.
    pub minify: bool,

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

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "each independently selectable source rule is a boolean switch"
)]
#[serde(default, deny_unknown_fields)]
#[schemars(rename = "BuildRules")]
/// Optional source transformations applied during builds.
pub struct Rules {
    /// Convert eligible require bindings from local to const.
    pub const_requires: bool,

    /// Remove comments while retaining their newline count.
    pub remove_comments: Option<RemoveComments>,

    /// Append comment text at the start or end of each source.
    pub append_text_comment: Option<AppendTextComment>,

    /// Add a Luau directive when no directive from the same family exists.
    pub add_luau_directive: Option<String>,

    /// Rewrite method definitions as ordinary function definitions.
    pub remove_method_definition: bool,

    /// Rewrite compound assignments as ordinary assignments.
    pub remove_compound_assignment: bool,

    /// Rewrite floor division using math.floor.
    pub remove_floor_division: bool,

    /// Rewrite if expressions as boolean expressions when safe.
    pub remove_if_expression: bool,

    /// Rewrite method calls as ordinary function calls.
    pub remove_method_call: bool,

    /// Rewrite bracketed identifier string keys as fields.
    pub convert_index_to_field: bool,

    /// Rewrite named function declarations as assignments.
    pub convert_function_to_assignment: bool,

    /// Rewrite Luau-only numeric literal forms.
    pub convert_luau_number: bool,

    /// Convert const declarations to local declarations.
    pub make_assignment_local: bool,

    /// Remove type syntax and declarations.
    pub remove_types: bool,

    /// Remove function attributes.
    pub remove_attribute: Option<RemoveAttribute>,

    /// Remove parentheses from eligible single-argument calls.
    pub remove_function_call_parens: bool,

    /// Remove statements after an unconditional return or break.
    pub filter_after_early_return: bool,

    /// Rewrite interpolated strings using string.format.
    pub remove_interpolated_string: Option<RemoveInterpolatedString>,

    /// Rewrite continue statements without changing loop behavior.
    pub remove_continue: bool,

    /// Remove empty do blocks.
    pub remove_empty_do: bool,

    /// Remove assertion calls when their arguments can be discarded safely.
    pub remove_assertions: Option<PreserveSideEffects>,

    /// Remove debug profiling calls.
    pub remove_debug_profiling: Option<PreserveSideEffects>,

    /// Fold constant expressions.
    pub compute_expression: bool,

    /// Remove branches whose conditions have constant truth values.
    pub remove_unused_if_branch: bool,

    /// Remove while loops whose conditions are always false.
    pub remove_unused_while: bool,

    /// Remove local declarations whose values are all nil.
    pub remove_nil_declaration: bool,

    /// Group compatible adjacent local declarations.
    pub group_local_assignment: bool,

    /// Rewrite local function declarations as local assignments.
    pub convert_local_function_to_assign: bool,

    /// Rewrite calls to math.sqrt as exponentiation.
    pub convert_square_root_call: bool,

    /// Remove unread local bindings when their values have no side effects.
    pub remove_unused_variable: bool,

    /// Rename local bindings to short names.
    pub rename_variables: bool,

    /// Remove configured calls in statement position.
    pub remove_calls: Option<RemoveCalls>,

    /// Rewrite direct service fields as `GetService` calls.
    pub use_get_service: bool,

    /// Merge duplicate top-level requires.
    pub dedupe_requires: bool,

    /// Name a constant containing the source `DataModel` path.
    pub inject_module_path: Option<String>,

    /// Freeze module table returns that are safe to freeze.
    pub freeze_module: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(untagged)]
#[schemars(rename = "BuildRemoveComments")]
/// Comment removal switch or exception patterns.
pub enum RemoveComments {
    /// Enable or disable comment removal with directive comments retained by default.
    Enabled(bool),

    /// Enable comment removal with regular expression exceptions.
    Options {
        /// Regular expressions matching comments to retain.
        #[serde(default = "comment_exceptions")]
        except: Vec<String>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(untagged)]
#[schemars(rename = "BuildRemoveAttribute")]
/// Attribute removal switch or matching patterns.
pub enum RemoveAttribute {
    /// Enable or disable removal of every attribute.
    Enabled(bool),

    /// Remove attributes whose names match any regular expression.
    Options {
        /// Regular expressions matching attribute names.
        #[serde(default, rename = "match")]
        patterns: Vec<String>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(untagged)]
#[schemars(rename = "BuildRemoveInterpolatedString")]
/// Interpolated string removal switch or conversion strategy.
pub enum RemoveInterpolatedString {
    /// Enable or disable interpolation removal with tostring wrapping.
    Enabled(bool),

    /// Select the string or tostring strategy.
    Options {
        /// Interpolation conversion strategy.
        #[serde(default = "interpolation_strategy")]
        strategy: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(untagged)]
#[schemars(rename = "BuildPreserveSideEffects")]
/// Rule switch or argument side-effect behavior.
pub enum PreserveSideEffects {
    /// Enable or disable the rule while preserving argument side effects.
    Enabled(bool),

    /// Configure whether calls in arguments prevent removal.
    Options {
        /// Preserve calls whose arguments may have side effects.
        #[serde(default = "enabled")]
        preserve_arguments_side_effects: bool,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(untagged)]
#[schemars(rename = "BuildRemoveCalls")]
/// Calls to remove with optional argument side-effect behavior.
pub enum RemoveCalls {
    /// Function names to remove.
    Functions(Vec<String>),

    /// Function names and argument side-effect behavior.
    Options {
        /// Function names to remove.
        functions: Vec<String>,

        /// Preserve calls whose arguments may have side effects.
        #[serde(default = "enabled")]
        preserve_arguments_side_effects: bool,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(rename = "BuildAppendTextComment")]
/// Comment text added to generated sources.
pub struct AppendTextComment {
    /// Literal comment text, mutually exclusive with file.
    #[serde(default)]
    pub text: Option<String>,

    /// Text file relative to the build configuration, mutually exclusive with text.
    #[serde(default)]
    pub file: Option<PathBuf>,

    /// Start or end placement.
    #[serde(default = "comment_location")]
    pub location: String,
}

const fn enabled() -> bool {
    true
}

fn interpolation_strategy() -> String {
    "string".into()
}

fn comment_exceptions() -> Vec<String> {
    vec!["^--!".into()]
}

fn comment_location() -> String {
    "start".into()
}

impl RemoveComments {
    pub(super) const fn enabled(&self) -> bool {
        !matches!(self, Self::Enabled(false))
    }

    pub(super) fn exceptions(&self) -> &[String] {
        match self {
            Self::Enabled(_) => &[],
            Self::Options { except } => except,
        }
    }

    pub(super) const fn retains_directives(&self) -> bool {
        matches!(self, Self::Enabled(true))
    }
}

impl RemoveAttribute {
    pub(super) const fn enabled(&self) -> bool {
        !matches!(self, Self::Enabled(false))
    }

    pub(super) fn patterns(&self) -> &[String] {
        match self {
            Self::Enabled(_) => &[],
            Self::Options { patterns } => patterns,
        }
    }
}

impl RemoveInterpolatedString {
    pub(super) const fn enabled(&self) -> bool {
        !matches!(self, Self::Enabled(false))
    }

    pub(super) fn strategy(&self) -> &str {
        match self {
            Self::Enabled(_) => "string",
            Self::Options { strategy } => strategy,
        }
    }
}

impl PreserveSideEffects {
    pub(super) const fn enabled(&self) -> bool {
        !matches!(self, Self::Enabled(false))
    }

    pub(super) const fn preserve(&self) -> bool {
        match self {
            Self::Enabled(_) => true,

            Self::Options {
                preserve_arguments_side_effects,
            } => *preserve_arguments_side_effects,
        }
    }
}

impl RemoveCalls {
    pub(super) fn functions(&self) -> &[String] {
        match self {
            Self::Functions(functions) | Self::Options { functions, .. } => functions,
        }
    }

    pub(super) const fn preserve(&self) -> bool {
        match self {
            Self::Functions(_) => true,

            Self::Options {
                preserve_arguments_side_effects,
                ..
            } => *preserve_arguments_side_effects,
        }
    }
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

impl Configuration {
    pub(super) fn frontend(&self, path: &Path) -> Option<&crate::graft::Graft> {
        self.grafts.values().find(|graft| graft.owns(path))
    }

    pub(super) fn has_frontends(&self) -> bool {
        self.grafts.values().any(crate::graft::Graft::is_frontend)
    }
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
    validate_rules(&root, &settings.rules, &mut files)?;
    let mut manifests = BTreeMap::new();

    for parent in root.ancestors().collect::<Vec<_>>().into_iter().rev() {
        let path = parent.join("instar.toml");

        if path.is_file() {
            let bytes = fs::read(&path)?;

            let parsed =
                InstarConfig::parse(std::str::from_utf8(&bytes).map_err(io::Error::other)?)
                    .map_err(io::Error::other)?;

            files.insert(path, bytes);

            for (name, dependency) in parsed.grafts.unwrap_or_default() {
                manifests.insert(name, (dependency, parent.to_owned()));
            }
        }
    }

    let mut grafts = BTreeMap::new();

    for (name, (dependency, directory)) in manifests {
        let path = super::paths::absolute(&dependency.resolve(&directory, &name)?)?;

        let graft =
            crate::graft::Graft::load_with_configuration(&path, &name, dependency.configuration())?;

        files.insert(path, fs::read(graft.manifest())?);
        files.insert(graft.entry().to_owned(), fs::read(graft.entry())?);
        grafts.insert(name, graft);
    }

    let mut extensions = BTreeMap::new();

    for (name, graft) in &grafts {
        for extension in graft.extensions() {
            if let Some(previous) = extensions.insert(extension, name) {
                return Err(io::Error::other(format!(
                    "grafts {previous} and {name} both own .{extension}"
                )));
            }
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

fn validate_rules(
    root: &Path,
    rules: &Rules,
    files: &mut BTreeMap<PathBuf, Vec<u8>>,
) -> io::Result<()> {
    if let Some(interpolation) = &rules.remove_interpolated_string
        && interpolation.enabled()
        && !matches!(interpolation.strategy(), "string" | "tostring")
    {
        return Err(io::Error::other(
            "interpolated string strategy must be string or tostring",
        ));
    }

    for pattern in rules
        .remove_comments
        .iter()
        .flat_map(RemoveComments::exceptions)
        .chain(
            rules
                .remove_attribute
                .iter()
                .flat_map(RemoveAttribute::patterns),
        )
    {
        crate::luau::matches(pattern, "")?;
    }

    if let Some(comment) = &rules.append_text_comment {
        if comment.text.is_some() == comment.file.is_some()
            || !matches!(comment.location.as_str(), "start" | "end")
        {
            return Err(io::Error::other(
                "appended comment requires exactly one of text or file and a start or end location",
            ));
        }

        if let Some(path) = &comment.file {
            let path = super::paths::absolute(&root.join(path))?;

            if !path.starts_with(root) {
                return Err(io::Error::other(
                    "appended comment file is outside the build project",
                ));
            }

            super::paths::safe(root, path.strip_prefix(root).map_err(io::Error::other)?)?;

            files.insert(path.clone(), fs::read(path)?);
        }
    }

    if rules
        .inject_module_path
        .as_ref()
        .is_some_and(|name| !identifier(name))
    {
        return Err(io::Error::other(
            "injected module path name must be an identifier",
        ));
    }

    if let Some(calls) = &rules.remove_calls
        && calls
            .functions()
            .iter()
            .any(|name| !name.split('.').all(identifier))
    {
        return Err(io::Error::other(
            "removed calls must be names or dotted paths of names",
        ));
    }

    Ok(())
}

fn identifier(value: &str) -> bool {
    let mut bytes = value.bytes();

    let Some(first) = bytes.next() else {
        return false;
    };

    (first.is_ascii_alphabetic() || first == b'_')
        && bytes.all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
        && !matches!(
            value,
            "and"
                | "break"
                | "do"
                | "else"
                | "elseif"
                | "end"
                | "false"
                | "for"
                | "function"
                | "if"
                | "in"
                | "local"
                | "nil"
                | "not"
                | "or"
                | "repeat"
                | "return"
                | "then"
                | "true"
                | "until"
                | "while"
        )
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

    constant_definitions(&settings.constants)?;

    Ok(settings)
}

pub(crate) fn constant_definitions(constants: &BTreeMap<String, Constant>) -> io::Result<Vec<u8>> {
    let mut definitions = String::new();

    for (name, constant) in constants {
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

        definitions.push_str("declare ");
        definitions.push_str(name);
        definitions.push_str(": ");

        definitions.push_str(match constant {
            Constant::Boolean(_) => "boolean",
            Constant::Number(_) => "number",
            Constant::String(_) => "string",
        });

        definitions.push('\n');
    }

    Ok(definitions.into_bytes())
}
