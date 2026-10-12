use std::{
    collections::{BTreeMap, BTreeSet},
    fs, io,
    path::{Component, Path, PathBuf},
    sync::Arc,
};

use instar_luau::{ImportOutcome, ImportRequest};

use instar_shared::{
    Completion, EditIdentity, EditIntent, EditProposal, Options, SourceIdentity, TextEdit,
};

use instar_syntax::literal;

use super::{
    request::{EditGuarantee, FileChange, FileRequest, ValidatedEdit},
    validation::{overlay, relocated},
};

use crate::{
    capture::{Capture, Listing},
    change::{ChangeBatch, ProjectChange, SourceChange},
    configuration::{CheckOverrides, OperationOverrides},
    execution::Runtime,
    graph::DependencyGraph,
    inputs::{InputIdentity, InputState, InputVersion, ProjectSnapshot, SourceOrigin, absolute},
    report::EditReport,
    request::Invocation,
    resolution::module_path,
    selection::SelectedSource,
};

impl Runtime {
    pub(crate) fn files(
        &mut self,
        request: FileRequest,
        report: &mut EditReport,
        options: &Options,
    ) -> io::Result<ValidatedEdit> {
        let changes = normalize(request.changes, options)?;

        if changes.is_empty() {
            return Err(invalid("file preflight contains no operations"));
        }

        let mut capture = Capture::new(Arc::clone(&self.host), request.snapshot);

        let prepared = changes
            .iter()
            .try_for_each(|change| preflight(change, &mut capture, options));

        report.snapshot = capture.snapshot.clone();
        report.inputs = capture.guards();
        prepared?;

        let selected = self.file_targets(&mut capture, options)?;
        let mut before = self.file_before(&selected, &mut capture, options)?;

        capture.consult_inventory();
        report.snapshot = capture.snapshot.clone();
        report.inputs = capture.guards();

        report.diagnostics.extend(
            before
                .findings
                .iter()
                .map(|finding| finding.diagnostic.clone()),
        );

        report.completion = report.completion.clone().combine(before.completion.clone());

        if before.completion != Completion::Complete {
            return Err(before.completion.error());
        }

        validate_original(&before, &capture.snapshot, &changes, options)?;
        let created = self.file_creates(&changes);

        let (moved, intermediate) = loop {
            options.check()?;
            let moved = transform(&capture.snapshot, &changes, &created, options)?;
            let (intermediate, observed) = self.file_graph(&before, &moved, options)?;
            report.snapshots.push(observed.snapshot.clone());
            let previous = capture.snapshot.identity;

            capture.admit(
                &observed.snapshot,
                &super::validation::protected(&capture.snapshot, &moved),
            )?;

            capture.consult_inventory();

            let inputs = super::validation::consulted(
                &capture.snapshot,
                Some(&moved),
                capture.guards().into_iter().chain(observed.guards()),
            )?;

            for input in inputs {
                capture.inputs.insert(input.identity.clone(), input);
            }

            if previous == capture.snapshot.identity {
                break (moved, intermediate);
            }

            before = self.file_before(&selected, &mut capture, options)?;
        };

        let original = capture.snapshot.clone();
        report.snapshot = original.clone();
        report.inputs = capture.guards();
        let proposal = self.file_proposal(&before, &intermediate, &original, &moved, options)?;

        report.proposal = Some(proposal.clone());
        let text = overlay(&proposal, &original, options)?;
        let proposed = transform(&text, &changes, &created, options)?;
        let (after, after_capture) = self.file_graph(&before, &proposed, options)?;
        report.snapshots.push(after_capture.snapshot.clone());

        report.diagnostics.extend(
            after
                .findings
                .iter()
                .map(|finding| finding.diagnostic.clone()),
        );

        report.completion = report.completion.clone().combine(after.completion.clone());

        if after.completion != Completion::Complete {
            return Err(after.completion.error());
        }

        capture.admit(
            &after_capture.snapshot,
            &super::validation::protected(&original, &proposed),
        )?;

        let original = capture.snapshot.clone();
        report.snapshot = original.clone();
        report.inputs = capture.guards();

        validate_movement(&before, &after, &proposed, &proposal)?;

        let inputs = super::validation::consulted(
            &original,
            Some(&proposed),
            report.inputs.iter().cloned().chain(after_capture.guards()),
        )?;

        let guarantees = vec![
            EditGuarantee::Ranges,
            EditGuarantee::Syntax,
            EditGuarantee::Diagnostics,
            EditGuarantee::Resolution,
        ];

        let mut validated = self.validate_overlay(
            proposal,
            &original,
            after_capture.snapshot,
            &inputs,
            guarantees,
            options,
        )?;

        if !created.is_empty() {
            let text = overlay(&validated.proposal, &validated.original, options)?;
            validated.proposed = transform(&text, &changes, &BTreeMap::new(), options)?;
        }

        Ok(validated)
    }

