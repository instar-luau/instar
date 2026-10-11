use instar_luau::{CompletionCandidate, CompletionContext, CompletionKind, Notation, Parentheses};

use instar_project::{
    AssistanceOutput, CompletionQuery, DocumentationEntry, PositionQueryKind, ProjectSnapshot,
    Query, QueryOutput, QueryReport,
};

use instar_shared::{Completion, Location as DomainLocation, Options, SourceVersion, Span};
use instar_syntax::literal::quote;
use serde_json::{Value, json};

use tower_lsp_server::{
    gen_lsp_types::{
        ActiveParameter, CodeLens, CodeLensParams, Command, CompletionItem, CompletionItemKind,
        CompletionItemTag, CompletionList, CompletionParams, CompletionResponse, Documentation,
        Hover, HoverParams, InlayHint, InlayHintKind, InlayHintParams, InlineCompletionParams,
        InlineCompletionResponse, InlineValue, InlineValueParams, InsertTextFormat, MarkupContent,
        ParameterInformation, ParameterInformationLabel, Position, SignatureHelp,
        SignatureHelpParams, SignatureInformation, TextEdit as ProtocolTextEdit,
    },
    jsonrpc::{Error, Result},
};

use crate::{
    Server, markup, position,
    session::{LazyRecord, RetainedResult, failure, known},
};

pub(crate) async fn hover(server: &Server, parameters: HoverParams) -> Result<Option<Hover>> {
    let parameters_position = parameters.text_document_position_params;

    let options = server.options.child();

    let (snapshot, position) = server.position(
        &parameters_position.text_document.uri,
        parameters_position.position,
        PositionQueryKind::Hover,
        &options,
    )?;

    let source_version = position.source;

    let report = server
        .query(
            snapshot,
            Query::Inspect(position),
            parameters.work_done_progress_params.work_done_token,
            &options,
        )
        .await?;

    let QueryOutput::Inspection(inspection) = report.output else {
        return Err(Error::internal_error());
    };

    let Some(occurrence) = inspection.occurrence else {
        return Ok(None);
    };

    let information = known(occurrence.type_information)?;
    let symbol = known(occurrence.documentation)?;

    let format = server
        .state
        .lock()
        .expect("session state")
        .capabilities
        .hover_markup();

    let mut value = markup::code(&information.display, &format);

    if occurrence.deprecated {
        value.push_str("\n\nDeprecated.");
    }

    if let Some(symbol) = symbol
        && let Some(entry) = documentation(
            server,
            report.snapshot.clone(),
            source_version,
            &symbol,
            &options,
        )
        .await?
    {
        let content = markup::documentation(entry, format.clone());

        let text = match content {
            Documentation::String(text) => text,
            Documentation::MarkupContent(content) => content.value,
        };

        value.push_str("\n\n");
        value.push_str(&text);
    }

    Ok(Some(Hover {
        contents: MarkupContent {
            kind: format,
            value,
        }
        .into(),
        range: Some(server.range(&report.snapshot, occurrence.location)?),
    }))
}

