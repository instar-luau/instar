use std::{
    collections::{BTreeMap, BTreeSet},
    io,
    path::{Component, Path, PathBuf},
    sync::Arc,
};

use instar_shared::{ContextIdentity, SourceIdentity, SourceSnapshot, SourceVersion};
use serde::Serialize;

use crate::{
    change::{ChangeBatch, ProjectChange, SourceChange, SourceEdit},
    configuration::ProjectSettingsPatch,
};

/// The source's filesystem, client, or synthetic ownership.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub enum SourceOrigin {
    /// A source owned by its absolute disk path.
    Disk(PathBuf),

    /// Invocation-owned standard input and its optional resolution context.
    Stdin {
        /// Optional absolute path used for resolution.
        context: Option<PathBuf>,
    },

    /// Client-synchronized contents and their optional resolution context.
    Document {
        /// Stable client address.
        address: String,
        /// Optional absolute path used for resolution.
        context: Option<PathBuf>,
    },

    /// Synthetic contents addressed independently of the filesystem.
    Virtual {
        /// Stable synthetic address.
        address: String,
    },
}

impl SourceOrigin {
    /// Returns the source's filesystem resolution context, if any.
    #[must_use]
    pub fn path(&self) -> Option<&Path> {
        match self {
            Self::Disk(path) => Some(path),
            Self::Stdin { context } | Self::Document { context, .. } => context.as_deref(),
            Self::Virtual { .. } => None,
        }
    }
}

/// The stable identity of a consulted project input.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub enum InputIdentity {
    /// Exact contents of a source.
    Source(SourceIdentity),

    /// A configuration input derived from a source.
    Configuration(SourceIdentity),

    /// Entries in an absolute directory.
    Directory(PathBuf),

    /// A named external asset.
    Asset(String),

    /// A sourcemap input derived from a source.
    Sourcemap(SourceIdentity),

    /// Workspace or client configuration policy.
    Context(ContextIdentity),

    /// Source availability at an absolute path.
    Path(PathBuf),

    /// The complete registered source inventory.
    Sources,
}

/// The exact observed value of an input, retained by its snapshot.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub enum InputState {
    /// The input was absent or had no synchronized contents.
    Missing,

    /// The directory input was present but could not be traversed as a directory.
    NonDirectory,

    /// The input requires a fresh observation.
    Stale,

    /// Exact original source bytes and their source version.
    Source(SourceSnapshot),

    /// Sorted absolute paths present in a directory.
    Directory(Vec<PathBuf>),

    /// An externally supplied content digest.
    Digest(String),

    /// The revision of workspace or client policy.
    Policy(u64),

    /// Complete source records, including origins and closed documents.
    Sources(BTreeMap<SourceIdentity, SourceRecord>),
}

/// An immutable input value and its compare-and-swap revision.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct InputVersion {
    /// Stable identity of the consulted input.
    pub identity: InputIdentity,

    /// Revision advanced exactly once for each changed value.
    pub revision: u64,

    /// The complete captured value.
    pub state: InputState,
}

/// Registered source ownership and exact contents, including closed state.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SourceRecord {
    /// Ownership and resolution context of the source.
    pub origin: SourceOrigin,

    /// Exact contents, or no contents for a closed document.
    pub snapshot: Option<SourceSnapshot>,

    /// Source revision retained even when synchronized contents are absent.
    pub revision: u64,
}

/// Immutable snapshot-owned source records and disk identities.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SourceStore {
    pub(crate) records: BTreeMap<SourceIdentity, SourceRecord>,
    pub(crate) disk: BTreeMap<PathBuf, SourceIdentity>,
    pub(crate) declarations: BTreeSet<SourceVersion>,
}

/// Immutable snapshot-owned workspace and client configuration.
#[derive(Clone, Debug, Default)]
pub struct ContextStore {
    pub(crate) roots: BTreeMap<PathBuf, ContextIdentity>,
    pub(crate) clients: BTreeMap<ContextIdentity, ProjectSettingsPatch>,
}

/// Immutable project history extended only by explicit transitions or new observations.
#[derive(Clone, Debug)]
pub struct ProjectSnapshot {
    /// Monotonic identity within this snapshot's transition history.
    pub identity: u64,