    fn file_targets(
        &mut self,
        capture: &mut Capture,
        options: &Options,
    ) -> io::Result<Vec<SelectedSource>> {
        let mut selected = self.editor_targets(None, true, capture, None, options)?;

        let snapshot = capture.snapshot.clone();

        for source in snapshot.sources().cloned() {
            if !module(&capture.snapshot, source.version.identity) {
                continue;
            }

            let source = if let Some(SourceOrigin::Disk(path)) =
                capture.snapshot.origin(source.version.identity).cloned()
            {
                let Some(source) = capture.read(&path, options)? else {
                    continue;
                };

                source
            } else {
                source
            };

            if !selected
                .iter()
                .any(|target| target.identity == source.version.identity)
            {
                selected.extend(self.editor_targets(
                    Some(source.version.identity),
                    false,
                    capture,
                    None,
                    options,
                )?);
            }
        }

        Ok(selected)
    }

    fn file_before(
        &mut self,
        selected: &[SelectedSource],
        capture: &mut Capture,
        options: &Options,
    ) -> io::Result<DependencyGraph> {
        self.graph(
            selected,
            &Invocation::default(),
            &OperationOverrides::Check(CheckOverrides::default()),
            capture,
            true,
            options,
        )
    }

    fn file_creates(&self, changes: &[FileChange]) -> BTreeMap<PathBuf, SourceIdentity> {
        changes
            .iter()
            .filter_map(|change| {
                if let FileChange::Create(path) = change {
                    matches!(
                        path.extension().and_then(|extension| extension.to_str()),
                        Some("lua" | "luau")
                    )
                    .then(|| (path.clone(), self.host.source_identity()))
                } else {
                    None
                }
            })
            .collect()
    }