pub(crate) async fn completion(
    server: &Server,
    parameters: CompletionParams,
) -> Result<Option<CompletionResponse>> {
    let parameters_position = parameters.text_document_position_params;

    let options = server.options.child();

    let (snapshot, position) = server.position(
        &parameters_position.text_document.uri,
        parameters_position.position,
        PositionQueryKind::Hover,
        &options,
    )?;

    let source_version = position.source;

    let report = server
        .query(
            snapshot,
            Query::Complete(CompletionQuery {
                position,
                inline: false,
            }),
            parameters.work_done_progress_params.work_done_token,
            &options,
        )
        .await?;

    let QueryOutput::Completions(completions) = &report.output else {
        return Err(Error::internal_error());
    };

    let source = report
        .snapshot
        .source(source_version.identity)
        .ok_or_else(Error::content_modified)?;

    let (snippets, deprecated, commits, capabilities) = {
        let state = server.state.lock().expect("session state");

        let capabilities = state
            .capabilities
            .text_document()
            .and_then(|document| document.completion.as_ref())
            .cloned()
            .unwrap_or_default();

        let item = capabilities.completion_item.as_ref();

        (
            item.is_some_and(|item| item.snippet_support == Some(true)),
            item.is_some_and(|item| item.tag_support.is_some()),
            item.is_some_and(|item| item.commit_characters_support == Some(true)),
            capabilities,
        )
    };

    let deferred = capabilities
        .completion_item
        .as_ref()
        .and_then(|item| item.resolve_support.as_ref())
        .is_some_and(|support| {
            support
                .properties
                .iter()
                .any(|property| property == "additionalTextEdits")
        });

    let mut items = Vec::new();

    for proposed in &completions.candidates {
        if !deferred && !proposed.edits.is_empty() {
            continue;
        }

        let candidate = &proposed.entry;

        let (text, filter_text, span, snippet) = insertion(
            &source.text,
            completions.native.replacement.span,
            completions.native.operator.map(|location| location.span),
            completions.native.context,
            candidate,
            snippets,
        )?;

        let encoding = server.state.lock().expect("session state").encoding.clone();

        let range = position::range(&source.text, [span.start, span.end], &encoding)
            .map_err(|error| failure(&error))?;

        if range.start.line != range.end.line {
            continue;
        }

        let kind = kind(candidate);

        let supported = capabilities
            .completion_item_kind
            .as_ref()
            .and_then(|kind| kind.value_set.as_ref())
            .is_none_or(|kinds| kinds.contains(&kind));

        let mut item = CompletionItem {
            label: candidate.name.clone(),
            kind: Some(if supported {
                kind
            } else {
                CompletionItemKind::Text
            }),
            detail: (!candidate.detail.is_empty()).then(|| candidate.detail.clone()),
            tags: (deprecated && candidate.deprecated)
                .then_some(vec![CompletionItemTag::Deprecated]),
            sort_text: Some(format!(
                "{}{}",
                if candidate.preferred { "0" } else { "1" },
                candidate.name
            )),
            filter_text: Some(filter_text),
            insert_text_format: snippet.then_some(InsertTextFormat::Snippet),
            text_edit: Some(
                ProtocolTextEdit {
                    range,
                    new_text: text,
                }
                .into(),
            ),
            commit_characters: (commits
                && !snippet
                && completions.native.context != CompletionContext::String
                && candidate.parentheses == Parentheses::None)
                .then(|| vec![".".to_owned(), ":".to_owned()]),
            ..CompletionItem::default()
        };

        let mut edits = Vec::new();
        let mut spans = vec![span];

        for edit in &proposed.edits {
            if edit.location.source != source_version
                || spans.iter().any(|span| {
                    edit.location.span.start <= span.end && span.start <= edit.location.span.end
                })
            {
                return Err(Error::invalid_params(
                    "completion edits must be disjoint and use one source revision",
                ));
            }

            edits.push(server.text_edit(&report.snapshot, edit)?);
            spans.push(edit.location.span);
        }

        item.data = Some(retain(
            server,
            &report,
            Some(source_version),
            RetainedResult::Completion {
                item: item.clone(),
                documentation: candidate.documentation.clone(),
                edits,
            },
        )?);

        items.push(item);
    }

    Ok(Some(
        CompletionList {
            is_incomplete: completions.native.require
                || completions.native.completion != Completion::Complete
                || report.completion != Completion::Complete,
            items,
            ..CompletionList::default()
        }
        .into(),
    ))
}

pub(crate) async fn completion_resolve(
    server: &Server,
    item: CompletionItem,
) -> Result<CompletionItem> {
    let record = lazy(server, item.data.as_ref(), "completion")?;
    let options = server.options.child();

    let RetainedResult::Completion {
        item: mut resolved,
        documentation: symbol,
        edits,
    } = record.result
    else {
        return Err(Error::internal_error());
    };

    resolved.additional_text_edits = (!edits.is_empty()).then_some(edits);

    if let Some(symbol) = symbol {
        let entry = documentation(
            server,
            record.snapshot.clone(),
            record.source.ok_or_else(Error::content_modified)?,
            &symbol,
            &options,
        )
        .await?;

        let format = server
            .state
            .lock()
            .expect("session state")
            .capabilities
            .completion_markup();

        resolved.documentation = entry.map(|entry| markup::documentation(entry, format));
    }

    if !server
        .state
        .lock()
        .expect("session state")
        .current(&server.project, &record.inputs)
    {
        return Err(Error::content_modified());
    }

    resolved.data = item.data;

    Ok(resolved)
}