    /// Exact source records belonging to this history.
    pub sources: Arc<SourceStore>,

    /// Workspace roots and client configuration belonging to this history.
    pub contexts: Arc<ContextStore>,

    /// Complete values of recorded project inputs.
    pub input_versions: Arc<BTreeMap<InputIdentity, InputVersion>>,
}

impl ProjectSnapshot {
    /// Creates an isolated snapshot retaining one exact declaration and its origin.
    ///
    /// # Errors
    /// Returns invalid-source or invalid-origin errors.
    pub fn declaration(source: SourceSnapshot, origin: SourceOrigin) -> io::Result<Self> {
        source.validate()?;
        validate_origin(&origin)?;
        let mut snapshot = initial(Vec::new());
        let identity = source.version.identity;
        let path = origin.path().map(Path::to_path_buf);
        let sources = Arc::make_mut(&mut snapshot.sources);

        if let SourceOrigin::Disk(path) = &origin {
            sources.disk.insert(path.clone(), identity);
        }

        sources.declarations.insert(source.version);

        sources.records.insert(
            identity,
            SourceRecord {
                origin,
                revision: source.version.revision,
                snapshot: Some(source.clone()),
            },
        );

        let inputs = Arc::make_mut(&mut snapshot.input_versions);
        let source_identity = InputIdentity::Source(identity);

        inputs.insert(
            source_identity.clone(),
            InputVersion {
                identity: source_identity,
                revision: 1,
                state: InputState::Source(source.clone()),
            },
        );

        if let Some(path) = path {
            let identity = InputIdentity::Path(path);

            inputs.insert(
                identity.clone(),
                InputVersion {
                    identity,
                    revision: 1,
                    state: InputState::Source(source),
                },
            );
        }

        advance_inventory(sources, inputs, 1)?;

        Ok(snapshot)
    }

    /// Returns exact synchronized contents for a registered source.
    #[must_use]
    pub fn source(&self, identity: SourceIdentity) -> Option<&SourceSnapshot> {
        self.sources.records.get(&identity)?.snapshot.as_ref()
    }

    /// Returns captured contents at a path, preferring an open document over disk.
    #[must_use]
    pub fn source_at(&self, path: &Path) -> Option<&SourceSnapshot> {
        self.sources
            .records
            .values()
            .filter(|record| {
                record.origin.path() == Some(path)
                    && !matches!(record.origin, SourceOrigin::Stdin { .. })
                    && record.snapshot.is_some()
            })
            .max_by_key(|record| matches!(record.origin, SourceOrigin::Document { .. }))
            .and_then(|record| record.snapshot.as_ref())
    }

    /// Returns captured contents for a disk or client address.
    #[must_use]
    pub fn source_for_address(&self, address: &str) -> Option<&SourceSnapshot> {
        self.sources.records.values().find_map(|record| {
            let matches = match &record.origin {
                SourceOrigin::Document {
                    address: candidate, ..
                }
                | SourceOrigin::Virtual { address: candidate } => candidate == address,

                SourceOrigin::Disk(path) => path.to_str() == Some(address),
                SourceOrigin::Stdin { .. } => false,
            };

            matches.then_some(record.snapshot.as_ref()).flatten()
        })
    }

    /// Returns the registered ownership of a source, including closed documents.
    #[must_use]
    pub fn origin(&self, identity: SourceIdentity) -> Option<&SourceOrigin> {
        self.sources
            .records
            .get(&identity)
            .map(|record| &record.origin)
    }

    /// Returns whether this exact source revision was captured as an environment declaration.
    #[must_use]
    pub fn is_declaration(&self, source: SourceVersion) -> bool {
        self.sources.declarations.contains(&source)
            && self
                .source(source.identity)
                .is_some_and(|snapshot| snapshot.version == source)
    }

    /// Iterates all sources with captured contents.
    pub fn sources(&self) -> impl Iterator<Item = &SourceSnapshot> {
        self.sources
            .records
            .values()
            .filter_map(|record| record.snapshot.as_ref())
    }

    /// Iterates registered absolute workspace roots.
    pub fn roots(&self) -> impl Iterator<Item = &PathBuf> {
        self.contexts.roots.keys()
    }