    fn file_proposal(
        &self,
        before: &DependencyGraph,
        intermediate: &DependencyGraph,
        original: &ProjectSnapshot,
        moved: &ProjectSnapshot,
        options: &Options,
    ) -> io::Result<EditProposal> {
        let mut edits = BTreeMap::new();

        for edge in &before.edges {
            options.check()?;

            if moved.source(edge.importer.source).is_none() {
                continue;
            }

            if intermediate.edges.iter().any(|updated| {
                updated.importer == edge.importer
                    && updated.target == edge.target
                    && updated.site.argument.span == edge.site.argument.span
            }) {
                continue;
            }

            let ImportRequest::Static(requested) = &edge.site.request else {
                return Err(unavailable(
                    "instance import movement requires an unchanged mapped loader identity",
                ));
            };

            if requested.starts_with("@game/") {
                return Err(unavailable(
                    "mapped string import movement requires an unchanged instance loader identity",
                ));
            }

            let source = original
                .source(edge.importer.source)
                .ok_or_else(|| invalid("file importer is absent"))?;

            edge.site.argument.validate(source)?;

            if literal::decode(
                source.text[edge.site.argument.span.start..edge.site.argument.span.end].as_bytes(),
            )
            .as_deref()
                != Some(requested.as_bytes())
            {
                return Err(unavailable(
                    "file reference is not one literal require argument",
                ));
            }

            let importer = moved
                .origin(edge.importer.source)
                .and_then(SourceOrigin::path)
                .ok_or_else(|| unavailable("file importer has no filesystem loader context"))?;

            let target = moved
                .origin(edge.target.source)
                .and_then(SourceOrigin::path)
                .ok_or_else(|| unavailable("file target has no filesystem loader context"))?;

            let parent = module_path(importer)
                .parent()
                .map(Path::to_path_buf)
                .ok_or_else(|| invalid("file importer has no resolution parent"))?;

            let target = if matches!(
                Path::new(requested)
                    .extension()
                    .and_then(|extension| extension.to_str()),
                Some("lua" | "luau")
            ) {
                target.to_path_buf()
            } else {
                module_path(target)
            };

            let destination = if let Some(alias) = requested.strip_prefix('@') {
                let (name, _) = alias.split_once(['/', '\\']).unwrap_or((alias, ""));

                let node = intermediate
                    .modules
                    .get(&edge.importer)
                    .ok_or_else(|| unavailable("file importer lost its semantic context"))?;

                let root = node
                    .effective
                    .settings
                    .resolution
                    .alias(name, &module_path(importer))?;

                if root == target {
                    format!("@{name}")
                } else {
                    let suffix = relative(&root, &target)?;

                    format!("@{name}/{}", suffix.strip_prefix("./").unwrap_or(&suffix))
                }
            } else {
                relative(&parent, &target)?
            };

            let replacement = literal::quote(&destination);
            let key = (edge.site.argument.source, edge.site.argument.span);

            if edits
                .get(&key)
                .is_some_and(|existing: &TextEdit| existing.replacement != replacement)
            {
                return Err(unavailable(
                    "one import spelling resolves to conflicting module contexts",
                ));
            }

            edits.insert(
                key,
                TextEdit {
                    location: edge.site.argument,
                    replacement,
                },
            );
        }

        let guards = before
            .modules
            .values()
            .map(|node| node.source.version)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();

        Ok(EditProposal {
            identity: EditIdentity::new(self.identity()),
            edits: edits.into_values().collect(),
            guards,
            intent: EditIntent::FileOperations,
        })
    }

    fn file_graph(
        &mut self,
        before: &DependencyGraph,
        snapshot: &ProjectSnapshot,
        options: &Options,
    ) -> io::Result<(DependencyGraph, Capture)> {
        let mut capture = Capture::new(Arc::clone(&self.host), snapshot.clone());
        let mut selected = Vec::new();

        let roots = before
            .roots
            .iter()
            .filter(|root| snapshot.source(root.source).is_some())
            .map(|root| root.source)
            .collect::<BTreeSet<_>>();

        let invocation = Invocation::default();
        let overrides = OperationOverrides::Check(CheckOverrides::default());

        for identity in roots {
            options.check()?;

            let effective =
                self.effective(identity, &invocation, &overrides, &mut capture, options)?;

            selected.push(SelectedSource {
                identity,
                effective,
            });
        }

        let graph = self.graph(
            &selected,
            &invocation,
            &overrides,
            &mut capture,
            true,
            options,
        )?;

        Ok((graph, capture))
    }
}