pub(crate) async fn inline_completion(
    server: &Server,
    parameters: InlineCompletionParams,
) -> Result<Option<InlineCompletionResponse>> {
    if server
        .state
        .lock()
        .expect("session state")
        .capabilities
        .text_document()
        .is_none_or(|document| document.inline_completion.is_none())
    {
        return Err(Error::invalid_params(
            "inline completion was not negotiated",
        ));
    }

    let parameters_position = parameters.text_document_position_params;

    let options = server.options.child();

    let (snapshot, position) = server.position(
        &parameters_position.text_document.uri,
        parameters_position.position,
        PositionQueryKind::Hover,
        &options,
    )?;

    let source_version = position.source;

    let report = server
        .query(
            snapshot,
            Query::Complete(CompletionQuery {
                position,
                inline: true,
            }),
            parameters.work_done_progress_params.work_done_token,
            &options,
        )
        .await?;

    let QueryOutput::Completions(completions) = &report.output else {
        return Err(Error::internal_error());
    };

    let source = report
        .snapshot
        .source(source_version.identity)
        .ok_or_else(Error::content_modified)?;

    let mut items = Vec::new();

    for proposed in &completions.candidates {
        if !proposed.edits.is_empty() {
            continue;
        }

        let candidate = &proposed.entry;

        let (text, filter, span, _) = insertion(
            &source.text,
            completions.native.replacement.span,
            completions.native.operator.map(|location| location.span),
            completions.native.context,
            candidate,
            false,
        )?;

        let location = DomainLocation {
            source: source_version,
            span,
        };

        items.push(json!({"insertText":text,"filterText":filter,"range":server.range(&report.snapshot, location)?}));
    }

    serde_json::from_value(json!({"items":items}))
        .map(Some)
        .map_err(|error| Error {
            message: error.to_string().into(),
            ..Error::internal_error()
        })
}

pub(crate) async fn signatures(
    server: &Server,
    parameters: SignatureHelpParams,
) -> Result<Option<SignatureHelp>> {
    let parameters_position = parameters.text_document_position_params;

    let options = server.options.child();

    let (snapshot, position) = server.position(
        &parameters_position.text_document.uri,
        parameters_position.position,
        PositionQueryKind::Signatures,
        &options,
    )?;

    let report = server
        .query(
            snapshot,
            Query::Inspect(position),
            parameters.work_done_progress_params.work_done_token,
            &options,
        )
        .await?;

    let QueryOutput::Inspection(inspection) = report.output else {
        return Err(Error::internal_error());
    };

    let Some(occurrence) = inspection.occurrence else {
        return Ok(None);
    };

    let entries = known(occurrence.signatures)?;

    let (offsets, active_support, no_active, format) = {
        let state = server.state.lock().expect("session state");

        let information = state
            .capabilities
            .text_document()
            .and_then(|document| document.signature_help.as_ref())
            .and_then(|signature| signature.signature_information.as_ref());

        (
            information
                .and_then(|information| information.parameter_information.as_ref())
                .is_some_and(|parameter| parameter.label_offset_support == Some(true)),
            information
                .is_some_and(|information| information.active_parameter_support == Some(true)),
            information
                .is_some_and(|information| information.no_active_parameter_support == Some(true)),
            state.capabilities.signature_markup(),
        )
    };

    let mut signatures = Vec::new();
    let mut active_parameter = None;

    for (index, signature) in entries.into_iter().enumerate() {
        let active = occurrence.active_argument.and_then(|argument| {
            if argument < signature.parameters.len() {
                Some(argument)
            } else if signature.variadic {
                signature.parameters.len().checked_sub(1)
            } else {
                None
            }
        });

        let active = active
            .map(|active| u32::try_from(active).map(ActiveParameter::Int))
            .transpose()
            .map_err(|error| Error {
                message: error.to_string().into(),
                ..Error::internal_error()
            })?
            .or_else(|| no_active.then_some(ActiveParameter::Null));

        if index == occurrence.active_signature.unwrap_or(0) {
            active_parameter = active;
        }

        let parameters = signature
            .parameters
            .iter()
            .map(|parameter| {
                let [start, end] = parameter.label_range;

                let label = signature
                    .label
                    .get(start..end)
                    .ok_or_else(Error::internal_error)?;

                let label = if offsets {
                    ParameterInformationLabel::Tuple((
                        u32::try_from(signature.label[..start].encode_utf16().count()).map_err(
                            |error| Error {
                                message: error.to_string().into(),
                                ..Error::internal_error()
                            },
                        )?,
                        u32::try_from(signature.label[..end].encode_utf16().count()).map_err(
                            |error| Error {
                                message: error.to_string().into(),
                                ..Error::internal_error()
                            },
                        )?,
                    ))
                } else {
                    ParameterInformationLabel::String(label.to_owned())
                };

                Ok(ParameterInformation {
                    label,
                    documentation: None,
                })
            })
            .collect::<Result<Vec<_>>>()?;

        let content = if let Some(symbol) = &signature.documentation {
            documentation(
                server,
                report.snapshot.clone(),
                occurrence.location.source,
                symbol,
                &options,
            )
            .await?
            .map(|entry| markup::documentation(entry, format.clone()))
        } else {
            None
        };

        signatures.push(SignatureInformation {
            label: signature.label,
            documentation: content,
            parameters: Some(parameters),
            active_parameter: if active_support { active } else { None },
        });
    }

    if signatures.is_empty() {
        return Ok(None);
    }

    Ok(Some(SignatureHelp {
        signatures,
        active_signature: occurrence
            .active_signature
            .map(u32::try_from)
            .transpose()
            .map_err(|error| Error {
                message: error.to_string().into(),
                ..Error::internal_error()
            })?,
        active_parameter,
    }))
}

