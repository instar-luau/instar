use std::{
    collections::{BTreeMap, BTreeSet},
    fmt::Display,
    fs,
    io::{self, Read, Write},
    path::{Path, PathBuf},
    sync::{Arc, PoisonError},
};

use instar_luau::{Capabilities, ClassCapability, PropertyCapability};

use instar_shared::{
    Completion, Diagnostic, DiagnosticCategory, Event, EventPayload, FailureKind, FindingIdentity,
    Location, OperationFailure, Options, Progress, Reason, Severity, SourceSnapshot, SourceVersion,
    Span,
};

use serde::Deserialize;
use serde_json::Deserializer;
use sha2::{Digest, Sha256};

use crate::{
    acquisition,
    capture::Capture,
    configuration::Security,
    documentation::{self, Index},
    execution::{Runtime, Work, WorkOutput},
    inputs::{ProjectSnapshot, SourceOrigin, absolute},
    roblox::members::Members,
};

const PROFILES: [&str; 4] = ["none", "local", "plugin", "roblox"];
pub(crate) const MAXIMUM_BYTES: usize = 64 * 1024 * 1024;

/// An explicitly requested, digest-pinned whole-bundle cache acquisition.
#[derive(Clone, Debug)]
pub struct AssetRequest {
    /// HTTPS identity of the selected bundle's metadata.json.
    pub selection: String,

    /// Expected canonical SHA-256 digest of metadata.json.
    pub expected_digest: String,

    /// Explicit bundle directory inside a versioned Instar asset cache.
    pub destination: PathBuf,
}

/// Verified contents installed by an authorized acquisition.
#[derive(Clone, Debug)]
pub struct AssetSnapshot {
    /// The requested asset identity.
    pub identity: String,

    /// Installed metadata source revision.
    pub revision: u64,

    /// Verified SHA-256 digest of metadata.json.
    pub digest: String,

    /// Exact installed metadata contents.
    pub contents: SourceSnapshot,
}

pub(crate) struct Assets {
    pub(crate) definitions: Vec<SourceSnapshot>,
    pub(crate) capabilities: Capabilities,
    pub(crate) documentation: Index,
    pub(crate) members: Members,
}

#[derive(Debug)]
pub(crate) struct Bundle {
    namespace: &'static str,
    files: BTreeMap<String, String>,
}