fn validate_original(
    before: &DependencyGraph,
    snapshot: &ProjectSnapshot,
    changes: &[FileChange],
    options: &Options,
) -> io::Result<()> {
    if before
        .modules
        .values()
        .flat_map(|node| &node.imports)
        .any(|import| matches!(import.site.request, ImportRequest::Nonconstant))
    {
        return Err(unavailable(
            "file preflight cannot establish targets of nonconstant require calls",
        ));
    }

    if changes
        .iter()
        .any(|change| !matches!(change, FileChange::Create(_)))
        && before
            .modules
            .values()
            .flat_map(|node| &node.imports)
            .any(|import| matches!(import.outcome, ImportOutcome::Unresolved))
    {
        return Err(unavailable(
            "file preflight requires all original import targets to resolve",
        ));
    }

    for source in snapshot.sources() {
        let Some(origin) = snapshot.origin(source.version.identity) else {
            return Err(invalid("file source origin is absent"));
        };

        if !matches!(
            origin,
            SourceOrigin::Disk(_) | SourceOrigin::Document { .. }
        ) {
            continue;
        }

        if let Some(path) = origin.path() {
            if affected(path, changes)
                && (!source_path(path)
                    || snapshot.is_declaration(source.version)
                    || snapshot
                        .input_versions
                        .contains_key(&InputIdentity::Configuration(source.version.identity))
                    || snapshot
                        .input_versions
                        .contains_key(&InputIdentity::Sourcemap(source.version.identity)))
            {
                return Err(unavailable(
                    "file operations affect a configuration, declaration, or sourcemap input",
                ));
            }

            if affected(path, changes) {
                ancestors(path, options)?;

                match fs::symlink_metadata(path) {
                    Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
                        return Err(unavailable("affected module is not a regular file"));
                    }

                    Ok(_) => {}

                    Err(error)
                        if error.kind() == io::ErrorKind::NotFound
                            && matches!(origin, SourceOrigin::Document { .. }) => {}

                    Err(error) => return Err(error),
                }
            }
        }
    }

    for edge in &before.edges {
        let path = snapshot
            .origin(edge.target.source)
            .and_then(SourceOrigin::path)
            .ok_or_else(|| unavailable("resolved target has no movable filesystem identity"))?;

        let importer = snapshot
            .origin(edge.importer.source)
            .and_then(SourceOrigin::path);

        if deleted(path, changes) && !importer.is_some_and(|path| deleted(path, changes)) {
            return Err(unavailable(
                "deletion would remove a referenced module; no behavior-preserving reference update exists",
            ));
        }
    }

    Ok(())
}

fn validate_movement(
    before: &DependencyGraph,
    after: &DependencyGraph,
    proposed: &ProjectSnapshot,
    proposal: &EditProposal,
) -> io::Result<()> {
    for node in before
        .modules
        .values()
        .filter(|node| proposed.source(node.identity.source).is_some())
    {
        let updated = after.modules.get(&node.identity).ok_or_else(|| {
            unavailable("file proposal changes a source's loader context identity")
        })?;

        let mut environment = updated.environment.input.clone();
        environment.identity = node.environment.input.identity;

        if environment != node.environment.input {
            return Err(unavailable(
                "file proposal changes the source's declaration or runtime environment",
            ));
        }
    }

    for edge in &before.edges {
        if proposed.source(edge.importer.source).is_none() {
            continue;
        }

        let (start, end) = relocated(
            edge.site.argument.source,
            edge.site.argument.span.start,
            edge.site.argument.span.end,
            &proposal.edits,
        )?;

        if !after.edges.iter().any(|updated| {
            updated.importer == edge.importer
                && updated.target == edge.target
                && updated.site.argument.span.start == start
                && updated.site.argument.span.end == end
        }) {
            return Err(unavailable(
                "file proposal cannot preserve a resolved loader and module identity",
            ));
        }
    }

    if after.edges.len()
        != before
            .edges
            .iter()
            .filter(|edge| proposed.source(edge.importer.source).is_some())
            .count()
    {
        return Err(unavailable(
            "file proposal changes the captured import graph",
        ));
    }

    let allowed = before
        .findings
        .iter()
        .map(|finding| (&finding.diagnostic.code, &finding.diagnostic.message))
        .collect::<BTreeSet<_>>();

    if after
        .findings
        .iter()
        .any(|finding| !allowed.contains(&(&finding.diagnostic.code, &finding.diagnostic.message)))
    {
        return Err(unavailable(
            "file proposal introduces a syntax, environment, or resolution diagnostic",
        ));
    }

    Ok(())
}

