use std::{
    collections::{BTreeMap, BTreeSet},
    fs, io,
    path::{Path, PathBuf},
    sync::{Arc, PoisonError},
};

use instar_shared::{Event, EventPayload, Options, SourceIdentity, SourceSnapshot, SourceVersion};

use crate::{
    change::{ChangeBatch, ProjectChange, SourceChange},
    execution::Host,
    inputs::{InputIdentity, InputState, InputVersion, ProjectSnapshot, SourceOrigin, absolute},
};

pub(crate) struct Capture {
    pub(crate) host: Arc<Host>,
    pub(crate) snapshot: ProjectSnapshot,
    pub(crate) inputs: BTreeMap<InputIdentity, InputVersion>,
    reads: BTreeSet<PathBuf>,
    consultation: Vec<Vec<InputVersion>>,
    frozen: bool,
}

impl Capture {
    pub(crate) fn new(host: Arc<Host>, snapshot: ProjectSnapshot) -> Self {
        Self {
            host,
            snapshot,
            inputs: BTreeMap::new(),
            reads: BTreeSet::new(),
            consultation: Vec::new(),
            frozen: true,
        }
    }

    pub(crate) fn live(host: Arc<Host>) -> Self {
        let snapshot = host.snapshot();

        Self {
            host,
            snapshot,
            inputs: BTreeMap::new(),
            reads: BTreeSet::new(),
            consultation: Vec::new(),
            frozen: false,
        }
    }