    /// Finds the most specific workspace context containing a path.
    #[must_use]
    pub fn context_at(&self, path: &Path) -> ContextIdentity {
        self.contexts
            .roots
            .iter()
            .filter(|(root, _)| path.starts_with(root))
            .max_by_key(|(root, _)| root.components().count())
            .map_or_else(|| ContextIdentity::new(0), |(_, identity)| *identity)
    }

    /// Finds the workspace context of a registered source.
    #[must_use]
    pub fn context(&self, source: SourceIdentity) -> ContextIdentity {
        self.origin(source)
            .and_then(SourceOrigin::path)
            .and_then(|path| {
                self.contexts
                    .roots
                    .iter()
                    .filter(|(root, _)| path.starts_with(root))
                    .max_by_key(|(root, _)| root.components().count())
                    .map(|(_, identity)| *identity)
            })
            .unwrap_or_else(|| ContextIdentity::new(0))
    }

    /// Checks complete input values and revisions against this snapshot.
    #[must_use]
    pub fn current(&self, inputs: &[InputVersion]) -> bool {
        inputs
            .iter()
            .all(|input| self.input_versions.get(&input.identity) == Some(input))
    }

    pub(crate) fn transition(&self, batch: ChangeBatch) -> io::Result<Self> {
        let mut next = self.clone();
        let sources = Arc::make_mut(&mut next.sources);
        let contexts = Arc::make_mut(&mut next.contexts);
        let inputs = Arc::make_mut(&mut next.input_versions);

        for change in batch.changes {
            match change {
                ProjectChange::Source(change) => {
                    apply_source(sources, inputs, change)?;
                    advance_inventory(sources, inputs, 1)?;
                }

                ProjectChange::WorkspaceRoots { added, removed } => {
                    for root in removed {
                        let root = absolute(&root)?;

                        let identity = contexts
                            .roots
                            .remove(&root)
                            .ok_or_else(|| invalid("removed workspace root is not registered"))?;

                        contexts.clients.remove(&identity);
                        advance_policy(inputs, identity)?;
                    }

                    for root in added {
                        let root = absolute(&root)?;

                        if contexts.roots.contains_key(&root) {
                            return Err(invalid("workspace root is already registered"));
                        }

                        let identity = ContextIdentity::new(
                            contexts
                                .roots
                                .values()
                                .map(|identity| identity.get())
                                .max()
                                .unwrap_or(0)
                                .checked_add(1)
                                .ok_or_else(|| invalid("context identity overflow"))?,
                        );

                        contexts.roots.insert(root, identity);
                        advance_policy(inputs, identity)?;
                    }
                }

                ProjectChange::ClientConfiguration { context, patch } => {
                    if context.get() != 0
                        && !contexts.roots.values().any(|identity| *identity == context)
                    {
                        return Err(invalid("unknown configuration context"));
                    }

                    if let Some(patch) = patch {
                        patch.validate()?;
                        contexts.clients.insert(context, *patch);
                    } else {
                        contexts.clients.remove(&context);
                    }

                    advance_policy(inputs, context)?;
                }

                ProjectChange::Invalidate { paths } => {
                    invalidate_inputs(sources, inputs, paths)?;
                }

                ProjectChange::Input { expected, current } => {
                    if inputs.get(&current.identity) != expected.as_ref() {
                        return Err(invalid("consulted input revision mismatch"));
                    }

                    validate_input(&current)?;

                    if current.identity == InputIdentity::Sources
                        && current.state != InputState::Sources(sources.records.clone())
                    {
                        return Err(invalid("source inventory differs from registered sources"));
                    }

                    if let Some(expected) = &expected {
                        if current.state == expected.state {
                            if current.revision != expected.revision {
                                return Err(invalid("unchanged input cannot advance its revision"));
                            }
                        } else if current.revision
                            != expected
                                .revision
                                .checked_add(1)
                                .ok_or_else(|| invalid("input revision overflow"))?
                        {
                            return Err(invalid("changed input must advance one revision"));
                        }
                    } else if current.revision != 1 {
                        return Err(invalid("new input revision must be one"));
                    }

                    if let InputIdentity::Path(path) = &current.identity
                        && current.state == InputState::Stale
                        && expected.as_ref() != Some(&current)
                    {
                        invalidate_path(sources, inputs, path)?;
                    }

                    inputs.insert(current.identity.clone(), current);
                }

                ProjectChange::WorkerDiscarded(_) => {}
            }
        }

        next.identity = next
            .identity
            .checked_add(1)
            .ok_or_else(|| invalid("snapshot identity overflow"))?;

        Ok(next)
    }

