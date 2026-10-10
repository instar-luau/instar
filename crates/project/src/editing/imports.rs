use std::{io, sync::Arc};

use instar_format::{FormatInput, FormatOutcome, FormatScope, settings::Order};
use instar_shared::{Completion, EditIntent, Options, Span};
use instar_syntax::{imports, parse, tree::NodeKind};

use super::request::{ImportRequest, ValidatedEdit};

use crate::{
    capture::Capture,
    configuration::worker_options,
    execution::{Runtime, Work, WorkOutput},
    graph::syntax_settings,
    queries::service::checked_source,
    report::EditReport,
};

impl Runtime {
    pub(crate) fn organize(
        &mut self,
        request: ImportRequest,
        report: &mut EditReport,
        options: &Options,
    ) -> io::Result<Option<ValidatedEdit>> {
        let source = checked_source(&request.snapshot, request.source)?.clone();
        let mut capture = Capture::new(Arc::clone(&self.host), request.snapshot);

        let target = self
            .editor_targets(
                Some(source.version.identity),
                false,
                &mut capture,
                None,
                options,
            )?
            .pop()
            .ok_or_else(|| io::Error::other("import source is absent"))?;

        let syntax = syntax_settings(&target.effective);
        let parsed = parse(&source, &syntax, options);
        report.snapshot = capture.snapshot.clone();
        report.inputs = capture.guards();
        report.diagnostics.extend(parsed.diagnostics.clone());
        report.completion = report.completion.clone().combine(parsed.completion.clone());

        if parsed.completion != Completion::Complete {
            return Err(parsed.completion.error());
        }

        if !parsed.diagnostics.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "import organization requires valid lossless syntax",
            ));
        }

        let mut settings = target.effective.settings.formatting.clone();
        settings.requires.order = Order::Alphabetical;
        let recognized = imports(&parsed.tree, options);
        let tree = parsed.tree.syntax();

        let NodeKind::Root { block, .. } = tree.node(tree.root).kind else {
            return Ok(None);
        };

        let NodeKind::Block { ref statements } = tree.node(block).kind else {
            return Ok(None);
        };

        let mut range: Option<Span> = None;

        for statement in tree.list(statements) {
            options.check()?;

            let NodeKind::Local {
                ref bindings,
                ref values,
                ..
            } = tree.node(statement.node).kind
            else {
                break;
            };

            let ([_], [value]) = (tree.list(bindings), tree.list(values)) else {
                break;
            };

            let value = tree.node(value.node).span;

            if !recognized
                .iter()
                .any(|import| import.call.start == value.start && import.call.end == value.end)
            {
                break;
            }

            let statement = tree.node(statement.node).span;

            if let Some(range) = &mut range {
                range.end = statement.end;
            } else {
                range = Some(Span {
                    start: statement.start,
                    end: statement.end,
                });
            }
        }

        let Some(range) = range else {
            return Ok(None);
        };

        let WorkOutput::Format(result) = self.workers.execute(
            Work::Format(FormatInput {
                source: source.clone(),
                settings,
                syntax,
                scope: FormatScope::Ranges(vec![range]),
            }),
            &worker_options(options),
        )?
        else {
            return Err(io::Error::other(
                "import formatting lane returned wrong outcome",
            ));
        };

        report.diagnostics.extend(result.diagnostics.clone());
        report.completion = report.completion.clone().combine(result.completion.clone());

        if result.completion != Completion::Complete {
            return Err(result.completion.error());
        }

        match result.outcome {
            FormatOutcome::Validated(format) if format.output() != source.text.as_ref() => {
                let mut validated =
                    self.validate_format(&format, &capture.snapshot, &capture.guards(), options)?;

                validated.proposal.intent = EditIntent::Imports;

                Ok(Some(validated))
            }

            FormatOutcome::Validated(_) => Ok(None),
            FormatOutcome::Unavailable { reason, .. } => Err(reason.error()),
        }
    }
}