pub(crate) async fn hints(
    server: &Server,
    parameters: InlayHintParams,
) -> Result<Option<Vec<InlayHint>>> {
    let options = server.options.child();

    let (snapshot, position) = server.position(
        &parameters.text_document.uri,
        parameters.range.start,
        PositionQueryKind::Hints,
        &options,
    )?;

    let source_version = position.source;

    let report = server
        .query(
            snapshot,
            Query::Inspect(position),
            parameters.work_done_progress_params.work_done_token,
            &options,
        )
        .await?;

    let QueryOutput::Assistance(AssistanceOutput::Hints(hints)) = &report.output else {
        return Err(Error::internal_error());
    };

    let deferred = server
        .state
        .lock()
        .expect("session state")
        .capabilities
        .hint_tooltips();

    let mut result = Vec::new();

    for hint in hints {
        let range = server.range(&report.snapshot, hint.location)?;

        if range.end < parameters.range.start || range.end > parameters.range.end {
            continue;
        }

        let mut item = InlayHint {
            position: range.end,
            label: hint.label.clone().into(),
            kind: Some(InlayHintKind::Type),
            tooltip: if deferred {
                None
            } else {
                hint.description.clone().map(Into::into)
            },
            text_edits: None,
            padding_left: None,
            padding_right: None,
            data: None,
        };

        item.data = Some(retain(
            server,
            &report,
            Some(source_version),
            RetainedResult::Hint {
                item: item.clone(),
                description: hint.description.clone(),
                documentation: hint.documentation.clone(),
            },
        )?);

        if !deferred {
            item = hint_resolve(server, item).await?;
            item.data = None;
        }

        result.push(item);
    }

    Ok(Some(result))
}

pub(crate) async fn hint_resolve(server: &Server, hint: InlayHint) -> Result<InlayHint> {
    let options = server.options.child();
    let record = lazy(server, hint.data.as_ref(), "hint")?;

    let RetainedResult::Hint {
        item: mut resolved,
        description,
        documentation: symbol,
    } = record.result
    else {
        return Err(Error::internal_error());
    };

    let mut description = description.unwrap_or_default();

    if let Some(symbol) = symbol
        && let Some(documentation) = documentation(
            server,
            record.snapshot.clone(),
            record.source.ok_or_else(Error::content_modified)?,
            &symbol,
            &options,
        )
        .await?
    {
        for text in [
            documentation.documentation,
            documentation.code_sample,
            documentation.learn_more_link,
        ]
        .into_iter()
        .flatten()
        {
            if !description.is_empty() {
                description.push_str("\n\n");
            }

            description.push_str(&text);
        }
    }

    resolved.tooltip = (!description.is_empty()).then(|| description.into());
    resolved.data = hint.data;

    if !server
        .state
        .lock()
        .expect("session state")
        .current(&server.project, &record.inputs)
    {
        return Err(Error::content_modified());
    }

    Ok(resolved)
}