    pub(crate) fn admit(
        &self,
        observed: &Self,
        protected: &BTreeSet<InputIdentity>,
    ) -> io::Result<Self> {
        let mut next = self.clone();
        let mut admitted = 0_u64;

        for (identity, record) in &observed.sources.records {
            if protected.contains(&InputIdentity::Source(*identity))
                || record.origin.path().is_some_and(|path| {
                    protected.contains(&InputIdentity::Path(path.to_path_buf()))
                })
            {
                continue;
            }

            if !next.admit_source(*identity, record)? {
                continue;
            }

            admitted = admitted
                .checked_add(1)
                .ok_or_else(|| invalid("source inventory revision overflow"))?;
        }

        let declarations = observed
            .sources
            .declarations
            .iter()
            .filter(|source| {
                !protected.contains(&InputIdentity::Source(source.identity))
                    && observed
                        .origin(source.identity)
                        .and_then(SourceOrigin::path)
                        .is_none_or(|path| {
                            !protected.contains(&InputIdentity::Path(path.to_path_buf()))
                        })
                    && next
                        .source(source.identity)
                        .is_some_and(|snapshot| snapshot.version == **source)
            })
            .copied()
            .collect::<Vec<_>>();

        Arc::make_mut(&mut next.sources)
            .declarations
            .extend(declarations);

        for (identity, input) in observed.input_versions.iter() {
            if *identity == InputIdentity::Sources || protected.contains(identity) {
                continue;
            }

            if let InputIdentity::Source(source) = identity
                && observed
                    .origin(*source)
                    .and_then(SourceOrigin::path)
                    .is_some_and(|path| {
                        protected.contains(&InputIdentity::Path(path.to_path_buf()))
                    })
            {
                continue;
            }

            if let Some(previous) = next.input_versions.get(identity) {
                if previous == input {
                    continue;
                }

                if previous.state != InputState::Stale
                    || input.state == InputState::Stale
                    || input.revision <= previous.revision
                {
                    return Err(invalid("admitted input conflicts with recorded history"));
                }
            }

            validate_input(input)?;

            if let InputState::Source(source) = &input.state {
                match identity {
                    InputIdentity::Source(identity)
                    | InputIdentity::Configuration(identity)
                    | InputIdentity::Sourcemap(identity)
                        if next.source(*identity) != Some(source) =>
                    {
                        return Err(invalid("admitted source input differs from its record"));
                    }

                    InputIdentity::Path(path) if next.source_at(path) != Some(source) => {
                        return Err(invalid("admitted path input differs from its source"));
                    }

                    _ => {}
                }
            }

            Arc::make_mut(&mut next.input_versions).insert(identity.clone(), input.clone());
        }

        if admitted != 0 {
            advance_inventory(
                &next.sources,
                Arc::make_mut(&mut next.input_versions),
                admitted,
            )?;
        }

        if next.sources == self.sources && next.input_versions == self.input_versions {
            return Ok(self.clone());
        }

        next.identity = next
            .identity
            .checked_add(1)
            .ok_or_else(|| invalid("snapshot identity overflow"))?;

        Ok(next)
    }