    pub(crate) fn source(&mut self, path: &Path, options: &Options) -> io::Result<SourceSnapshot> {
        self.read(path, options)?.ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                format!("source not found: {}", path.display()),
            )
        })
    }

    pub(crate) fn read(
        &mut self,
        path: &Path,
        options: &Options,
    ) -> io::Result<Option<SourceSnapshot>> {
        options.check()?;
        let path = absolute(path)?;

        options.report(Event {
            invocation: 0,
            operation: 0,
            payload: EventPayload::InputPath(path.clone()),
        });

        let identity = InputIdentity::Path(path.clone());

        let refreshing = self
            .snapshot
            .input_versions
            .get(&identity)
            .is_some_and(|input| input.state == InputState::Stale);

        let captured = if self.frozen {
            self.snapshot.input_versions.get(&identity)
        } else if self.reads.contains(&path) {
            self.inputs.get(&identity)
        } else {
            None
        }
        .cloned();

        if let Some(input) = captured {
            match &input.state {
                InputState::Source(source) => {
                    let source = source.clone();
                    self.consult_input(input);
                    self.consult_source(&source);

                    return Ok(Some(source));
                }

                InputState::Missing => {
                    self.consult_input(input);
                    self.consult_closed(&path);

                    return Ok(None);
                }

                _ => {}
            }
        }

        if let Some(source) = self.snapshot.source_at(&path).cloned()
            && (self.frozen && !refreshing
                || matches!(
                    self.snapshot.origin(source.version.identity),
                    Some(SourceOrigin::Document { .. })
                ))
        {
            self.consult_source(&source);

            if let Some(input) = self.snapshot.input_versions.get(&identity).cloned() {
                self.consult_input(input);
            }

            self.reads.insert(path.clone());

            return Ok(Some(source));
        }

        if self.frozen && !refreshing && self.absent(&path) {
            let baseline = matches!(read_bytes(&path), Ok(None));
            options.check()?;
            self.record(identity, InputState::Missing, baseline)?;

            return Ok(None);
        }

        let bytes = read_bytes(&path)?;
        options.check()?;
        let source = self.read_disk(&path, bytes, refreshing)?;

        self.observation(
            identity,
            source.as_ref().map_or(InputState::Missing, |source| {
                InputState::Source(source.clone())
            }),
        )?;

        if let Some(source) = &source {
            self.consult_source(source);
        } else {
            self.consult_closed(&path);
        }

        self.reads.insert(path);

        Ok(source)
    }

    pub(crate) fn listing(
        &mut self,
        path: &Path,
        options: &Options,
    ) -> io::Result<Option<Vec<PathBuf>>> {
        options.check()?;
        let path = absolute(path)?;

        options.report(Event {
            invocation: 0,
            operation: 0,
            payload: EventPayload::InputPath(path.clone()),
        });

        let identity = InputIdentity::Directory(path.clone());

        let refreshing = self
            .snapshot
            .input_versions
            .get(&identity)
            .is_some_and(|input| input.state == InputState::Stale);

        let captured = if self.frozen {
            self.snapshot.input_versions.get(&identity)
        } else {
            self.inputs.get(&identity)
        }
        .cloned();

        if let Some(input) = captured {
            let paths = match &input.state {
                InputState::Directory(paths) => Some(Some(paths.clone())),
                InputState::Missing => Some(None),
                _ => None,
            };

            if let Some(paths) = paths {
                self.consult_input(input);

                return Ok(paths);
            }
        }

        if self.frozen
            && !refreshing
            && (self
                .snapshot
                .input_versions
                .get(&InputIdentity::Path(path.clone()))
                .is_some_and(|input| {
                    matches!(input.state, InputState::Source(_) | InputState::Missing)
                })
                || self.absent(&path))
        {
            let baseline = matches!(read_listing(&path, Some(options)), Ok(None));
            options.check()?;
            self.record(identity, InputState::Missing, baseline)?;

            return Ok(None);
        }

        let paths = read_listing(&path, Some(options))?;

        self.observation(
            identity,
            paths.as_ref().map_or(InputState::Missing, |paths| {
                InputState::Directory(paths.clone())
            }),
        )?;

        Ok(paths)
    }

    pub(crate) fn admit(
        &mut self,
        observed: &ProjectSnapshot,
        protected: &BTreeSet<InputIdentity>,
    ) -> io::Result<()> {
        for (identity, record) in &observed.sources.records {
            if protected.contains(&InputIdentity::Source(*identity))
                || record.origin.path().is_some_and(|path| {
                    protected.contains(&InputIdentity::Path(path.to_path_buf()))
                })
                || self.snapshot.sources.records.contains_key(identity)
            {
                continue;
            }

            let SourceOrigin::Disk(path) = &record.origin else {
                return Err(invalid("admitted source is not a disk baseline"));
            };

            let source = record
                .snapshot
                .as_ref()
                .ok_or_else(|| invalid("admitted source has no baseline contents"))?;

            if read_bytes(path)?.as_deref() != Some(source.text.as_bytes()) {
                return Err(invalid("admitted source differs from its disk baseline"));
            }
        }

        for (identity, input) in observed.input_versions.iter() {
            if protected.contains(identity) || self.snapshot.input_versions.contains_key(identity) {
                continue;
            }

            let unchanged = match (identity, &input.state) {
                (InputIdentity::Path(path), InputState::Missing) => read_bytes(path)?.is_none(),

                (InputIdentity::Directory(path), InputState::Missing) => {
                    read_listing(path, None)?.is_none()
                }

                (InputIdentity::Directory(path), InputState::Directory(paths)) => {
                    read_listing(path, None)?.as_ref() == Some(paths)
                }

                _ => true,
            };

            if !unchanged {
                return Err(invalid("admitted input differs from its disk baseline"));
            }
        }

        let proposed = self.snapshot.admit(observed, protected)?;

        let mut current = self
            .host
            .snapshot
            .write()
            .unwrap_or_else(PoisonError::into_inner);

        let mut excluded = except(&proposed, &BTreeSet::new());

        for (identity, record) in &proposed.sources.records {
            if self.snapshot.sources.records.contains_key(identity) {
                continue;
            }

            let SourceOrigin::Disk(path) = &record.origin else {
                continue;
            };

            let compatible = match current.sources.records.get(identity) {
                Some(previous) => previous == record,

                None => {
                    !current
                        .input_versions
                        .contains_key(&InputIdentity::Source(*identity))
                        && !current.sources.disk.contains_key(path)
                        && !current
                            .input_versions
                            .contains_key(&InputIdentity::Path(path.clone()))
                        && current.source_at(path).is_none()
                }
            };

            if compatible {
                excluded.remove(&InputIdentity::Source(*identity));
            }
        }

        for (identity, input) in proposed.input_versions.iter() {
            if self.snapshot.input_versions.contains_key(identity)
                || *identity == InputIdentity::Sources
                || matches!(identity, InputIdentity::Source(_))
            {
                continue;
            }

            if current
                .input_versions
                .get(identity)
                .is_some_and(|previous| previous != input)
            {
                continue;
            }

            if let InputState::Source(source) = &input.state
                && current.source(source.version.identity) != Some(source)
                && excluded.contains(&InputIdentity::Source(source.version.identity))
            {
                continue;
            }

            excluded.remove(identity);
        }

        *current = current.admit(&proposed, &excluded)?;
        self.snapshot = proposed;

        Ok(())
    }

    pub(crate) fn begin_consultation(&mut self) {
        self.consultation.push(Vec::new());
    }

    pub(crate) fn end_consultation(&mut self) -> Vec<InputVersion> {
        self.consultation.pop().unwrap_or_default()
    }

    pub(crate) fn consult_input(&mut self, input: InputVersion) {
        for consultation in &mut self.consultation {
            if !consultation.contains(&input) {
                consultation.push(input.clone());
            }
        }

        self.inputs.insert(input.identity.clone(), input);
    }

    pub(crate) fn consult_source(&mut self, source: &SourceSnapshot) {
        let identity = InputIdentity::Source(source.version.identity);

        if let Some(input) = self.snapshot.input_versions.get(&identity).cloned() {
            self.consult_input(input);
        }

        if let Some(SourceOrigin::Disk(path)) = self.snapshot.origin(source.version.identity) {
            let identity = InputIdentity::Path(path.clone());

            if let Some(input) = self.snapshot.input_versions.get(&identity).cloned() {
                self.consult_input(input);
            }
        }

        let context = InputIdentity::Context(self.snapshot.context(source.version.identity));

        if let Some(input) = self.snapshot.input_versions.get(&context).cloned() {
            self.consult_input(input);
        }
    }

    pub(crate) fn consult_inventory(&mut self) {
        if let Some(input) = self
            .snapshot
            .input_versions
            .get(&InputIdentity::Sources)
            .cloned()
        {
            self.consult_input(input);
        }

        let inputs = self
            .snapshot
            .input_versions
            .iter()
            .filter(|(identity, _)| matches!(identity, InputIdentity::Context(_)))
            .map(|(_, input)| input.clone())
            .collect::<Vec<_>>();

        for input in inputs {
            self.consult_input(input);
        }
    }

    pub(crate) fn observation(
        &mut self,
        identity: InputIdentity,
        state: InputState,
    ) -> io::Result<()> {
        self.record(identity, state, true)
    }

    pub(crate) fn observe(&mut self, batch: ChangeBatch) -> io::Result<()> {
        let proposed = self.snapshot.transition(batch.clone())?;

        if !self.frozen {
            let mut current = self
                .host
                .snapshot
                .write()
                .unwrap_or_else(PoisonError::into_inner);

            *current = current.transition(batch)?;
        }

        self.snapshot = proposed;

        Ok(())
    }

    pub(crate) fn guards(&self) -> Vec<InputVersion> {
        self.inputs.values().cloned().collect()
    }

    pub(crate) fn frozen(&self) -> bool {
        self.frozen
    }

    fn read_disk(
        &mut self,
        path: &Path,
        bytes: Option<Vec<u8>>,
        refreshing: bool,
    ) -> io::Result<Option<SourceSnapshot>> {
        let previous = self.snapshot.source_at(path).cloned();

        if let Some(bytes) = bytes {
            if self.frozen
                && !refreshing
                && previous.is_none()
                && let Some(registered) = self.adopt_disk(path, &bytes)?
            {
                return Ok(Some(registered));
            }

            let identity = previous.as_ref().map_or_else(
                || self.host.source_identity(),
                |source| source.version.identity,
            );

            let revision = previous
                .as_ref()
                .map_or(1, |source| source.version.revision);

            let mut source =
                SourceSnapshot::from_bytes(SourceVersion { identity, revision }, &bytes)?;

            if previous
                .as_ref()
                .is_some_and(|previous| previous.text != source.text)
            {
                source.version.revision = revision
                    .checked_add(1)
                    .ok_or_else(|| io::Error::other("source revision overflow"))?;
            }

            if previous.as_ref() != Some(&source) {
                let change = if let Some(previous) = &previous {
                    SourceChange::Replace {
                        expected: previous.version,
                        text: Arc::clone(&source.text),
                    }
                } else {
                    SourceChange::Open {
                        identity,
                        origin: SourceOrigin::Disk(path.to_path_buf()),
                        text: Arc::clone(&source.text),
                    }
                };

                self.observe_disk(change, path, identity, refreshing)?;
            }

            Ok(Some(source))
        } else {
            if let Some(previous) = previous {
                self.observe_disk(
                    SourceChange::Remove {
                        expected: previous.version,
                    },
                    path,
                    previous.version.identity,
                    refreshing,
                )?;

                let identity = InputIdentity::Source(previous.version.identity);

                if let Some(input) = self.snapshot.input_versions.get(&identity).cloned() {
                    self.consult_input(input);
                }
            }

            Ok(None)
        }
    }

    fn adopt_disk(&mut self, path: &Path, bytes: &[u8]) -> io::Result<Option<SourceSnapshot>> {
        let mut current = self.host.snapshot();

        let registered = current
            .sources
            .disk
            .get(path)
            .and_then(|identity| current.source(*identity))
            .cloned();

        let Some(registered) = registered else {
            return Ok(None);
        };

        if registered.text.as_bytes() != bytes
            || self
                .snapshot
                .input_versions
                .contains_key(&InputIdentity::Source(registered.version.identity))
        {
            return Ok(None);
        }

        let identity = InputIdentity::Path(path.to_path_buf());
        let mut allowed = BTreeSet::from([InputIdentity::Source(registered.version.identity)]);

        if current.source_at(path) == Some(&registered)
            && current.input_versions.get(&identity).is_some_and(|input| {
                input.state == InputState::Source(registered.clone())
                    || input.state == InputState::Stale
            })
        {
            allowed.insert(identity.clone());
        } else {
            Arc::make_mut(&mut current.input_versions).remove(&identity);
        }

        self.snapshot = self.snapshot.admit(&current, &except(&current, &allowed))?;
        self.record(identity, InputState::Source(registered.clone()), true)?;

        Ok(Some(registered))
    }

    fn observe_disk(
        &mut self,
        change: SourceChange,
        path: &Path,
        source: SourceIdentity,
        refreshing: bool,
    ) -> io::Result<()> {
        let opening = matches!(change, SourceChange::Open { .. });
        let identity = InputIdentity::Path(path.to_path_buf());

        let batch = ChangeBatch {
            changes: vec![ProjectChange::Source(change)],
        };

        if self.frozen && refreshing {
            self.refresh(batch, &identity, source)?;
        } else if self.frozen && opening {
            let proposed = self.snapshot.transition(batch.clone())?;

            let mut current = self
                .host
                .snapshot
                .write()
                .unwrap_or_else(PoisonError::into_inner);

            if !current.input_versions.contains_key(&identity)
                && !current.sources.disk.contains_key(path)
                && current.source_at(path).is_none()
            {
                *current = current.transition(batch)?;
            }

            self.snapshot = proposed;
        } else {
            self.observe(batch)?;
        }

        Ok(())
    }

    fn refresh(
        &mut self,
        batch: ChangeBatch,
        path: &InputIdentity,
        source: SourceIdentity,
    ) -> io::Result<()> {
        let proposed = self.snapshot.transition(batch.clone())?;

        let mut current = self
            .host
            .snapshot
            .write()
            .unwrap_or_else(PoisonError::into_inner);

        if current.input_versions.get(path) == self.snapshot.input_versions.get(path) {
            let identity = InputIdentity::Source(source);

            if current.input_versions.get(&identity) != self.snapshot.input_versions.get(&identity)
                || current.origin(source) != self.snapshot.origin(source)
            {
                return Err(invalid("stale source conflicts with live history"));
            }

            *current = current.transition(batch)?;
        }

        self.snapshot = proposed;

        Ok(())
    }

    fn record(
        &mut self,
        identity: InputIdentity,
        state: InputState,
        baseline: bool,
    ) -> io::Result<()> {
        let expected = self.snapshot.input_versions.get(&identity).cloned();

        if self.frozen
            && let Some(input) = &expected
        {
            if input.state == state {
                self.consult_input(input.clone());

                return Ok(());
            }

            if input.state != InputState::Stale || !baseline {
                return Err(invalid("observation conflicts with frozen input"));
            }
        }

        if self.frozen && baseline {
            let mut current = self
                .host
                .snapshot
                .write()
                .unwrap_or_else(PoisonError::into_inner);

            if let Some(input) = current.input_versions.get(&identity)
                && input.state == state
                && expected.is_none()
            {
                let protected = except(&current, &BTreeSet::from([identity.clone()]));
                self.snapshot = self.snapshot.admit(&current, &protected)?;
                let input = input.clone();
                drop(current);
                self.consult_input(input);

                return Ok(());
            }

            let publish = match (&identity, &state) {
                (InputIdentity::Path(path), InputState::Missing) => {
                    current.source_at(path).is_none() && matches!(read_bytes(path), Ok(None))
                }

                (InputIdentity::Directory(path), InputState::Missing) => {
                    matches!(read_listing(path, None), Ok(None))
                }

                (InputIdentity::Directory(path), InputState::Directory(paths)) => {
                    read_listing(path, None)?.as_ref() == Some(paths)
                }

                (InputIdentity::Path(path), InputState::Source(source)) => {
                    current.source_at(path) == Some(source)
                }

                (_, InputState::Source(source)) => {
                    current.source(source.version.identity) == Some(source)
                }

                _ => false,
            };

            let revision = expected
                .as_ref()
                .map_or(Some(1), |input| input.revision.checked_add(1))
                .ok_or_else(|| invalid("input revision overflow"))?;

            let input = InputVersion {
                identity: identity.clone(),
                revision,
                state,
            };

            let batch = ChangeBatch {
                changes: vec![ProjectChange::Input {
                    expected: expected.clone(),
                    current: input.clone(),
                }],
            };

            let proposed = self.snapshot.transition(batch.clone())?;

            if publish && current.input_versions.get(&identity) == expected.as_ref() {
                *current = current.transition(batch)?;
            }

            self.snapshot = proposed;
            drop(current);
            self.consult_input(input);

            return Ok(());
        }

        let revision = match &expected {
            Some(previous) if previous.state == state => previous.revision,

            Some(previous) => previous
                .revision
                .checked_add(1)
                .ok_or_else(|| invalid("input revision overflow"))?,

            None => 1,
        };

        let current = InputVersion {
            identity,
            revision,
            state,
        };

        if expected.as_ref() != Some(&current) {
            self.observe(ChangeBatch {
                changes: vec![ProjectChange::Input {
                    expected,
                    current: current.clone(),
                }],
            })?;
        }

        self.consult_input(current);

        Ok(())
    }

    fn consult_closed(&mut self, path: &Path) {
        let inputs = self
            .snapshot
            .sources
            .records
            .iter()
            .filter(|(_, record)| record.snapshot.is_none() && record.origin.path() == Some(path))
            .filter_map(|(identity, _)| {
                self.snapshot
                    .input_versions
                    .get(&InputIdentity::Source(*identity))
                    .cloned()
            })
            .collect::<Vec<_>>();

        for input in inputs {
            self.consult_input(input);
        }
    }

    fn absent(&mut self, path: &Path) -> bool {
        for ancestor in path.ancestors().skip(1) {
            let identity = InputIdentity::Directory(ancestor.to_path_buf());

            let Some(input) = self.snapshot.input_versions.get(&identity).cloned() else {
                continue;
            };

            let absent = match &input.state {
                InputState::Missing => true,

                InputState::Directory(entries) => path
                    .strip_prefix(ancestor)
                    .ok()
                    .and_then(|relative| relative.components().next())
                    .is_some_and(|component| {
                        !entries.contains(&ancestor.join(component.as_os_str()))
                    }),

                _ => false,
            };

            if absent {
                self.consult_input(input);

                return true;
            }
        }

        false
    }
}

fn except(
    snapshot: &ProjectSnapshot,
    allowed: &BTreeSet<InputIdentity>,
) -> BTreeSet<InputIdentity> {
    snapshot
        .input_versions
        .keys()
        .filter(|identity| !allowed.contains(*identity))
        .cloned()
        .collect()
}

fn read_bytes(path: &Path) -> io::Result<Option<Vec<u8>>> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),

        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
            ) =>
        {
            Ok(None)
        }

        Err(error) => Err(io::Error::new(
            error.kind(),
            format!("{}: {error}", path.display()),
        )),
    }
}

fn read_listing(path: &Path, options: Option<&Options>) -> io::Result<Option<Vec<PathBuf>>> {
    match fs::read_dir(path) {
        Ok(entries) => {
            let mut paths = Vec::new();

            for entry in entries {
                if let Some(options) = options {
                    options.check()?;
                }

                paths.push(entry?.path());
            }

            paths.sort();

            Ok(Some(paths))
        }

        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
            ) =>
        {
            Ok(None)
        }

        Err(error) => Err(error),
    }
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}