impl Bundle {
    pub(crate) fn files(&self) -> &BTreeMap<String, String> {
        &self.files
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Metadata {
    revision: String,
    services: Vec<String>,
    creatable_instances: Vec<String>,
    profiles: BTreeMap<String, String>,
    documentation: String,
    enumerations: String,
    properties: BTreeMap<String, Vec<Access>>,
}

impl Metadata {
    fn parse(source: &str) -> io::Result<Self> {
        let metadata: Self = parse(source)?;
        revision(&metadata.revision)?;
        names(&metadata.services)?;
        names(&metadata.creatable_instances)?;

        if metadata.profiles.len() != PROFILES.len() || metadata.properties.len() != PROFILES.len()
        {
            return Err(invalid("invalid Roblox security profiles"));
        }

        for profile in PROFILES {
            digest(
                metadata
                    .profiles
                    .get(profile)
                    .ok_or_else(|| invalid("missing Roblox security profile"))?,
            )?;

            let properties = metadata
                .properties
                .get(profile)
                .ok_or_else(|| invalid("missing Roblox property profile"))?;

            let mut seen = BTreeSet::new();

            for property in properties {
                if !name(&property.class)
                    || property.name.is_empty()
                    || property.name.bytes().any(|byte| byte.is_ascii_control())
                    || !seen.insert((&property.class, &property.name))
                {
                    return Err(invalid("invalid or duplicate Roblox property capability"));
                }
            }
        }

        digest(&metadata.documentation)?;
        digest(&metadata.enumerations)?;

        Ok(metadata)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Access {
    class: String,
    name: String,
    read: bool,
    write: bool,
}

impl Runtime {
    pub(crate) fn acquire(
        &mut self,
        request: AssetRequest,
        options: &Options,
    ) -> io::Result<AssetSnapshot> {
        options.check()?;
        acquisition::validate(&request.selection, &request.expected_digest)?;

        let base = request
            .selection
            .strip_suffix("/metadata.json")
            .ok_or_else(|| invalid("asset selection must identify metadata.json"))?;

        let destination = absolute(&request.destination)?;

        let namespace = ["roblox", "documentation"]
            .into_iter()
            .find(|namespace| directory(namespace).is_ok_and(|root| destination.starts_with(root)))
            .ok_or_else(|| {
                invalid("asset installation is restricted to versioned Instar caches")
            })?;

        validate_directory(&destination)?;
        validate_documents(&destination, &self.host.snapshot())?;

        let bundle = if let Some(bundle) = cached(&destination, namespace, options)? {
            verify(&bundle.files["metadata.json"], &request.expected_digest)?;

            bundle
        } else {
            let metadata =
                self.asset_download(&request.selection, &request.expected_digest, options)?;

            let expected = manifest(namespace, &metadata)?;
            let mut files = BTreeMap::from([("metadata.json".to_owned(), metadata)]);
            let mut size = files["metadata.json"].len();

            for (name, digest) in expected {
                options.check()?;

                let contents = self.asset_download(&format!("{base}/{name}"), &digest, options)?;

                size = size
                    .checked_add(contents.len())
                    .ok_or_else(|| Reason::AnalysisLimit.error())?;

                if size > MAXIMUM_BYTES {
                    return Err(Reason::AnalysisLimit.error());
                }

                files.insert(name, contents);
            }

            let bundle = verify_bundle(namespace, files, options)?;

            let current = self
                .host
                .snapshot
                .write()
                .unwrap_or_else(PoisonError::into_inner);

            validate_documents(&destination, &current)?;
            let installed = publish(&destination, bundle, options)?;
            drop(current);
            verify(&installed.files["metadata.json"], &request.expected_digest)?;

            installed
        };

        options.check()?;
        validate_documents(&destination, &self.host.snapshot())?;
        let mut capture = Capture::live(Arc::clone(&self.host));
        let mut metadata = None;

        for (name, expected) in bundle.files {
            let source = capture.source(&destination.join(&name), options)?;

            if source.text.as_ref() != expected
                || !matches!(
                    capture.snapshot.origin(source.version.identity),
                    Some(SourceOrigin::Disk(_))
                )
            {
                return Err(Reason::StaleQuery.error());
            }

            if name == "metadata.json" {
                metadata = Some(source);
            }
        }

        let contents = metadata.ok_or_else(|| invalid("asset bundle has no metadata"))?;
        options.check()?;

        options.report(Event {
            invocation: 0,
            operation: 0,
            payload: EventPayload::Progress(Progress {
                phase: "Verified asset acquisition".to_owned(),
                source: Some(contents.version),
                completed: 1,
                total: Some(1),
            }),
        });

        options.check()?;

        Ok(AssetSnapshot {
            identity: request.selection,
            revision: contents.version.revision,
            digest: request.expected_digest,
            contents,
        })
    }

    fn asset_download(
        &mut self,
        selection: &str,
        expected_digest: &str,
        options: &Options,
    ) -> io::Result<String> {
        let WorkOutput::Asset(download) = self.workers.execute(
            Work::Asset {
                selection: selection.to_owned(),
                expected_digest: expected_digest.to_owned(),
            },
            options,
        )?
        else {
            return Err(io::Error::other(
                "asset acquisition lane returned wrong outcome",
            ));
        };

        if download.selection != selection || download.digest != expected_digest {
            return Err(invalid(
                "asset acquisition output does not match requested identity",
            ));
        }

        verify(&download.contents, expected_digest)?;
        options.check()?;

        Ok(download.contents)
    }
}

pub(crate) fn load(
    directory: &Path,
    security: Security,
    capture: &mut Capture,
    options: &Options,
) -> io::Result<Assets> {
    let metadata_source = capture.source(&directory.join("metadata.json"), options)?;

    let metadata = Metadata::parse(&metadata_source.text)
        .map_err(|error| attributed(&metadata_source, &[], "assets.metadata", error))?;

    let services = names(&metadata.services)
        .map_err(|error| attributed(&metadata_source, &[], "assets.metadata", error))?;

    let creatable = names(&metadata.creatable_instances)
        .map_err(|error| attributed(&metadata_source, &[], "assets.metadata", error))?;

    let profile = match security {
        Security::None => "none",
        Security::Local => "local",
        Security::Plugin => "plugin",
        Security::Roblox => "roblox",
    };

    let declarations = capture.source(&directory.join(format!("{profile}.d.luau")), options)?;
    let documentation = capture.source(&directory.join("documentation.json"), options)?;
    let enumerations = capture.source(&directory.join("enumerations.d.luau"), options)?;

    verify(&declarations.text, &metadata.profiles[profile]).map_err(|error| {
        attributed(
            &declarations,
            &[metadata_source.version],
            "assets.digest",
            error,
        )
    })?;

    verify(&documentation.text, &metadata.documentation).map_err(|error| {
        attributed(
            &documentation,
            &[metadata_source.version],
            "assets.digest",
            error,
        )
    })?;

    verify(&enumerations.text, &metadata.enumerations).map_err(|error| {
        attributed(
            &enumerations,
            &[metadata_source.version],
            "assets.digest",
            error,
        )
    })?;

    let index = Index::parse(&documentation.text, &["@roblox"])
        .map_err(|error| attributed(&documentation, &[], "assets.documentation", error))?;

    let members = Members::parse(&declarations, options)
        .map_err(|error| attributed(&declarations, &[], "assets.declarations", error))?;

    let mut classes = services
        .union(&creatable)
        .map(|name| {
            (
                (*name).to_owned(),
                ClassCapability {
                    name: (*name).to_owned(),
                    service: services.contains(name),
                    creatable: creatable.contains(name),
                    properties: Vec::new(),
                },
            )
        })
        .collect::<BTreeMap<_, _>>();

    for property in &metadata.properties[profile] {
        let class = classes
            .entry(property.class.clone())
            .or_insert_with(|| ClassCapability {
                name: property.class.clone(),
                service: false,
                creatable: false,
                properties: Vec::new(),
            });

        if class
            .properties
            .iter()
            .any(|previous| previous.name == property.name)
        {
            return Err(attributed(
                &metadata_source,
                &[],
                "assets.capabilities",
                invalid("duplicate Roblox property capability"),
            ));
        }

        class.properties.push(PropertyCapability {
            name: property.name.clone(),
            read: property.read,
            write: property.write,
        });
    }

    options.check()?;

    Ok(Assets {
        definitions: vec![declarations, enumerations],
        capabilities: Capabilities {
            classes: classes.into_values().collect(),
        },
        documentation: index,
        members,
    })
}

/// Returns the versioned Roblox asset cache without creating it.
///
/// # Errors
/// Returns an error when the operating system cache directory is unavailable.
pub fn roblox_cache_directory() -> io::Result<PathBuf> {
    directory("roblox")
}

/// Returns the versioned builtin-documentation cache without creating it.
///
/// # Errors
/// Returns an error when the operating system cache directory is unavailable.
pub fn documentation_cache_directory() -> io::Result<PathBuf> {
    directory("documentation")
}

pub(crate) fn directory(namespace: &str) -> io::Result<PathBuf> {
    let base = dirs::cache_dir().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "user cache directory is unavailable",
        )
    })?;

    if !base.is_absolute() {
        return Err(invalid("user cache directory must be absolute"));
    }

    Ok(base
        .join("instar")
        .join(namespace)
        .join(env!("CARGO_PKG_VERSION")))
}

pub(crate) fn parse<'source, Output: Deserialize<'source>>(
    source: &'source str,
) -> io::Result<Output> {
    let mut deserializer = Deserializer::from_str(source);
    let value = serde_path_to_error::deserialize(&mut deserializer).map_err(invalid)?;
    deserializer.end().map_err(invalid)?;

    Ok(value)
}

pub(crate) fn verify(source: &str, expected: &str) -> io::Result<()> {
    if format!("{:x}", Sha256::digest(source.as_bytes())) != expected {
        return Err(invalid("assets do not match their metadata"));
    }

    Ok(())
}

pub(crate) fn digest(value: &str) -> io::Result<()> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(invalid("invalid canonical SHA-256 digest"));
    }

    Ok(())
}