pub(crate) async fn lenses(
    server: &Server,
    parameters: CodeLensParams,
) -> Result<Option<Vec<CodeLens>>> {
    let options = server.options.child();

    let (snapshot, position) = server.position(
        &parameters.text_document.uri,
        Position::default(),
        PositionQueryKind::Lenses,
        &options,
    )?;

    let source_version = position.source;

    let report = server
        .query(
            snapshot,
            Query::Inspect(position),
            parameters.work_done_progress_params.work_done_token,
            &options,
        )
        .await?;

    let QueryOutput::Assistance(AssistanceOutput::Lenses(lenses)) = &report.output else {
        return Err(Error::internal_error());
    };

    let mut result = Vec::new();

    for lens in lenses {
        let argument = retain(
            server,
            &report,
            Some(source_version),
            RetainedResult::Command {
                command: "instar.check".to_owned(),
                edit: None,
            },
        )?;

        let mut item = CodeLens {
            range: server.range(&report.snapshot, lens.location)?,
            command: None,
            data: None,
        };

        item.data = Some(retain(
            server,
            &report,
            Some(source_version),
            RetainedResult::Lens {
                item: item.clone(),
                command: Command {
                    title: lens.title.clone(),
                    tooltip: None,
                    command: "instar.check".to_owned(),
                    arguments: Some(vec![argument]),
                },
            },
        )?);

        result.push(item);
    }

    Ok(Some(result))
}

pub(crate) fn lens_resolve(server: &Server, lens: CodeLens) -> Result<CodeLens> {
    let record = lazy(server, lens.data.as_ref(), "lens")?;

    let RetainedResult::Lens {
        item: mut result,
        command,
    } = record.result
    else {
        return Err(Error::internal_error());
    };

    result.command = Some(command);

    result.data = lens.data;

    Ok(result)
}

pub(crate) async fn inline_values(
    server: &Server,
    parameters: InlineValueParams,
) -> Result<Option<Vec<InlineValue>>> {
    let options = server.options.child();

    let (snapshot, position) = server.position(
        &parameters.text_document.uri,
        parameters.range.start,
        PositionQueryKind::InlineValues,
        &options,
    )?;

    let report = server
        .query(
            snapshot,
            Query::Inspect(position),
            parameters.work_done_progress_params.work_done_token,
            &options,
        )
        .await?;

    let QueryOutput::Assistance(AssistanceOutput::InlineValues(values)) = &report.output else {
        return Err(Error::internal_error());
    };

    let mut result = Vec::new();

    for value in values {
        let range = server.range(&report.snapshot, value.location)?;

        if range.end < parameters.range.start || range.start > parameters.range.end {
            continue;
        }

        let item = if let Some(variable) = &value.variable {
            json!({"range":range,"variableName":variable,"caseSensitiveLookup":true})
        } else if let Some(expression) = &value.expression {
            json!({"range":range,"expression":expression})
        } else {
            return Err(Error::internal_error());
        };

        result.push(serde_json::from_value(item).map_err(|error| Error {
            message: error.to_string().into(),
            ..Error::internal_error()
        })?);
    }

    Ok(Some(result))
}

pub(crate) fn retain(
    server: &Server,
    report: &QueryReport,
    source: Option<SourceVersion>,
    result: RetainedResult,
) -> Result<Value> {
    let mut state = server.state.lock().expect("session state");

    if !state.current(&server.project, &report.inputs) {
        return Err(Error::content_modified());
    }

    state.identities.insert(LazyRecord {
        source,
        inputs: report.inputs.clone(),
        snapshot: report.snapshot.clone(),
        result,
    })
}