    fn admit_source(
        &mut self,
        identity: SourceIdentity,
        record: &SourceRecord,
    ) -> io::Result<bool> {
        if let Some(previous) = self.sources.records.get(&identity) {
            if previous != record {
                return Err(invalid("admitted source conflicts with recorded history"));
            }

            return Ok(false);
        }

        if self
            .input_versions
            .contains_key(&InputIdentity::Source(identity))
        {
            return Err(invalid("admitted source identity was already recorded"));
        }

        if !matches!(record.origin, SourceOrigin::Disk(_)) {
            return Err(invalid("newly admitted source is not a disk baseline"));
        }

        validate_origin(&record.origin)?;

        let source = record
            .snapshot
            .as_ref()
            .ok_or_else(|| invalid("newly admitted source has no baseline contents"))?;

        source.validate()?;

        if source.version.identity != identity || source.version.revision != record.revision {
            return Err(invalid("admitted source version differs from its record"));
        }

        if let SourceOrigin::Disk(path) = &record.origin {
            if self.sources.disk.contains_key(path) {
                return Err(invalid(
                    "admitted disk path conflicts with recorded history",
                ));
            }

            Arc::make_mut(&mut self.sources)
                .disk
                .insert(path.clone(), identity);
        }

        Arc::make_mut(&mut self.sources)
            .records
            .insert(identity, record.clone());

        Ok(true)
    }
}

pub(crate) fn initial(roots: Vec<PathBuf>) -> ProjectSnapshot {
    let roots: BTreeMap<_, _> = roots
        .into_iter()
        .enumerate()
        .map(|(index, path)| (normalize(&path), ContextIdentity::new(index as u64 + 1)))
        .collect();

    let inventory = InputVersion {
        identity: InputIdentity::Sources,
        revision: 0,
        state: InputState::Sources(BTreeMap::new()),
    };

    let mut input_versions = BTreeMap::from([(InputIdentity::Sources, inventory)]);

    for identity in roots.values() {
        input_versions.insert(
            InputIdentity::Context(*identity),
            InputVersion {
                identity: InputIdentity::Context(*identity),
                revision: 0,
                state: InputState::Policy(0),
            },
        );
    }

    input_versions.insert(
        InputIdentity::Context(ContextIdentity::new(0)),
        InputVersion {
            identity: InputIdentity::Context(ContextIdentity::new(0)),
            revision: 0,
            state: InputState::Policy(0),
        },
    );

    ProjectSnapshot {
        identity: 0,
        sources: Arc::new(SourceStore::default()),
        contexts: Arc::new(ContextStore {
            roots,
            clients: BTreeMap::new(),
        }),
        input_versions: Arc::new(input_versions),
    }
}

fn apply_source(
    store: &mut SourceStore,
    inputs: &mut BTreeMap<InputIdentity, InputVersion>,
    change: SourceChange,
) -> io::Result<()> {
    let opening = matches!(&change, SourceChange::Open { .. });

    let membership = matches!(
        &change,
        SourceChange::Open { .. }
            | SourceChange::Move { .. }
            | SourceChange::Close { .. }
            | SourceChange::Remove { .. }
    );

    let identity = match &change {
        SourceChange::Open { identity, .. } => *identity,

        SourceChange::Replace { expected, .. }
        | SourceChange::Incremental { expected, .. }
        | SourceChange::Move { expected, .. }
        | SourceChange::Close { expected }
        | SourceChange::Remove { expected } => expected.identity,
    };

    let previous_path = store
        .records
        .get(&identity)
        .and_then(|record| record.origin.path())
        .map(Path::to_path_buf);

    let next_path = match &change {
        SourceChange::Open { origin, .. } | SourceChange::Move { origin, .. } => {
            origin.path().map(Path::to_path_buf)
        }

        _ => previous_path.clone(),
    };

    let previous = inputs
        .get(&InputIdentity::Source(identity))
        .map(|input| input.revision);

    match change {
        SourceChange::Open {
            identity,
            origin,
            text,
        } => open_source(store, inputs, identity, origin, text)?,

        SourceChange::Replace { expected, text } => replace_source(store, inputs, expected, text)?,

        SourceChange::Incremental { expected, edits } => {
            incremental_source(store, inputs, expected, edits)?;
        }

        SourceChange::Move { expected, origin } => move_source(store, inputs, expected, origin)?,

        SourceChange::Close { expected } => close_source(store, inputs, expected)?,

        SourceChange::Remove { expected } => remove_source(store, inputs, expected)?,
    }

    if let Some(previous) = previous
        && let Some(input) = inputs.get_mut(&InputIdentity::Source(identity))
    {
        input.revision = previous
            .checked_add(1)
            .ok_or_else(|| invalid("source input revision overflow"))?;
    }

    for path in previous_path.into_iter().chain(next_path) {
        if membership {
            invalidate_directories(inputs, &path, opening)?;
        }

        let snapshot = store
            .records
            .values()
            .filter(|record| {
                record.origin.path() == Some(path.as_path())
                    && !matches!(record.origin, SourceOrigin::Stdin { .. })
                    && record.snapshot.is_some()
            })
            .max_by_key(|record| matches!(record.origin, SourceOrigin::Document { .. }))
            .and_then(|record| record.snapshot.clone());

        let state = snapshot.map_or(InputState::Missing, InputState::Source);
        let identity = InputIdentity::Path(path);

        if inputs
            .get(&identity)
            .is_none_or(|input| input.state != state)
        {
            let revision = inputs
                .get(&identity)
                .map_or(Some(1), |input| input.revision.checked_add(1))
                .ok_or_else(|| invalid("path revision overflow"))?;

            inputs.insert(
                identity.clone(),
                InputVersion {
                    identity,
                    revision,
                    state,
                },
            );
        }
    }

    Ok(())
}