fn normalize(changes: Vec<FileChange>, options: &Options) -> io::Result<Vec<FileChange>> {
    let changes = changes
        .into_iter()
        .map(|change| match change {
            FileChange::Create(path) => absolute(&path).map(FileChange::Create),
            FileChange::Delete(path) => absolute(&path).map(FileChange::Delete),

            FileChange::Rename { old, new } => Ok(FileChange::Rename {
                old: absolute(&old)?,
                new: absolute(&new)?,
            }),
        })
        .collect::<io::Result<Vec<_>>>()?;

    let mut paths: Vec<&Path> = Vec::new();

    for change in &changes {
        options.check()?;

        let current = match change {
            FileChange::Create(path) | FileChange::Delete(path) => vec![path.as_path()],
            FileChange::Rename { old, new } => vec![old.as_path(), new.as_path()],
        };

        for path in current {
            if path.parent().is_none()
                || paths
                    .iter()
                    .any(|existing| path.starts_with(existing) || existing.starts_with(path))
            {
                return Err(invalid(
                    "file operations overlap or include a filesystem root",
                ));
            }

            paths.push(path);
        }
    }

    Ok(changes)
}

fn preflight(change: &FileChange, capture: &mut Capture, options: &Options) -> io::Result<()> {
    let (old, new) = match change {
        FileChange::Create(path) => (None, Some(path)),
        FileChange::Delete(path) => (Some(path), None),
        FileChange::Rename { old, new } => (Some(old), Some(new)),
    };

    for path in old.into_iter().chain(new) {
        options.check()?;
        ancestors(path, options)?;

        let parent = path
            .parent()
            .ok_or_else(|| invalid("file operation has no parent"))?;

        match capture.listing(parent, options)? {
            Listing::Directory(_) => {}

            Listing::NonDirectory => {
                return Err(invalid("file operation parent is not a directory"));
            }

            Listing::Missing => {
                if Some(path) == old {
                    return Err(invalid("file operation parent directory is absent"));
                }

                let mut present = false;

                for ancestor in parent.ancestors().skip(1) {
                    match capture.listing(ancestor, options)? {
                        Listing::Directory(_) => {
                            present = true;
                            break;
                        }

                        Listing::NonDirectory => {
                            return Err(invalid("file destination ancestor is not a directory"));
                        }

                        Listing::Missing => {}
                    }
                }

                if !present {
                    return Err(invalid(
                        "file destination has no captured directory ancestor",
                    ));
                }
            }
        }
    }

    if let Some(path) = old {
        let metadata = fs::symlink_metadata(path)?;

        if metadata.file_type().is_symlink() || (!metadata.is_file() && !metadata.is_dir()) {
            return Err(unavailable(
                "file preflight requires regular files or directories",
            ));
        }

        if metadata.is_file() && source_path(path) {
            capture.source(path, options)?;
        } else if metadata.is_dir() {
            capture.listing(path, options)?;
        } else {
            return Err(unavailable(
                "file preflight cannot analyze a non-module file",
            ));
        }
    }

    if let Some(path) = new {
        match fs::symlink_metadata(path) {
            Ok(_) => return Err(invalid("file destination already exists")),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }

        if capture.snapshot.source_at(path).is_some() {
            return Err(invalid(
                "file destination already has synchronized contents",
            ));
        }

        if path.file_name().is_some_and(|name| {
            name == "instar.luau" || name == ".luaurc" || name == "sourcemap.json"
        }) {
            return Err(unavailable(
                "file creation or movement would change configuration or the mapped environment",
            ));
        }

        capture.observation(InputIdentity::Path(path.clone()), InputState::Missing)?;
        capture.listing(path, options)?;
    }

    Ok(())
}