pub(crate) fn lazy(server: &Server, data: Option<&Value>, operation: &str) -> Result<LazyRecord> {
    let state = server.state.lock().expect("session state");
    let record = state.identities.get(data, operation)?;

    if !state.current(&server.project, &record.inputs) {
        return Err(Error::content_modified());
    }

    if let Some(source) = record.source
        && server
            .project
            .snapshot()
            .source(source.identity)
            .is_none_or(|current| current.version != source)
    {
        return Err(Error::content_modified());
    }

    Ok(record)
}

async fn documentation(
    server: &Server,
    snapshot: ProjectSnapshot,
    source: SourceVersion,
    symbol: &str,
    options: &Options,
) -> Result<Option<DocumentationEntry>> {
    let report = server
        .query(
            snapshot,
            Query::Documentation {
                source,
                symbol: symbol.to_owned(),
            },
            None,
            options,
        )
        .await?;

    let QueryOutput::Documentation(entry) = report.output else {
        return Err(Error::internal_error());
    };

    Ok(entry)
}

fn insertion(
    source: &str,
    replacement: Span,
    operator: Option<Span>,
    context: CompletionContext,
    candidate: &CompletionCandidate,
    snippets: bool,
) -> Result<(String, String, Span, bool)> {
    let mut range = replacement;

    let mut text = candidate
        .insert_text
        .as_deref()
        .unwrap_or(&candidate.name)
        .to_owned();

    let mut filter = candidate.name.clone();

    if context == CompletionContext::String {
        text = quote(&text);

        let original = source
            .get(range.start..range.end)
            .ok_or_else(Error::internal_error)?;

        let opening = if let Some(content) = original.strip_prefix('[') {
            content.find('[').map_or("", |end| &original[..end + 2])
        } else {
            original.get(..1).unwrap_or("")
        };

        filter = format!("{opening}{}", candidate.name);
    } else if let Some(operator) = operator {
        let prefix = source
            .get(operator.start..operator.end)
            .ok_or_else(Error::internal_error)?;

        match candidate.notation {
            Notation::Identifier => {}

            Notation::Index => {
                range.start = operator.start;
                filter = format!("{prefix}{}", candidate.name);
                text = format!("[{}]", quote(&text));
            }

            Notation::Method => {
                range.start = operator.start;
                filter = format!("{prefix}{}", candidate.name);
                text = format!(":{text}");
            }
        }
    }

    let mut snippet = false;

    if snippets && candidate.kind == CompletionKind::Function && text.ends_with("  end") {
        text = format!("{}\n\t$0\nend", escape(&text[..text.len() - 5]));
        snippet = true;
    } else if context != CompletionContext::String
        && !source
            .get(replacement.end..)
            .ok_or_else(Error::internal_error)?
            .trim_start()
            .starts_with('(')
    {
        match candidate.parentheses {
            Parentheses::After => text.push_str("()"),

            Parentheses::Inside if snippets => {
                text = format!("{}($0)", escape(&text));
                snippet = true;
            }

            Parentheses::None | Parentheses::Inside => {}
        }
    }

    Ok((text, filter, range, snippet))
}

fn kind(candidate: &CompletionCandidate) -> CompletionItemKind {
    match candidate.kind {
        CompletionKind::Property if candidate.callable => CompletionItemKind::Method,
        CompletionKind::Binding if candidate.callable => CompletionItemKind::Function,
        CompletionKind::Property => CompletionItemKind::Field,
        CompletionKind::Binding => CompletionItemKind::Variable,
        CompletionKind::Keyword | CompletionKind::Comment => CompletionItemKind::Keyword,
        CompletionKind::String => CompletionItemKind::Value,
        CompletionKind::Type => CompletionItemKind::Class,
        CompletionKind::Module => CompletionItemKind::Module,
        CompletionKind::Function => CompletionItemKind::Snippet,
        CompletionKind::Path if candidate.name.ends_with('/') => CompletionItemKind::Folder,
        CompletionKind::Path => CompletionItemKind::File,
    }
}

fn escape(text: &str) -> String {
    text.replace('\\', "\\\\")
        .replace('$', "\\$")
        .replace('}', "\\}")
}