pub(crate) fn revision(value: &str) -> io::Result<()> {
    if value.len() != 40 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(invalid("invalid asset revision"));
    }

    Ok(())
}

pub(crate) fn attributed(
    source: &SourceSnapshot,
    prerequisites: &[SourceVersion],
    code: &str,
    error: io::Error,
) -> io::Error {
    if matches!(
        error.kind(),
        io::ErrorKind::Interrupted | io::ErrorKind::TimedOut
    ) || error.get_ref().is_some_and(|error| {
        error.downcast_ref::<Reason>().is_some()
            || error.downcast_ref::<OperationFailure>().is_some()
    }) {
        return error;
    }

    let primary = Location {
        source: source.version,
        span: Span {
            start: 0,
            end: source.text.len(),
        },
    };

    let message = error.to_string();

    let diagnostic = Diagnostic {
        identity: FindingIdentity::new("project", prerequisites.to_vec(), code, primary),
        code: code.to_owned(),
        category: DiagnosticCategory::Configuration,
        severity: Severity::Error,
        message: message.clone(),
        primary,
        related: Vec::new(),
        tags: Vec::new(),
    };

    io::Error::new(
        error.kind(),
        OperationFailure {
            kind: FailureKind::InvalidInvocation,
            message,
            completion: Completion::Incomplete(vec![Reason::UnavailableEnvironment]),
            diagnostics: vec![diagnostic],
            completed_sources: Vec::new(),
            completed_edits: Vec::new(),
        },
    )
}

