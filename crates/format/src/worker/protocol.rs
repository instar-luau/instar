use std::{
    io,
    sync::Arc,
    time::{Duration, Instant},
};

use instar_shared::{
    Completion, Diagnostic, DiagnosticCategory, EventPayload, FindingIdentity, Location, Options,
    Progress, Reason, Severity, SourceSnapshot, Span,
    error::invalid,
    process::{Emitter, serve},
};

use instar_syntax::{
    DirectiveKind, PreservationResult, SelectionResult, Tree, directives, expand, parse,
    preserve_mapped, selections, tree::NodeKind,
};

use serde::{Deserialize, Serialize};

use crate::{
    FormatInput, FormatScope, FormatSelection, SelectionOutcome,
    layout::{Builder, emit},
    settings::Order,
};

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Request {
    pub(crate) identity: u64,
    pub(crate) input: FormatInput,
    pub(crate) remaining_budget: Option<Duration>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Response {
    pub(crate) identity: u64,
    pub(crate) generation: u64,
    pub(crate) input: FormatInput,
    pub(crate) outcome: WireOutcome,
    pub(crate) diagnostics: Vec<Diagnostic>,
    pub(crate) completion: Completion,
}

#[derive(Deserialize, Serialize)]
pub(crate) enum WireOutcome {
    Validated {
        output: Arc<str>,
        selections: Vec<FormatSelection>,
        preservation: PreservationResult,
    },

    Unavailable {
        reason: Reason,
        selections: Vec<FormatSelection>,
        preservation: Option<PreservationResult>,
    },
}

pub(crate) struct Prepared {
    pub(crate) selections: Vec<FormatSelection>,
    pub(crate) regions: Vec<Span>,
    pub(crate) whole: bool,
    pub(crate) reason: Option<Reason>,
    pub(crate) completion: Completion,
}

struct Rendered {
    bytes: Vec<u8>,
    ranges: Vec<Span>,
}

pub(crate) fn execute(request: &Request, emitter: &Emitter<Response>) -> io::Result<Response> {
    let deadline = request
        .remaining_budget
        .map(|budget| {
            Instant::now().checked_add(budget).ok_or_else(|| {
                invalid("format request time budget exceeds the monotonic clock range")
            })
        })
        .transpose()?;

    let options = Options::new(deadline);
    request.input.validate()?;
    progress(emitter, &request.input, "parsing")?;
    let parsed = parse(&request.input.source, &request.input.syntax, &options);
    let directive = directives(&parsed.tree, &options);
    let mut diagnostics = parsed.diagnostics.clone();
    diagnostics.extend(directive.diagnostics.clone());
    let prepared = prepare(&parsed.tree, &request.input, &options)?;

    let completion = parsed
        .completion
        .clone()
        .combine(directive.completion)
        .combine(prepared.completion.clone());

    if completion != Completion::Complete {
        return Ok(unavailable(
            request,
            emitter,
            prepared.reason.unwrap_or(Reason::FailedPrerequisite),
            prepared.selections,
            None,
            diagnostics,
            completion,
        ));
    }

    if let Some(reason) = prepared.reason {
        return Ok(unavailable(
            request,
            emitter,
            reason,
            prepared.selections,
            None,
            diagnostics,
            Completion::Complete,
        ));
    }

    if !directive.diagnostics.is_empty() {
        return Ok(unavailable(
            request,
            emitter,
            Reason::FailedPrerequisite,
            prepared.selections,
            None,
            diagnostics,
            Completion::Complete,
        ));
    }

    format(
        request,
        emitter,
        &parsed.tree,
        prepared,
        diagnostics,
        &options,
    )
}

fn render(
    tree: &Tree<'_>,
    input: &FormatInput,
    prepared: &Prepared,
    options: &Options,
) -> Result<Rendered, (Reason, Completion)> {
    let mut output = Vec::new();
    let mut output_ranges = Vec::new();
    let mut cursor = 0;

    for span in &prepared.regions {
        if let Some(completion) = options.interrupted() {
            return Err((Reason::FailedPrerequisite, completion));
        }

        output.extend_from_slice(&input.source.text.as_bytes()[cursor..span.start]);

        let selection = if prepared.whole {
            None
        } else {
            Some(expand(tree, *span, options))
        };

        if selection.as_ref().is_some_and(|selection| {
            selection.unavailable.is_some() || selection.expanded.as_slice() != [*span]
        }) {
            return Err((
                Reason::UnsupportedSyntax,
                options.interrupted().unwrap_or(Completion::Complete),
            ));
        }

        let plan = Builder::build(tree, selection.as_ref(), &input.settings, options).map_err(
            |reason| {
                (
                    reason,
                    options
                        .interrupted()
                        .unwrap_or(Completion::Incomplete(vec![reason])),
                )
            },
        )?;

        let mut rendered = emit(tree, &input.settings, &plan, options)
            .map_err(|completion| (Reason::FailedPrerequisite, completion))?;

        let original = &input.source.text.as_bytes()[span.start..span.end];

        if !matches!(input.scope, FormatScope::Whole)
            && !original.ends_with(b"\n")
            && !original.ends_with(b"\r")
            || matches!(input.scope, FormatScope::Whole)
                && !input.settings.final_newline
                && span.end == input.source.text.len()
        {
            while rendered
                .last()
                .is_some_and(|byte| matches!(byte, b'\n' | b'\r'))
            {
                rendered.pop();
            }
        }

        let start = output.len();
        output.extend(rendered);

        output_ranges.push(Span {
            start,
            end: output.len(),
        });

        cursor = span.end;
    }

    output.extend_from_slice(&input.source.text.as_bytes()[cursor..]);

    Ok(Rendered {
        bytes: output,
        ranges: output_ranges,
    })
}

fn format(
    request: &Request,
    emitter: &Emitter<Response>,
    tree: &Tree<'_>,
    prepared: Prepared,
    mut diagnostics: Vec<Diagnostic>,
    options: &Options,
) -> io::Result<Response> {
    let (_, generation) = emitter.identities();
    progress(emitter, &request.input, "formatting")?;

    let rendered = match render(tree, &request.input, &prepared, options) {
        Ok(rendered) => rendered,

        Err((reason, completion)) => {
            return Ok(unavailable(
                request,
                emitter,
                reason,
                prepared.selections,
                None,
                diagnostics,
                completion,
            ));
        }
    };

    let output = String::from_utf8(rendered.bytes).map_err(invalid)?;
    let proposed = SourceSnapshot::new(request.input.source.version, output.clone())?;
    let reparsed = parse(&proposed, &request.input.syntax, options);
    progress(emitter, &request.input, "preservation")?;

    let preservation = preserve_mapped(
        tree,
        &reparsed.tree,
        &prepared.regions,
        &rendered.ranges,
        request.input.settings.requires.order != Order::Preserve,
        options,
    );

    if !preservation.preserved() {
        let completion = options
            .interrupted()
            .unwrap_or_else(|| preservation.completion.clone());

        if completion == Completion::Complete {
            diagnostics.push(failure(
                &request.input,
                "format.preservation",
                "formatting could not establish every requested syntax preservation guarantee",
            ));
        }

        return Ok(unavailable(
            request,
            emitter,
            Reason::UnsupportedSyntax,
            prepared.selections,
            Some(preservation),
            diagnostics,
            completion,
        ));
    }

    let selections = selection_outcomes(
        &prepared.selections,
        &prepared.regions,
        &rendered.ranges,
        &request.input.source.text,
        &output,
    );

    emitter.emit(EventPayload::Progress(Progress {
        phase: "preservation".to_owned(),
        source: Some(request.input.source.version),
        completed: 1,
        total: Some(1),
    }))?;

    Ok(Response {
        identity: request.identity,
        generation,
        input: request.input.clone(),
        outcome: WireOutcome::Validated {
            output: Arc::from(output),
            selections,
            preservation,
        },
        diagnostics,
        completion: Completion::Complete,
    })
}

fn unavailable(
    request: &Request,
    emitter: &Emitter<Response>,
    reason: Reason,
    mut selections: Vec<FormatSelection>,
    preservation: Option<PreservationResult>,
    diagnostics: Vec<Diagnostic>,
    completion: Completion,
) -> Response {
    for selection in &mut selections {
        selection.outcome = SelectionOutcome::Unavailable(reason);
    }

    let (_, generation) = emitter.identities();

    Response {
        identity: request.identity,
        generation,
        input: request.input.clone(),
        outcome: WireOutcome::Unavailable {
            reason,
            selections,
            preservation,
        },
        diagnostics,
        completion,
    }
}

pub(crate) fn prepare(
    tree: &Tree<'_>,
    input: &FormatInput,
    options: &Options,
) -> io::Result<Prepared> {
    let requested = input.requested()?;
    let whole = matches!(input.scope, FormatScope::Whole);
    let policies = directives(tree, options);

    let protected = policies
        .directives
        .iter()
        .filter(|directive| matches!(directive.kind, DirectiveKind::Format))
        .flat_map(|directive| [directive.scope, directive.location])
        .collect::<Vec<_>>();

    let prefix = usize::from(input.source.text.starts_with('\u{feff}')) * 3;

    let selection = if whole {
        None
    } else {
        Some(selections(tree, &requested, options))
    };

    let mut formats = format_selections(
        &requested,
        selection.as_ref(),
        Span {
            start: prefix,
            end: input.source.text.len(),
        },
    );

    let mut reason = if whole && !tree.syntax().diagnostics.is_empty() {
        Some(Reason::UnsupportedSyntax)
    } else {
        formats.iter().find_map(|selection| {
            if let SelectionOutcome::Unavailable(reason) = selection.outcome {
                Some(reason)
            } else {
                None
            }
        })
    };

    if reason.is_none()
        && let Completion::Incomplete(reasons) = tree.coverage()
    {
        reason = Some(
            reasons
                .first()
                .copied()
                .unwrap_or(Reason::FailedPrerequisite),
        );
    }

    let mut regions = selection.as_ref().map_or_else(
        || {
            vec![Span {
                start: prefix,
                end: input.source.text.len(),
            }]
        },
        |selected| selected.expanded.clone(),
    );

    if !protected.is_empty() && reason.is_none() {
        for format in &mut formats {
            let mut available = Vec::new();

            for bound in &format.expanded {
                available.extend(unsuppressed(tree, *bound, &protected, options));
            }

            merge(&mut available);
            format.expanded = available;
        }

        regions = formats
            .iter()
            .flat_map(|selection| selection.expanded.iter().copied())
            .collect();

        merge(&mut regions);
    }

    if let Some(unavailable) = reason {
        for format in &mut formats {
            format.outcome = SelectionOutcome::Unavailable(unavailable);
        }
    }

    let mut completion = tree.coverage().clone().combine(policies.completion);

    if let Some(selected) = selection {
        completion = completion.combine(selected.completion);
    }

    if let Some(interruption) = options.interrupted() {
        completion = interruption;
        reason = Some(Reason::FailedPrerequisite);
    }

    Ok(Prepared {
        selections: formats,
        regions,
        whole: whole && protected.is_empty(),
        reason,
        completion,
    })
}

fn format_selections(
    requested: &[Span],
    selection: Option<&SelectionResult>,
    whole: Span,
) -> Vec<FormatSelection> {
    selection.map_or_else(
        || {
            requested
                .iter()
                .map(|requested| FormatSelection {
                    requested: *requested,
                    expanded: vec![whole],
                    outcome: SelectionOutcome::Unchanged,
                })
                .collect()
        },
        |selected| {
            selected
                .selections
                .iter()
                .map(|selection| FormatSelection {
                    requested: selection.requested,
                    expanded: selection.expanded.clone(),
                    outcome: selection
                        .unavailable
                        .map_or(SelectionOutcome::Unchanged, SelectionOutcome::Unavailable),
                })
                .collect()
        },
    )
}

pub(crate) fn selection_outcomes(
    selections: &[FormatSelection],
    regions: &[Span],
    outputs: &[Span],
    original: &str,
    output: &str,
) -> Vec<FormatSelection> {
    selections
        .iter()
        .map(|selection| {
            let mut selection = selection.clone();

            let changed = regions.iter().zip(outputs).any(|(region, output_range)| {
                selection
                    .expanded
                    .iter()
                    .any(|span| overlap(*span, *region))
                    && original.get(region.start..region.end)
                        != output.get(output_range.start..output_range.end)
            });

            selection.outcome = if changed {
                SelectionOutcome::Formatted
            } else {
                SelectionOutcome::Unchanged
            };

            selection
        })
        .collect()
}

fn unsuppressed(tree: &Tree<'_>, bound: Span, protected: &[Span], options: &Options) -> Vec<Span> {
    let syntax = tree.syntax();
    let mut pending = vec![syntax.root];
    let mut result = Vec::new();

    while let Some(index) = pending.pop() {
        if options.interrupted().is_some() {
            break;
        }

        if let NodeKind::Block { statements } = &syntax.node(index).kind {
            for entry in syntax.list(statements) {
                let node = syntax.node(entry.node);

                let span = Span {
                    start: node.span.start,
                    end: node.span.end,
                };

                if !overlap(bound, span) {
                    continue;
                }

                if protected.iter().any(|protected| overlap(*protected, span)) {
                    pending.extend(syntax.children(entry.node));
                    continue;
                }

                let selected = expand(tree, span, options);

                if selected.unavailable.is_none()
                    && selected.expanded.iter().all(|span| {
                        bound.start <= span.start
                            && span.end <= bound.end
                            && !protected.iter().any(|protected| overlap(*protected, *span))
                    })
                {
                    result.extend(selected.expanded);
                }
            }
        } else {
            pending.extend(syntax.children(index));
        }
    }

    result
}

fn merge(regions: &mut Vec<Span>) {
    regions.sort_by_key(|span| (span.start, span.end));
    let mut merged: Vec<Span> = Vec::new();

    for span in regions.iter().copied() {
        if let Some(previous) = merged.last_mut()
            && span.start <= previous.end
        {
            previous.end = previous.end.max(span.end);
        } else {
            merged.push(span);
        }
    }

    *regions = merged;
}

fn overlap(left: Span, right: Span) -> bool {
    left.start < right.end && right.start < left.end
}

fn progress(emitter: &Emitter<Response>, input: &FormatInput, phase: &str) -> io::Result<()> {
    emitter.emit(EventPayload::Progress(Progress {
        phase: phase.to_owned(),
        source: Some(input.source.version),
        completed: 0,
        total: Some(1),
    }))
}

fn failure(input: &FormatInput, code: &str, message: &str) -> Diagnostic {
    let primary = Location {
        source: input.source.version,
        span: Span { start: 0, end: 0 },
    };

    Diagnostic {
        identity: FindingIdentity::new("format", vec![input.source.version], code, primary),
        code: code.to_owned(),
        category: DiagnosticCategory::Formatting,
        severity: Severity::Error,
        message: message.to_owned(),
        primary,
        related: Vec::new(),
        tags: Vec::new(),
    }
}

/// Serves isolated formatting requests using shared handshake and result framing.
///
/// # Errors
/// Returns malformed input, worker protocol or stream failures.
pub fn run_worker() -> io::Result<()> {
    serve("format", |request, emitter| execute(&request, emitter))
}