fn invalidate_inputs(
    sources: &SourceStore,
    inputs: &mut BTreeMap<InputIdentity, InputVersion>,
    paths: Vec<PathBuf>,
) -> io::Result<()> {
    let paths = paths
        .into_iter()
        .map(|path| absolute(&path))
        .collect::<io::Result<BTreeSet<_>>>()?;

    let mut identities = BTreeSet::new();

    for path in paths {
        if let Some(parent) = path.parent() {
            identities.insert(InputIdentity::Directory(parent.to_path_buf()));
        }

        identities.insert(InputIdentity::Path(path.clone()));

        for identity in inputs.keys() {
            if matches!(identity, InputIdentity::Directory(candidate) | InputIdentity::Path(candidate)
                if candidate.starts_with(&path) || path.starts_with(candidate))
            {
                identities.insert(identity.clone());
            }
        }
    }

    for identity in identities {
        if inputs
            .get(&identity)
            .is_some_and(|input| input.state == InputState::Stale)
        {
            continue;
        }

        let revision = inputs
            .get(&identity)
            .map_or(Some(1), |input| input.revision.checked_add(1))
            .ok_or_else(|| invalid("input revision overflow"))?;

        if let InputIdentity::Path(path) = &identity {
            invalidate_path(sources, inputs, path)?;
        }

        inputs.insert(
            identity.clone(),
            InputVersion {
                identity,
                revision,
                state: InputState::Stale,
            },
        );
    }

    Ok(())
}

fn invalidate_path(
    sources: &SourceStore,
    inputs: &mut BTreeMap<InputIdentity, InputVersion>,
    path: &Path,
) -> io::Result<()> {
    invalidate_directories(inputs, path, false)?;

    if let Some(source) = sources.disk.get(path)
        && let Some(input) = inputs.get_mut(&InputIdentity::Source(*source))
        && input.state != InputState::Stale
    {
        input.revision = input
            .revision
            .checked_add(1)
            .ok_or_else(|| invalid("source input revision overflow"))?;

        input.state = InputState::Stale;
    }

    Ok(())
}

fn invalidate_directories(
    inputs: &mut BTreeMap<InputIdentity, InputVersion>,
    path: &Path,
    opening: bool,
) -> io::Result<()> {
    for (identity, input) in inputs.iter_mut() {
        let InputIdentity::Directory(directory) = identity else {
            continue;
        };

        let Some(child) = path
            .strip_prefix(directory)
            .ok()
            .and_then(|relative| relative.components().next())
        else {
            continue;
        };

        if input.state == InputState::Stale
            || opening
                && matches!(&input.state, InputState::Directory(entries)
                    if entries.contains(&directory.join(child.as_os_str())))
        {
            continue;
        }

        input.revision = input
            .revision
            .checked_add(1)
            .ok_or_else(|| invalid("directory revision overflow"))?;

        input.state = InputState::Stale;
    }

    Ok(())
}

