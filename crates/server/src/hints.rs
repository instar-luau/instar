use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    io,
    path::{Path, PathBuf},
    time::Instant,
};

use instar_analysis::{Options, error::invalid};
use instar_core::{project::Project, snapshot::Snapshot};
use serde_json::{Value, json};

use tower_lsp_server::{
    gen_lsp_types::{InlayHint, InlayHintKind, InlayHintParams, PositionEncodingKind, Range, Uri},
    jsonrpc::{Error, Result},
};

use crate::{position, session::Session};

const RETAINED_LISTS: usize = 32;

#[derive(Default)]
pub(crate) struct Hints {
    pub(crate) deferred_tooltips: bool,
    generation: u64,
    records: VecDeque<Record>,
}

impl Hints {
    pub(crate) fn forget(&mut self, uri: &Uri) {
        self.records.retain(|record| &record.uri != uri);
    }
}

struct Record {
    generation: u64,
    snapshot: Snapshot,
    inputs: BTreeSet<PathBuf>,
    uri: Uri,
    candidates: Vec<Candidate>,
}

#[derive(Clone)]
struct Candidate {
    hint: InlayHint,
    symbol: Option<String>,
    description: String,
}

impl Session {
    pub(crate) async fn hints(&self, parameters: InlayHintParams) -> Result<Vec<InlayHint>> {
        let uri = parameters.text_document.uri;

        let (snapshot, deferred) = {
            let workspace = self.workspace.lock().expect("workspace lock");

            (
                workspace.snapshot.clone(),
                workspace.hints.deferred_tooltips,
            )
        };

        let (mut candidates, inputs) = self
            .query(
                &uri,
                &self.queries,
                move |path, encoding, project, options, started| {
                    let mut candidates =
                        query(path, parameters.range, encoding, project, options, started)?;

                    if !deferred {
                        for candidate in &mut candidates {
                            resolve(candidate, path, project, options, started)?;
                        }
                    }

                    Ok((candidates, project.watch_inputs()))
                },
            )
            .await?;

        let mut workspace = self.workspace.lock().expect("workspace lock");

        if !workspace.current(&snapshot, &inputs) {
            return Err(Error::content_modified());
        }

        if !deferred {
            return Ok(candidates
                .into_iter()
                .map(|candidate| candidate.hint)
                .collect());
        }

        let hints = &mut workspace.hints;

        let generation = hints
            .generation
            .checked_add(1)
            .ok_or_else(Error::internal_error)?;

        hints.generation = generation;

        for (index, candidate) in candidates.iter_mut().enumerate() {
            candidate.hint.data = Some(json!({"hints": generation, "item": index}));
        }

        let result = candidates
            .iter()
            .map(|candidate| candidate.hint.clone())
            .collect();

        hints.records.push_back(Record {
            generation,
            snapshot,
            inputs,
            uri,
            candidates,
        });

        while hints.records.len() > RETAINED_LISTS {
            hints.records.pop_front();
        }

        Ok(result)
    }

    pub(crate) async fn resolve_hint(&self, hint: InlayHint) -> Result<InlayHint> {
        let data = hint
            .data
            .as_ref()
            .ok_or_else(|| Error::invalid_params("missing inlay hint identity"))?;

        let generation = data
            .get("hints")
            .and_then(Value::as_u64)
            .ok_or_else(|| Error::invalid_params("invalid inlay hint identity"))?;

        let index = data
            .get("item")
            .and_then(Value::as_u64)
            .and_then(|index| usize::try_from(index).ok())
            .ok_or_else(|| Error::invalid_params("invalid inlay hint item"))?;

        let (snapshot, inputs, uri, mut candidate) = {
            let workspace = self.workspace.lock().expect("workspace lock");

            let record = workspace
                .hints
                .records
                .iter()
                .find(|record| record.generation == generation)
                .ok_or_else(Error::content_modified)?;

            if !workspace.current(&record.snapshot, &record.inputs) {
                return Err(Error::content_modified());
            }

            let candidate = record
                .candidates
                .get(index)
                .ok_or_else(|| Error::invalid_params("invalid inlay hint item"))?;

            (
                record.snapshot.clone(),
                record.inputs.clone(),
                record.uri.clone(),
                candidate.clone(),
            )
        };

        candidate = self
            .query(
                &uri,
                &self.queries,
                move |path, _, project, options, started| {
                    resolve(&mut candidate, path, project, options, started)?;

                    Ok(candidate)
                },
            )
            .await?;

        if !self
            .workspace
            .lock()
            .expect("workspace lock")
            .current(&snapshot, &inputs)
        {
            return Err(Error::content_modified());
        }

        Ok(candidate.hint)
    }
}

fn query(
    path: &Path,
    range: Range,
    encoding: &PositionEncodingKind,
    project: &mut Project,
    options: &Options,
    started: Instant,
) -> io::Result<Vec<Candidate>> {
    let source = project.source(path)?;

    let range = [
        position::offset(&source.text, range.start, encoding).map_err(invalid)?,
        position::offset(&source.text, range.end, encoding).map_err(invalid)?,
    ];

    if range[0] > range[1] {
        return Err(invalid("reversed inlay hint range"));
    }

    let entries = project.entries(&[path.to_path_buf()], &options.remaining(started))?;
    let mut common: Option<BTreeMap<_, _>> = None;

    for entry in entries {
        let hints = project.hints(&entry, range, &options.remaining(started))?;

        let current: BTreeMap<_, _> = hints
            .into_iter()
            .map(|hint| (hint.location.range, (hint.text, hint.documentation)))
            .collect();

        match &mut common {
            Some(common) => common.retain(|range, value| current.get(range) == Some(value)),
            None => common = Some(current),
        }
    }

    common
        .unwrap_or_default()
        .into_iter()
        .map(|(range, (text, symbol))| {
            options.check(started)?;

            let name = source
                .text
                .get(range[0]..range[1])
                .ok_or_else(|| invalid("inlay hint range is unavailable"))?;

            Ok(Candidate {
                hint: InlayHint {
                    position: position::range(&source.text, range, encoding)?.end,
                    label: format!(": {}", text.replace(['\r', '\n'], " ")).into(),
                    kind: Some(InlayHintKind::Type),
                    text_edits: None,
                    tooltip: None,
                    padding_left: None,
                    padding_right: None,
                    data: None,
                },
                symbol,
                description: format!("{name}: {text}"),
            })
        })
        .collect()
}

fn resolve(
    candidate: &mut Candidate,
    path: &Path,
    project: &mut Project,
    options: &Options,
    started: Instant,
) -> io::Result<()> {
    options.check(started)?;
    let mut description = candidate.description.clone();

    if let Some(symbol) = &candidate.symbol
        && let Some(documentation) =
            project.documentation_for(path, symbol, &options.remaining(started))?
    {
        for text in [
            documentation.documentation,
            documentation.code_sample,
            documentation.learn_more_link,
        ]
        .into_iter()
        .flatten()
        {
            description.push_str("\n\n");
            description.push_str(&text);
        }
    }

    candidate.hint.tooltip = Some(description.into());

    options.check(started)
}
