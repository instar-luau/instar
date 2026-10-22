use instar_core::analysis::FoldKind;
use serde_json::Value;

use tower_lsp_server::{
    gen_lsp_types::{FoldingRange, FoldingRangeKind, FoldingRangeParams},
    jsonrpc::Result,
};

use crate::project::Project;

pub(crate) fn folding_range(
    project: &mut Project,
    parameters: &FoldingRangeParams,
) -> Result<Option<Vec<FoldingRange>>> {
    let module = project.module(&parameters.text_document.uri)?;
    let document = project.snapshot(&module)?;
    let should_fold_lines = project.has_capability("/textDocument/foldingRange/lineFoldingOnly");

    let kinds = project
        .capabilities
        .pointer("/textDocument/foldingRange/foldingRangeKind/valueSet")
        .and_then(Value::as_array);

    let limit = project
        .capabilities
        .pointer("/textDocument/foldingRange/rangeLimit")
        .and_then(Value::as_u64)
        .and_then(|limit| usize::try_from(limit).ok())
        .unwrap_or(usize::MAX);

    let mut ranges = Vec::new();

    for fold in &document.analysis().folds {
        let range = document.range(fold.range.clone(), project.encoding)?;

        let end_line = if range.end.character == 0 {
            range.end.line.saturating_sub(1)
        } else {
            range.end.line
        };

        if range.start.line >= end_line {
            continue;
        }

        let (name, kind) = match fold.kind {
            FoldKind::Region => ("region", FoldingRangeKind::Region),
            FoldKind::Comment => ("comment", FoldingRangeKind::Comment),
            FoldKind::Imports => ("imports", FoldingRangeKind::Imports),
        };

        ranges.push(FoldingRange {
            start_line: range.start.line,
            start_character: (!should_fold_lines).then_some(range.start.character),
            end_line,
            end_character: (!should_fold_lines && range.end.character != 0)
                .then_some(range.end.character),
            kind: kinds
                .is_none_or(|kinds| kinds.iter().any(|kind| kind.as_str() == Some(name)))
                .then_some(kind),
            collapsed_text: None,
        });
    }

    ranges.dedup();
    ranges.truncate(limit);

    Ok(Some(ranges))
}