fn open_source(
    store: &mut SourceStore,
    inputs: &mut BTreeMap<InputIdentity, InputVersion>,
    identity: SourceIdentity,
    origin: SourceOrigin,
    text: Arc<str>,
) -> io::Result<()> {
    validate_origin(&origin)?;

    if store.records.contains_key(&identity) {
        return Err(invalid("source identity is already registered"));
    }

    if let SourceOrigin::Disk(path) = &origin {
        if store.disk.contains_key(path) {
            return Err(invalid("disk path is already registered"));
        }

        store.disk.insert(path.clone(), identity);
    }

    let snapshot = SourceSnapshot::new(
        SourceVersion {
            identity,
            revision: 1,
        },
        text,
    )?;

    store.records.insert(
        identity,
        SourceRecord {
            origin,
            snapshot: Some(snapshot.clone()),
            revision: 1,
        },
    );

    inputs.insert(
        InputIdentity::Source(identity),
        InputVersion {
            identity: InputIdentity::Source(identity),
            revision: 1,
            state: InputState::Source(snapshot),
        },
    );

    Ok(())
}

fn replace_source(
    store: &mut SourceStore,
    inputs: &mut BTreeMap<InputIdentity, InputVersion>,
    expected: SourceVersion,
    text: Arc<str>,
) -> io::Result<()> {
    let record = checked_record(store, expected)?;

    let revision = expected
        .revision
        .checked_add(1)
        .ok_or_else(|| invalid("source revision overflow"))?;

    let snapshot = SourceSnapshot::new(
        SourceVersion {
            identity: expected.identity,
            revision,
        },
        text,
    )?;

    record.snapshot = Some(snapshot.clone());
    record.revision = revision;

    inputs.insert(
        InputIdentity::Source(expected.identity),
        InputVersion {
            identity: InputIdentity::Source(expected.identity),
            revision,
            state: InputState::Source(snapshot),
        },
    );

    Ok(())
}

fn incremental_source(
    store: &mut SourceStore,
    inputs: &mut BTreeMap<InputIdentity, InputVersion>,
    expected: SourceVersion,
    edits: Vec<SourceEdit>,
) -> io::Result<()> {
    let record = checked_record(store, expected)?;

    let mut source = record
        .snapshot
        .clone()
        .ok_or_else(|| invalid("source has no synchronized contents"))?;

    for edit in edits {
        edit.span.validate(&source)?;
        let mut text = source.text.to_string();
        text.replace_range(edit.span.start..edit.span.end, &edit.replacement);
        source = SourceSnapshot::new(source.version, text)?;
    }

    apply_source(
        store,
        inputs,
        SourceChange::Replace {
            expected,
            text: source.text,
        },
    )
}

fn move_source(
    store: &mut SourceStore,
    inputs: &mut BTreeMap<InputIdentity, InputVersion>,
    expected: SourceVersion,
    origin: SourceOrigin,
) -> io::Result<()> {
    validate_origin(&origin)?;

    if let SourceOrigin::Disk(path) = &origin
        && store
            .disk
            .get(path)
            .is_some_and(|identity| *identity != expected.identity)
    {
        return Err(invalid("move destination is already registered"));
    }

    let record = checked_record(store, expected)?;
    let previous = record.origin.path().map(Path::to_path_buf);

    let revision = expected
        .revision
        .checked_add(1)
        .ok_or_else(|| invalid("source revision overflow"))?;

    let contents = &record
        .snapshot
        .as_ref()
        .ok_or_else(|| invalid("moved source has no contents"))?
        .text;

    let snapshot = SourceSnapshot::new(
        SourceVersion {
            identity: expected.identity,
            revision,
        },
        Arc::clone(contents),
    )?;

    record.origin = origin.clone();
    record.snapshot = Some(snapshot.clone());
    record.revision = revision;

    if let Some(previous) = previous
        && store.disk.get(&previous) == Some(&expected.identity)
    {
        store.disk.remove(&previous);
    }

    if let SourceOrigin::Disk(path) = origin {
        store.disk.insert(path, expected.identity);
    }

    inputs.insert(
        InputIdentity::Source(expected.identity),
        InputVersion {
            identity: InputIdentity::Source(expected.identity),
            revision,
            state: InputState::Source(snapshot),
        },
    );

    Ok(())
}