fn transform(
    snapshot: &ProjectSnapshot,
    changes: &[FileChange],
    created: &BTreeMap<PathBuf, SourceIdentity>,
    options: &Options,
) -> io::Result<ProjectSnapshot> {
    let mut batch = ChangeBatch::default();

    for source in snapshot.sources() {
        options.check()?;

        let Some(origin) = snapshot.origin(source.version.identity) else {
            return Err(invalid("file source origin is absent"));
        };

        if !matches!(
            origin,
            SourceOrigin::Disk(_) | SourceOrigin::Document { .. }
        ) {
            continue;
        }

        let Some(path) = origin.path() else {
            continue;
        };

        if deleted(path, changes) {
            batch
                .changes
                .push(ProjectChange::Source(SourceChange::Remove {
                    expected: source.version,
                }));
        } else if let Some(destination) = destination(path, changes) {
            let origin = match origin {
                SourceOrigin::Disk(_) => SourceOrigin::Disk(destination),

                SourceOrigin::Document { address, .. } => SourceOrigin::Document {
                    address: address.clone(),
                    context: Some(destination),
                },

                _ => {
                    return Err(unavailable(
                        "file movement includes a non-filesystem source",
                    ));
                }
            };

            batch
                .changes
                .push(ProjectChange::Source(SourceChange::Move {
                    expected: source.version,
                    origin,
                }));
        }
    }

    for (path, identity) in created {
        batch
            .changes
            .push(ProjectChange::Source(SourceChange::Open {
                identity: *identity,
                origin: SourceOrigin::Disk(path.clone()),
                text: "".into(),
            }));
    }

    let proposed = snapshot.transition(batch)?;

    transform_directories(snapshot, proposed, changes, options)
}

fn transform_directories(
    snapshot: &ProjectSnapshot,
    mut proposed: ProjectSnapshot,
    changes: &[FileChange],
    options: &Options,
) -> io::Result<ProjectSnapshot> {
    let mut directories = BTreeMap::new();

    for input in snapshot.input_versions.values() {
        options.check()?;

        let InputIdentity::Directory(path) = &input.identity else {
            continue;
        };

        if input.state == InputState::NonDirectory {
            if affected(path, changes) {
                directories.insert(path.clone(), InputState::Missing);
            }

            if !deleted(path, changes) {
                let path = destination(path, changes).unwrap_or_else(|| path.clone());
                directories.insert(path, InputState::NonDirectory);
            }

            continue;
        }

        let InputState::Directory(entries) = &input.state else {
            continue;
        };

        if deleted(path, changes) || destination(path, changes).is_some() {
            directories.insert(path.clone(), InputState::Missing);
        }

        if !deleted(path, changes) {
            let directory = destination(path, changes).unwrap_or_else(|| path.clone());

            let mut entries = entries
                .iter()
                .filter(|entry| !deleted(entry, changes))
                .map(|entry| destination(entry, changes).unwrap_or_else(|| entry.clone()))
                .collect::<Vec<_>>();

            entries.retain(|entry| entry.parent() == Some(directory.as_path()));

            for change in changes {
                let new = match change {
                    FileChange::Create(path) | FileChange::Rename { new: path, .. } => Some(path),
                    FileChange::Delete(_) => None,
                };

                if let Some(new) = new
                    && new.parent() == Some(directory.as_path())
                {
                    entries.push(new.clone());
                }
            }

            entries.sort();
            entries.dedup();
            directories.insert(directory, InputState::Directory(entries));
        }
    }

    for change in changes {
        let new = match change {
            FileChange::Create(path) | FileChange::Rename { new: path, .. } => path,
            FileChange::Delete(_) => continue,
        };

        if matches!(change, FileChange::Create(_))
            || matches!(change, FileChange::Rename { old, .. }
                if snapshot.source_at(old).is_some())
        {
            directories.insert(new.clone(), InputState::NonDirectory);
        }

        let mut entry = new.clone();

        while let Some(parent) = entry.parent() {
            options.check()?;
            let identity = InputIdentity::Directory(parent.to_path_buf());
            let captured = snapshot.input_versions.get(&identity);

            let state = directories.entry(parent.to_path_buf()).or_insert_with(|| {
                captured.map_or(InputState::Directory(Vec::new()), |input| {
                    if input.state == InputState::NonDirectory {
                        InputState::NonDirectory
                    } else {
                        InputState::Directory(Vec::new())
                    }
                })
            });

            let InputState::Directory(entries) = state else {
                return Err(unavailable("file destination overlaps a removed directory"));
            };

            entries.push(entry.clone());
            entries.sort();
            entries.dedup();

            if captured.is_some_and(|input| matches!(input.state, InputState::Directory(_)))
                && !deleted(parent, changes)
                && destination(parent, changes).is_none()
            {
                break;
            }

            entry = parent.to_path_buf();
        }
    }

    for (path, state) in directories {
        let identity = InputIdentity::Directory(path);
        let expected = proposed.input_versions.get(&identity).cloned();

        if expected.as_ref().is_some_and(|input| input.state == state) {
            continue;
        }

        let revision = expected
            .as_ref()
            .map_or(Some(1), |input| input.revision.checked_add(1))
            .ok_or_else(|| invalid("file directory revision overflow"))?;

        proposed = proposed.transition(ChangeBatch {
            changes: vec![ProjectChange::Input {
                expected,
                current: InputVersion {
                    identity,
                    revision,
                    state,
                },
            }],
        })?;
    }

    Ok(proposed)
}