pub(crate) fn verify_bundle(
    namespace: &str,
    files: BTreeMap<String, String>,
    options: &Options,
) -> io::Result<Bundle> {
    options.check()?;

    let namespace = match namespace {
        "roblox" => "roblox",
        "documentation" => "documentation",
        _ => return Err(invalid("unsupported asset namespace")),
    };

    let mut size = 0_usize;

    for contents in files.values() {
        size = size
            .checked_add(contents.len())
            .ok_or_else(|| Reason::AnalysisLimit.error())?;

        if size > MAXIMUM_BYTES {
            return Err(Reason::AnalysisLimit.error());
        }
    }

    let metadata = files
        .get("metadata.json")
        .ok_or_else(|| invalid("asset bundle has no metadata"))?;

    let expected = manifest(namespace, metadata)?;

    if files.len() != expected.len() + 1 {
        return Err(invalid("asset bundle contains unexpected or missing files"));
    }

    for (name, digest) in expected {
        options.check()?;

        let contents = files
            .get(&name)
            .ok_or_else(|| invalid(format!("asset bundle is missing {name}")))?;

        verify(contents, &digest)?;
    }

    let documentation = files
        .get("documentation.json")
        .ok_or_else(|| invalid("asset bundle has no documentation"))?;

    Index::parse(
        documentation,
        &[if namespace == "roblox" {
            "@roblox"
        } else {
            "@luau"
        }],
    )?;

    options.check()?;

    Ok(Bundle { namespace, files })
}

pub(crate) fn publish(directory: &Path, bundle: Bundle, options: &Options) -> io::Result<Bundle> {
    options.check()?;
    validate_directory(directory)?;

    if let Some(existing) = cached(directory, bundle.namespace, options)? {
        return Ok(existing);
    }

    let parent = directory
        .parent()
        .ok_or_else(|| invalid("asset destination has no parent"))?;

    fs::create_dir_all(parent)?;
    validate_directory(directory)?;
    options.check()?;

    let staged = tempfile::Builder::new()
        .prefix("instar-")
        .tempdir_in(parent)?;

    for (name, contents) in bundle.files() {
        options.check()?;

        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(staged.path().join(name))?;

        for bytes in contents.as_bytes().chunks(8192) {
            options.check()?;
            file.write_all(bytes)?;
        }

        file.sync_all()?;
    }

    #[cfg(unix)]
    fs::File::open(staged.path())?.sync_all()?;

    options.check()?;
    validate_directory(directory)?;

    match fs::rename(staged.path(), directory) {
        Ok(()) => Ok(bundle),

        Err(error) => match cached(directory, bundle.namespace, options)? {
            Some(existing) => Ok(existing),
            None => Err(error),
        },
    }
}