fn close_source(
    store: &mut SourceStore,
    inputs: &mut BTreeMap<InputIdentity, InputVersion>,
    expected: SourceVersion,
) -> io::Result<()> {
    let record = checked_record(store, expected)?;

    if !matches!(record.origin, SourceOrigin::Document { .. }) {
        return Err(invalid("only synchronized documents can be closed"));
    }

    record.snapshot = None;

    record.revision = record
        .revision
        .checked_add(1)
        .ok_or_else(|| invalid("source revision overflow"))?;

    inputs.insert(
        InputIdentity::Source(expected.identity),
        InputVersion {
            identity: InputIdentity::Source(expected.identity),
            revision: record.revision,
            state: InputState::Missing,
        },
    );

    Ok(())
}

fn remove_source(
    store: &mut SourceStore,
    inputs: &mut BTreeMap<InputIdentity, InputVersion>,
    expected: SourceVersion,
) -> io::Result<()> {
    let record = checked_record(store, expected)?.clone();

    if let SourceOrigin::Disk(path) = record.origin {
        store.disk.remove(&path);
    }

    store.records.remove(&expected.identity);

    inputs.insert(
        InputIdentity::Source(expected.identity),
        InputVersion {
            identity: InputIdentity::Source(expected.identity),
            revision: expected
                .revision
                .checked_add(1)
                .ok_or_else(|| invalid("source revision overflow"))?,
            state: InputState::Missing,
        },
    );

    Ok(())
}

fn checked_record(
    store: &mut SourceStore,
    expected: SourceVersion,
) -> io::Result<&mut SourceRecord> {
    let record = store
        .records
        .get_mut(&expected.identity)
        .ok_or_else(|| invalid("unknown source identity"))?;

    if record.revision != expected.revision {
        return Err(invalid("source revision mismatch"));
    }

    Ok(record)
}

fn advance_inventory(
    sources: &SourceStore,
    inputs: &mut BTreeMap<InputIdentity, InputVersion>,
    additions: u64,
) -> io::Result<()> {
    let identity = InputIdentity::Sources;

    let revision = inputs
        .get(&identity)
        .and_then(|input| input.revision.checked_add(additions))
        .ok_or_else(|| invalid("source inventory revision overflow"))?;

    inputs.insert(
        identity.clone(),
        InputVersion {
            identity,
            revision,
            state: InputState::Sources(sources.records.clone()),
        },
    );

    Ok(())
}

fn advance_policy(
    inputs: &mut BTreeMap<InputIdentity, InputVersion>,
    context: ContextIdentity,
) -> io::Result<()> {
    let identity = InputIdentity::Context(context);

    let revision = inputs
        .get(&identity)
        .map_or(Some(1), |input| input.revision.checked_add(1))
        .ok_or_else(|| invalid("context revision overflow"))?;

    inputs.insert(
        identity.clone(),
        InputVersion {
            identity,
            revision,
            state: InputState::Policy(revision),
        },
    );

    Ok(())
}

fn validate_input(input: &InputVersion) -> io::Result<()> {
    match &input.identity {
        InputIdentity::Directory(path) | InputIdentity::Path(path) => {
            absolute(path)?;
        }

        _ => {}
    }

    if input.state == InputState::NonDirectory
        && !matches!(input.identity, InputIdentity::Directory(_))
    {
        return Err(invalid("non-directory state requires a directory identity"));
    }

    if let InputState::Directory(paths) = &input.state {
        for path in paths {
            absolute(path)?;
        }
    }

    Ok(())
}

fn validate_origin(origin: &SourceOrigin) -> io::Result<()> {
    if let Some(path) = origin.path() {
        absolute(path)?;
    }

    if matches!(origin, SourceOrigin::Document { address, .. } | SourceOrigin::Virtual { address } if address.is_empty() || address.contains('\0'))
    {
        return Err(invalid(
            "source address must be nonempty and contain no NUL",
        ));
    }

    Ok(())
}

pub(crate) fn absolute(path: &Path) -> io::Result<PathBuf> {
    if !path.is_absolute() {
        return Err(invalid("path must be absolute"));
    }

    Ok(normalize(path))
}

pub(crate) fn normalize(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();

    for component in path.components() {
        match component {
            Component::CurDir => {}

            Component::ParentDir => {
                normalized.pop();
            }

            component => normalized.push(component.as_os_str()),
        }
    }

    normalized
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}