fn destination(path: &Path, changes: &[FileChange]) -> Option<PathBuf> {
    changes.iter().find_map(|change| {
        if let FileChange::Rename { old, new } = change {
            path.strip_prefix(old).ok().map(|suffix| new.join(suffix))
        } else {
            None
        }
    })
}

fn deleted(path: &Path, changes: &[FileChange]) -> bool {
    changes
        .iter()
        .any(|change| matches!(change, FileChange::Delete(deleted) if path.starts_with(deleted)))
}

fn affected(path: &Path, changes: &[FileChange]) -> bool {
    deleted(path, changes) || destination(path, changes).is_some()
}

fn source_path(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| {
            name != "instar.luau" && !name.ends_with(".d.lua") && !name.ends_with(".d.luau")
        })
        && matches!(
            path.extension().and_then(|extension| extension.to_str()),
            Some("lua" | "luau")
        )
}

fn module(snapshot: &ProjectSnapshot, identity: SourceIdentity) -> bool {
    snapshot
        .origin(identity)
        .filter(|origin| {
            matches!(
                origin,
                SourceOrigin::Disk(_) | SourceOrigin::Document { .. }
            )
        })
        .and_then(SourceOrigin::path)
        .is_some_and(|path| {
            source_path(path)
                && snapshot
                    .source_at(path)
                    .is_some_and(|source| source.version.identity == identity)
        })
}

fn relative(parent: &Path, target: &Path) -> io::Result<String> {
    let parent = parent.components().collect::<Vec<_>>();
    let target = target.components().collect::<Vec<_>>();

    let common = parent
        .iter()
        .zip(&target)
        .take_while(|(left, right)| left == right)
        .count();

    if common == 0
        || parent[common..]
            .iter()
            .any(|component| !matches!(component, Component::Normal(_)))
        || target[common..]
            .iter()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(unavailable("file reference crosses filesystem roots"));
    }

    let mut relative = PathBuf::new();

    for _ in &parent[common..] {
        relative.push("..");
    }

    for component in &target[common..] {
        relative.push(component.as_os_str());
    }

    let text = relative
        .to_str()
        .ok_or_else(|| unavailable("file reference destination is not UTF-8"))?
        .replace('\\', "/");

    if text.is_empty() {
        return Err(invalid("file reference has an empty destination"));
    }

    Ok(if text.starts_with("../") {
        text
    } else {
        format!("./{text}")
    })
}

fn ancestors(path: &Path, options: &Options) -> io::Result<()> {
    for ancestor in path.ancestors().skip(1) {
        options.check()?;

        match fs::symlink_metadata(ancestor) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(unavailable(
                    "file preflight does not follow symbolic-link ancestors",
                ));
            }

            Ok(metadata) if !metadata.is_dir() => {
                return Err(unavailable("file operation ancestor is not a directory"));
            }

            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }

    Ok(())
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}

fn unavailable(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::Unsupported, message)
}