fn manifest(namespace: &str, metadata: &str) -> io::Result<BTreeMap<String, String>> {
    match namespace {
        "roblox" => {
            let metadata = Metadata::parse(metadata)?;

            let mut expected: BTreeMap<_, _> = metadata
                .profiles
                .into_iter()
                .map(|(profile, digest)| (format!("{profile}.d.luau"), digest))
                .collect();

            expected.insert("documentation.json".to_owned(), metadata.documentation);
            expected.insert("enumerations.d.luau".to_owned(), metadata.enumerations);

            Ok(expected)
        }

        "documentation" => {
            let metadata = documentation::Metadata::parse(metadata)?;

            Ok(BTreeMap::from([(
                "documentation.json".to_owned(),
                metadata.documentation,
            )]))
        }

        _ => Err(invalid("unsupported asset namespace")),
    }
}

fn cached(directory: &Path, namespace: &str, options: &Options) -> io::Result<Option<Bundle>> {
    options.check()?;
    validate_directory(directory)?;

    match fs::symlink_metadata(directory) {
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    }

    let mut remaining = MAXIMUM_BYTES;
    let metadata = read_file(&directory.join("metadata.json"), &mut remaining, options)?;
    let expected = manifest(namespace, &metadata)?;
    let mut files = BTreeMap::from([("metadata.json".to_owned(), metadata)]);

    for name in expected.keys() {
        files.insert(
            name.clone(),
            read_file(&directory.join(name), &mut remaining, options)?,
        );
    }

    Ok(Some(verify_bundle(namespace, files, options)?))
}

fn read_file(path: &Path, remaining: &mut usize, options: &Options) -> io::Result<String> {
    options.check()?;

    if !fs::symlink_metadata(path)?.is_file() {
        return Err(invalid("asset cache contains a nonregular file"));
    }

    let mut file = fs::File::open(path)?;
    let mut bytes = Vec::new();
    let mut buffer = [0; 8192];

    loop {
        options.check()?;
        let count = file.read(&mut buffer)?;

        if count == 0 {
            break;
        }

        if count > *remaining {
            return Err(Reason::AnalysisLimit.error());
        }

        *remaining -= count;
        bytes.extend_from_slice(&buffer[..count]);
    }

    options.check()?;

    String::from_utf8(bytes).map_err(|_| invalid("asset cache is not UTF-8"))
}

fn validate_directory(directory: &Path) -> io::Result<()> {
    absolute(directory)?;

    for ancestor in directory.ancestors() {
        match fs::symlink_metadata(ancestor) {
            Ok(metadata) if !metadata.is_dir() || metadata.file_type().is_symlink() => {
                return Err(invalid(
                    "asset destination has a non-directory or symlink ancestor",
                ));
            }

            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }

    Ok(())
}

fn validate_documents(directory: &Path, snapshot: &ProjectSnapshot) -> io::Result<()> {
    if snapshot.sources.records.values().any(|record| {
        !matches!(record.origin, SourceOrigin::Disk(_))
            && record
                .origin
                .path()
                .is_some_and(|path| path.starts_with(directory))
            && record.snapshot.is_some()
    }) {
        return Err(invalid(
            "asset installation cannot replace a synchronized document",
        ));
    }

    Ok(())
}

fn name(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

fn names(values: &[String]) -> io::Result<BTreeSet<&str>> {
    let mut names = BTreeSet::new();

    for value in values {
        if !name(value) || !names.insert(value.as_str()) {
            return Err(invalid("invalid or duplicate Roblox metadata class"));
        }
    }

    Ok(names)
}

fn invalid(message: impl Display) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.to_string())
}
