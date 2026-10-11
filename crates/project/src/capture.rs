use std::{
    collections::{BTreeMap, BTreeSet},
    fs, io,
    path::{Path, PathBuf},
    sync::{Arc, PoisonError},
};

use instar_shared::{Event, EventPayload, Options, Reason, SourceSnapshot, SourceVersion};

use crate::{
    change::{ChangeBatch, ProjectChange, SourceChange},
    execution::Host,
    inputs::{InputIdentity, InputState, InputVersion, ProjectSnapshot, SourceOrigin, absolute},
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Listing {
    Missing,
    NonDirectory,
    Directory(Vec<PathBuf>),
}

impl Listing {
    pub(crate) fn state(&self) -> InputState {
        match self {
            Self::Missing => InputState::Missing,
            Self::NonDirectory => InputState::NonDirectory,
            Self::Directory(paths) => InputState::Directory(paths.clone()),
        }
    }
}

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
            .is_some_and(|input| input.state == InputState::Stale)
            || self.snapshot.source_at(&path).is_some_and(|source| {
                matches!(
                    self.snapshot.origin(source.version.identity),
                    Some(SourceOrigin::Disk(_))
                ) && self
                    .snapshot
                    .input_versions
                    .get(&InputIdentity::Source(source.version.identity))
                    .is_some_and(|input| input.state == InputState::Stale)
            });

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
                InputState::Source(source) if !refreshing => {
                    let source = source.clone();
                    self.consult_input(input);
                    self.consult_source(&source);

                    return Ok(Some(source));
                }

                InputState::Missing if !refreshing => {
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

        let directories = self
            .inputs
            .iter()
            .filter_map(|(identity, input)| {
                if let InputIdentity::Directory(directory) = identity
                    && input.state != InputState::Stale
                    && self
                        .snapshot
                        .input_versions
                        .get(identity)
                        .is_some_and(|current| current.state == InputState::Stale)
                {
                    Some(directory.clone())
                } else {
                    None
                }
            })
            .collect::<Vec<_>>();

        for directory in directories {
            self.listing(&directory, options)?;
        }

        if let Some(source) = &source {
            self.consult_source(source);
        } else {
            self.consult_closed(&path);
        }

        self.reads.insert(path);

        Ok(source)
    }

    pub(crate) fn listing(&mut self, path: &Path, options: &Options) -> io::Result<Listing> {
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

        if !refreshing && let Some(input) = captured {
            let paths = match &input.state {
                InputState::Directory(paths) => Some(Listing::Directory(paths.clone())),
                InputState::Missing => Some(Listing::Missing),
                InputState::NonDirectory => Some(Listing::NonDirectory),
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
            let state = if self
                .snapshot
                .input_versions
                .get(&InputIdentity::Path(path.clone()))
                .is_some_and(|input| matches!(input.state, InputState::Source(_)))
                || path.ancestors().skip(1).any(|ancestor| {
                    self.snapshot
                        .input_versions
                        .get(&InputIdentity::Directory(ancestor.to_path_buf()))
                        .is_some_and(|input| input.state == InputState::NonDirectory)
                }) {
                Listing::NonDirectory
            } else {
                Listing::Missing
            };

            let baseline = read_listing(&path, Some(options))? == state;
            options.check()?;
            self.record(identity, state.state(), baseline)?;

            return Ok(state);
        }

        let paths = read_listing(&path, Some(options))?;
        self.observation(identity, paths.state())?;

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

                (
                    InputIdentity::Directory(path),
                    state @ (InputState::Missing
                    | InputState::NonDirectory
                    | InputState::Directory(_)),
                ) => read_listing(path, None)?.state() == *state,

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

    pub(crate) fn admit_inventory(&mut self, options: &Options) -> io::Result<()> {
        let observed = self.host.snapshot();
        let mut allowed = BTreeSet::new();

        for (identity, record) in &observed.sources.records {
            options.check()?;

            let SourceOrigin::Disk(path) = &record.origin else {
                continue;
            };

            if self.snapshot.sources.records.contains_key(identity)
                || self
                    .snapshot
                    .input_versions
                    .contains_key(&InputIdentity::Source(*identity))
                || self
                    .snapshot
                    .input_versions
                    .contains_key(&InputIdentity::Path(path.clone()))
                || observed.source_at(path) != record.snapshot.as_ref()
                || self.absent(path)
            {
                continue;
            }

            allowed.insert(InputIdentity::Source(*identity));
            allowed.insert(InputIdentity::Path(path.clone()));
        }

        if !allowed.is_empty() {
            self.admit(&observed, &except(&observed, &allowed))?;
        }

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
            if let Some(previous) = consultation
                .iter_mut()
                .find(|previous| previous.identity == input.identity)
            {
                *previous = input.clone();
            } else {
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

    pub(crate) fn declaration(&mut self, source: &SourceSnapshot) -> io::Result<()> {
        if self.snapshot.source(source.version.identity) != Some(source) {
            return Err(invalid("declaration differs from its captured source"));
        }

        self.consult_source(source);

        Arc::make_mut(&mut self.snapshot.sources)
            .declarations
            .insert(source.version);

        Ok(())
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
        self.inputs
            .values()
            .map(|input| match input.identity {
                InputIdentity::Sources => {
                    self.snapshot.input_versions[&InputIdentity::Sources].clone()
                }

                _ => input.clone(),
            })
            .collect()
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
        let host = Arc::clone(&self.host);

        let mut current = host
            .snapshot
            .write()
            .unwrap_or_else(PoisonError::into_inner);

        let path_identity = InputIdentity::Path(path.to_path_buf());

        if !self.frozen
            && let Some(source) = current.source_at(path)
            && matches!(
                current.origin(source.version.identity),
                Some(SourceOrigin::Document { .. })
            )
        {
            let source = source.clone();
            self.snapshot = current.clone();

            return Ok(Some(source));
        }

        let registered = current
            .sources
            .disk
            .get(path)
            .and_then(|identity| current.source(*identity))
            .cloned();

        if self.snapshot.source_at(path).is_none()
            && let (Some(registered), Some(bytes)) = (&registered, &bytes)
            && registered.text.as_bytes() == bytes.as_slice()
            && !self
                .snapshot
                .input_versions
                .contains_key(&InputIdentity::Source(registered.version.identity))
        {
            let allowed = BTreeSet::from([
                InputIdentity::Source(registered.version.identity),
                path_identity.clone(),
            ]);

            let mut observed = current.clone();

            if self
                .snapshot
                .input_versions
                .get(&path_identity)
                .is_some_and(|input| input.state != InputState::Stale)
                || current.source_at(path) != Some(registered)
            {
                Arc::make_mut(&mut observed.input_versions).remove(&path_identity);
            }

            self.snapshot = self
                .snapshot
                .admit(&observed, &except(&current, &allowed))?;
        }

        let previous = self.snapshot.source_at(path).cloned();
        let source_identity = previous.as_ref().map(|source| source.version.identity);
        let (mut proposed, batch) = self.disk_observation(path, bytes)?;
        let source = proposed.source_at(path).cloned();

        let identity = source
            .as_ref()
            .map(|source| source.version.identity)
            .or(source_identity);

        let compatible = current.input_versions.get(&path_identity)
            == self.snapshot.input_versions.get(&path_identity)
            && identity.is_none_or(|identity| {
                current.input_versions.get(&InputIdentity::Source(identity))
                    == self
                        .snapshot
                        .input_versions
                        .get(&InputIdentity::Source(identity))
                    && current.origin(identity) == self.snapshot.origin(identity)
            })
            && (previous.is_some() || registered.is_none());

        let published = current.input_versions.get(&path_identity)
            == proposed.input_versions.get(&path_identity)
            && identity.is_none_or(|identity| {
                current.input_versions.get(&InputIdentity::Source(identity))
                    == proposed
                        .input_versions
                        .get(&InputIdentity::Source(identity))
                    && current.sources.records.get(&identity)
                        == proposed.sources.records.get(&identity)
            });

        if !batch.changes.is_empty() && compatible {
            *current = current.transition(batch)?;
        } else if (refreshing || !self.frozen) && !published && !compatible {
            return Err(invalid("stale source conflicts with live history"));
        }

        if published {
            for (identity, input) in self.snapshot.input_versions.iter() {
                if let InputIdentity::Directory(directory) = identity
                    && input.state != InputState::Stale
                    && current.input_versions.get(identity) == Some(input)
                    && proposed
                        .input_versions
                        .get(identity)
                        .is_some_and(|observed| observed.state == InputState::Stale)
                    && read_listing(directory, None)?.state() == input.state
                {
                    Arc::make_mut(&mut proposed.input_versions)
                        .insert(identity.clone(), input.clone());
                }
            }
        }

        self.snapshot = proposed;
        drop(current);

        if source.is_none()
            && let Some(identity) = identity
            && let Some(input) = self
                .snapshot
                .input_versions
                .get(&InputIdentity::Source(identity))
                .cloned()
        {
            self.consult_input(input);
        }

        Ok(source)
    }

    fn disk_observation(
        &self,
        path: &Path,
        bytes: Option<Vec<u8>>,
    ) -> io::Result<(ProjectSnapshot, ChangeBatch)> {
        let previous = self.snapshot.source_at(path);
        let mut batch = ChangeBatch::default();

        match (previous, bytes) {
            (previous, Some(bytes)) => {
                let identity = previous.map_or_else(
                    || self.host.source_identity(),
                    |source| source.version.identity,
                );

                let revision = previous.map_or(1, |source| source.version.revision);

                let source =
                    SourceSnapshot::from_bytes(SourceVersion { identity, revision }, &bytes)?;

                let change = match previous {
                    Some(previous) if previous.text != source.text => Some(SourceChange::Replace {
                        expected: previous.version,
                        text: Arc::clone(&source.text),
                    }),

                    None => Some(SourceChange::Open {
                        identity,
                        origin: SourceOrigin::Disk(path.to_path_buf()),
                        text: Arc::clone(&source.text),
                    }),

                    Some(_) => None,
                };

                if let Some(change) = change {
                    batch.changes.push(ProjectChange::Source(change));
                } else if let Some(expected) = self
                    .snapshot
                    .input_versions
                    .get(&InputIdentity::Source(identity))
                    .cloned()
                    && expected.state == InputState::Stale
                {
                    batch.changes.push(ProjectChange::Input {
                        current: InputVersion {
                            identity: expected.identity.clone(),
                            revision: expected
                                .revision
                                .checked_add(1)
                                .ok_or_else(|| invalid("input revision overflow"))?,
                            state: InputState::Source(source),
                        },
                        expected: Some(expected),
                    });
                }
            }

            (Some(previous), None) => {
                batch
                    .changes
                    .push(ProjectChange::Source(SourceChange::Remove {
                        expected: previous.version,
                    }));
            }

            (None, None) => {}
        }

        let mut proposed = if batch.changes.is_empty() {
            self.snapshot.clone()
        } else {
            self.snapshot.transition(batch.clone())?
        };

        let state = proposed
            .source_at(path)
            .cloned()
            .map_or(InputState::Missing, InputState::Source);

        let identity = InputIdentity::Path(path.to_path_buf());
        let expected = proposed.input_versions.get(&identity).cloned();

        if expected.as_ref().is_none_or(|input| input.state != state) {
            batch.changes.push(ProjectChange::Input {
                current: InputVersion {
                    identity,
                    revision: expected
                        .as_ref()
                        .map_or(Some(1), |input| input.revision.checked_add(1))
                        .ok_or_else(|| invalid("input revision overflow"))?,
                    state,
                },
                expected,
            });

            proposed = self.snapshot.transition(batch.clone())?;
        }

        Ok((proposed, batch))
    }

    fn record(
        &mut self,
        identity: InputIdentity,
        state: InputState,
        baseline: bool,
    ) -> io::Result<()> {
        let expected = self.snapshot.input_versions.get(&identity).cloned();

        if let Some(input) = &expected {
            if input.state == state {
                self.consult_input(input.clone());

                return Ok(());
            }

            if self.frozen && (input.state != InputState::Stale || !baseline) {
                return Err(invalid("observation conflicts with frozen input"));
            }
        }

        let host = Arc::clone(&self.host);

        let mut current = host
            .snapshot
            .write()
            .unwrap_or_else(PoisonError::into_inner);

        if baseline
            && expected
                .as_ref()
                .is_none_or(|input| input.state == InputState::Stale)
            && let Some(input) = current.input_versions.get(&identity)
            && input.state == state
            && expected
                .as_ref()
                .is_none_or(|previous| input.revision > previous.revision)
        {
            let protected = except(&current, &BTreeSet::from([identity.clone()]));
            self.snapshot = self.snapshot.admit(&current, &protected)?;
            let input = input.clone();
            drop(current);
            self.consult_input(input);

            return Ok(());
        }

        let input = InputVersion {
            identity: identity.clone(),
            revision: expected
                .as_ref()
                .map_or(Some(1), |input| input.revision.checked_add(1))
                .ok_or_else(|| invalid("input revision overflow"))?,
            state,
        };

        let batch = ChangeBatch {
            changes: vec![ProjectChange::Input {
                expected: expected.clone(),
                current: input.clone(),
            }],
        };

        let proposed = self.snapshot.transition(batch.clone())?;

        let publish = baseline
            && (!self.frozen
                || match (&identity, &input.state) {
                    (InputIdentity::Path(path), InputState::Missing) => {
                        current.source_at(path).is_none() && read_bytes(path)?.is_none()
                    }

                    (
                        InputIdentity::Directory(path),
                        state @ (InputState::Missing
                        | InputState::NonDirectory
                        | InputState::Directory(_)),
                    ) => read_listing(path, None)?.state() == *state,

                    (InputIdentity::Path(path), InputState::Source(source)) => {
                        current.source_at(path) == Some(source)
                    }

                    (_, InputState::Source(source)) => {
                        current.source(source.version.identity) == Some(source)
                    }

                    _ => false,
                });

        if publish && current.input_versions.get(&identity) == expected.as_ref() {
            *current = current.transition(batch)?;
        } else if publish
            && current.input_versions.get(&identity) != Some(&input)
            && expected
                .as_ref()
                .is_some_and(|input| input.state == InputState::Stale)
        {
            return self.refresh_observation(identity, input.state, &mut current);
        }

        self.snapshot = proposed;
        drop(current);
        self.consult_input(input);

        Ok(())
    }

    fn refresh_observation(
        &mut self,
        identity: InputIdentity,
        state: InputState,
        current: &mut ProjectSnapshot,
    ) -> io::Result<()> {
        let expected = current
            .input_versions
            .get(&identity)
            .cloned()
            .ok_or_else(|| Reason::StaleQuery.error())?;

        if expected.state != InputState::Stale
            && !match &identity {
                InputIdentity::Directory(path) => read_listing(path, None)?.state() == state,
                _ => false,
            }
        {
            return Err(Reason::StaleQuery.error());
        }

        if expected.state == state {
            let protected = except(current, &BTreeSet::from([identity]));
            self.snapshot = self.snapshot.admit(current, &protected)?;
            self.consult_input(expected);

            return Ok(());
        }

        let input = InputVersion {
            identity: identity.clone(),
            revision: expected
                .revision
                .checked_add(1)
                .ok_or_else(|| invalid("input revision overflow"))?,
            state,
        };

        let proposed = current.transition(ChangeBatch {
            changes: vec![ProjectChange::Input {
                expected: Some(expected),
                current: input.clone(),
            }],
        })?;

        let protected = except(&proposed, &BTreeSet::from([identity]));
        let snapshot = self.snapshot.admit(&proposed, &protected)?;
        *current = proposed;
        self.snapshot = snapshot;
        self.consult_input(input);

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
                InputState::Missing | InputState::NonDirectory => true,

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

pub(crate) fn read_listing(path: &Path, options: Option<&Options>) -> io::Result<Listing> {
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

            Ok(Listing::Directory(paths))
        }

        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Listing::Missing),
        Err(error) if error.kind() == io::ErrorKind::NotADirectory => Ok(Listing::NonDirectory),

        Err(error) => Err(error),
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

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}
