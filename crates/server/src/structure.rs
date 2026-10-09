use std::{io, path::Path, time::Instant};

use instar_analysis::{Options, error::invalid};
use instar_core::project::Project;

use tower_lsp_server::{
    gen_lsp_types::{
        FoldingRange, FoldingRangeClientCapabilities, FoldingRangeKind, FoldingRangeParams,
        PositionEncodingKind, SelectionRange, SelectionRangeParams,
    },
    jsonrpc::Result,
};

use crate::{position, session::Session};

impl Session {
    pub(crate) async fn folds(&self, parameters: FoldingRangeParams) -> Result<Vec<FoldingRange>> {
        let capabilities = self
            .workspace
            .lock()
            .expect("workspace lock")
            .fold_capabilities
            .clone();

        self.query(
            &parameters.text_document.uri,
            &self.workers,
            move |path, encoding, project, options, started| {
                folds(path, encoding, &capabilities, project, options, started)
            },
        )
        .await
    }

    pub(crate) async fn selections(
        &self,
        parameters: SelectionRangeParams,
    ) -> Result<Vec<SelectionRange>> {
        self.query(
            &parameters.text_document.uri,
            &self.workers,
            move |path, encoding, project, options, started| {
                let source = project.source(path)?;
                let mut offsets = Vec::with_capacity(parameters.positions.len());

                for selected in parameters.positions {
                    options.check(started)?;

                    offsets
                        .push(position::offset(&source.text, selected, encoding).map_err(invalid)?);
                }

                let selections = project.selections(path, &offsets, &options.remaining(started))?;
                let mut result = Vec::with_capacity(selections.len());

                for ranges in selections {
                    let mut parent = None;

                    for range in ranges.into_iter().rev() {
                        options.check(started)?;

                        parent = Some(Box::new(SelectionRange {
                            range: position::range(&source.text, range, encoding)?,
                            parent,
                        }));
                    }

                    result.push(*parent.ok_or_else(|| invalid("selection hierarchy is empty"))?);
                }

                Ok(result)
            },
        )
        .await
    }
}

fn folds(
    path: &Path,
    encoding: &PositionEncodingKind,
    capabilities: &FoldingRangeClientCapabilities,
    project: &mut Project,
    options: &Options,
    started: Instant,
) -> io::Result<Vec<FoldingRange>> {
    let source = project.source(path)?;
    let line_only = capabilities.line_folding_only.unwrap_or(false);

    let comments = capabilities
        .folding_range_kind
        .as_ref()
        .and_then(|kinds| kinds.value_set.as_ref())
        .is_none_or(|kinds| kinds.contains(&FoldingRangeKind::Comment));

    let limit = capabilities
        .range_limit
        .map(usize::try_from)
        .transpose()
        .map_err(invalid)?
        .unwrap_or(usize::MAX);

    let mut result = Vec::new();

    if limit == 0 {
        return Ok(result);
    }

    for fold in project.folds(path, &options.remaining(started))? {
        options.check(started)?;
        let range = position::range(&source.text, fold.range, encoding)?;
        let mut end_line = range.end.line;

        if line_only {
            let suffix = source.text[fold.range[1]..]
                .split(['\r', '\n'])
                .next()
                .unwrap_or_default();

            if range.end.character == 0 || !suffix.trim().is_empty() {
                end_line = end_line.saturating_sub(1);
            }
        }

        if range.start.line >= end_line
            || result
                .last()
                .is_some_and(|previous: &FoldingRange| previous.start_line == range.start.line)
        {
            continue;
        }

        result.push(FoldingRange {
            start_line: range.start.line,
            start_character: None,
            end_line,
            end_character: (!line_only).then_some(range.end.character),
            kind: (fold.comment && comments).then_some(FoldingRangeKind::Comment),
            collapsed_text: None,
        });

        if result.len() == limit {
            break;
        }
    }

    Ok(result)
}
