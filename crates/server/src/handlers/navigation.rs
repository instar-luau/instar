use instar_project::{
    LinkQuery, PositionQueryKind, Query, QueryOutput, ReferenceQuery, Symbol,
    SymbolKind as DomainSymbolKind, SymbolQuery,
};

use serde_json::{Value, json};

use tower_lsp_server::{
    gen_lsp_types::{
        BaseSymbolInformation, DocumentHighlight, DocumentHighlightKind, DocumentHighlightParams,
        DocumentLink, DocumentLinkParams, DocumentSymbol, DocumentSymbolParams,
        DocumentSymbolResponse, Location as ProtocolLocation, PositionEncodingKind, ProgressToken,
        ReferenceParams, ReferencesRequest, SymbolInformation, SymbolKind,
        TextDocumentPositionParams, Uri, WorkspaceSymbol, WorkspaceSymbolParams,
        WorkspaceSymbolResponse,
    },
    jsonrpc::{Error, Result},
};

use super::assistance::{lazy, retain};
use crate::{Server, position, session::failure};

pub(crate) async fn navigate(
    server: &Server,
    parameters: TextDocumentPositionParams,
    kind: PositionQueryKind,
    token: Option<ProgressToken>,
) -> Result<Vec<ProtocolLocation>> {
    let options = server.options.child();

    let (snapshot, position) = server.position(
        &parameters.text_document.uri,
        parameters.position,
        kind,
        &options,
    )?;

    let report = server
        .query(snapshot, Query::Inspect(position), token, &options)
        .await?;

    let QueryOutput::Locations(locations) = report.output else {
        return Err(Error::internal_error());
    };

    locations
        .into_iter()
        .map(|location| server.location(&report.snapshot, location))
        .collect()
}

pub(crate) async fn references(
    server: &Server,
    parameters: ReferenceParams,
) -> Result<Option<Vec<ProtocolLocation>>> {
    let parameters_position = parameters.text_document_position_params;

    let options = server.options.child();

    let (snapshot, position) = server.position(
        &parameters_position.text_document.uri,
        parameters_position.position,
        PositionQueryKind::Definition,
        &options,
    )?;

    let report = server
        .query(
            snapshot,
            Query::References(ReferenceQuery {
                position,
                include_declaration: parameters.context.include_declaration,
                workspace: true,
                highlights: false,
            }),
            parameters.work_done_progress_params.work_done_token,
            &options,
        )
        .await?;

    let QueryOutput::References(occurrences) = report.output else {
        return Err(Error::internal_error());
    };

    let mut locations = occurrences
        .into_iter()
        .filter(|occurrence| parameters.context.include_declaration || !occurrence.declaration)
        .map(|occurrence| server.location(&report.snapshot, occurrence.location))
        .collect::<Result<Vec<_>>>()?;

    locations.sort_by(|left, right| {
        (
            left.uri.as_str(),
            left.range.start.line,
            left.range.start.character,
            left.range.end.line,
            left.range.end.character,
        )
            .cmp(&(
                right.uri.as_str(),
                right.range.start.line,
                right.range.start.character,
                right.range.end.line,
                right.range.end.character,
            ))
    });

    if let Some(token) = parameters.partial_result_params.partial_result_token {
        server
            .client
            .send_partial_result::<ReferencesRequest>(token, locations)
            .await;

        return Ok(Some(Vec::new()));
    }

    Ok(Some(locations))
}

pub(crate) async fn highlights(
    server: &Server,
    parameters: DocumentHighlightParams,
) -> Result<Option<Vec<DocumentHighlight>>> {
    let parameters_position = parameters.text_document_position_params;

    let options = server.options.child();

    let (snapshot, position) = server.position(
        &parameters_position.text_document.uri,
        parameters_position.position,
        PositionQueryKind::Definition,
        &options,
    )?;

    let source = position.source;

    let report = server
        .query(
            snapshot,
            Query::References(ReferenceQuery {
                position,
                include_declaration: true,
                workspace: false,
                highlights: true,
            }),
            parameters.work_done_progress_params.work_done_token,
            &options,
        )
        .await?;

    let QueryOutput::References(occurrences) = report.output else {
        return Err(Error::internal_error());
    };

    occurrences
        .into_iter()
        .filter(|occurrence| occurrence.location.source == source)
        .map(|occurrence| {
            Ok(DocumentHighlight {
                range: server.range(&report.snapshot, occurrence.location)?,
                kind: Some(if occurrence.access.writes() {
                    DocumentHighlightKind::Write
                } else {
                    DocumentHighlightKind::Read
                }),
            })
        })
        .collect::<Result<Vec<_>>>()
        .map(Some)
}

pub(crate) async fn symbols(
    server: &Server,
    parameters: DocumentSymbolParams,
) -> Result<Option<DocumentSymbolResponse>> {
    let options = server.options.child();
    let (snapshot, source) = server.capture(&parameters.text_document.uri, &options)?;

    let report = server
        .query(
            snapshot,
            Query::Symbols(SymbolQuery {
                source: Some(source.version.identity),
                search: String::new(),
            }),
            parameters.work_done_progress_params.work_done_token,
            &options,
        )
        .await?;

    let QueryOutput::Symbols(symbols) = report.output else {
        return Err(Error::internal_error());
    };

    let (hierarchical, kinds, encoding) = {
        let state = server.state.lock().expect("session state");

        let capabilities = state
            .capabilities
            .client
            .text_document
            .as_ref()
            .and_then(|document| document.document_symbol.as_ref());

        (
            capabilities
                .and_then(|symbols| symbols.hierarchical_document_symbol_support)
                .unwrap_or(false),
            capabilities
                .and_then(|symbols| symbols.symbol_kind.as_ref())
                .and_then(|kinds| kinds.value_set.clone()),
            state.encoding.clone(),
        )
    };

    let source = report
        .snapshot
        .source(source.version.identity)
        .ok_or_else(Error::content_modified)?;

    let symbols = symbols
        .into_iter()
        .map(|symbol| render(symbol.symbol, &source.text, &encoding, kinds.as_deref()))
        .collect::<Result<Vec<_>>>()?;

    if hierarchical {
        Ok(Some(DocumentSymbolResponse::DocumentSymbolList(symbols)))
    } else {
        let mut flattened = Vec::new();
        flatten(symbols, &parameters.text_document.uri, None, &mut flattened);

        Ok(Some(DocumentSymbolResponse::SymbolInformationList(
            flattened,
        )))
    }
}

pub(crate) async fn search(
    server: &Server,
    parameters: WorkspaceSymbolParams,
) -> Result<Option<WorkspaceSymbolResponse>> {
    let options = server.options.child();

    let report = server
        .query(
            server.project.snapshot(),
            Query::Symbols(SymbolQuery {
                source: None,
                search: parameters.query,
            }),
            parameters.work_done_progress_params.work_done_token,
            &options,
        )
        .await?;

    let QueryOutput::Symbols(symbols) = &report.output else {
        return Err(Error::internal_error());
    };

    let capabilities = server
        .state
        .lock()
        .expect("session state")
        .capabilities
        .client
        .workspace
        .as_ref()
        .and_then(|workspace| workspace.symbol.as_ref())
        .cloned()
        .unwrap_or_default();

    let kinds = capabilities
        .symbol_kind
        .as_ref()
        .and_then(|kinds| kinds.value_set.as_deref());

    let deferred = capabilities.resolve_support.is_some();

    if deferred {
        let mut result = Vec::new();

        for symbol in symbols {
            let location = server.location(&report.snapshot, symbol.location)?;
            let mut item: WorkspaceSymbol = serde_json::from_value(json!({"name":symbol.symbol.name,"kind":kind(symbol.symbol.kind,kinds),"location":{"uri":location.uri},"containerName":symbol.container})).map_err(|_| Error::internal_error())?;

            item.data = Some(retain(
                server,
                &report,
                Some(symbol.location.source),
                "symbol",
                json!({"item":item,"location":location}),
            )?);

            result.push(item);
        }

        Ok(Some(WorkspaceSymbolResponse::WorkspaceSymbolList(result)))
    } else {
        let result = symbols
            .iter()
            .map(|symbol| {
                Ok(SymbolInformation::new(
                    None,
                    server.location(&report.snapshot, symbol.location)?,
                    BaseSymbolInformation::new(
                        symbol.symbol.name.clone(),
                        kind(symbol.symbol.kind, kinds),
                        None,
                        symbol.container.clone(),
                    ),
                ))
            })
            .collect::<Result<Vec<_>>>()?;

        Ok(Some(WorkspaceSymbolResponse::SymbolInformationList(result)))
    }
}

pub(crate) async fn symbol_resolve(
    server: &Server,
    symbol: WorkspaceSymbol,
) -> Result<WorkspaceSymbol> {
    let record = lazy(server, symbol.data.as_ref(), "symbol")?;
    let mut item = record.payload["item"].clone();
    item["location"] = record.payload["location"].clone();
    item["data"] = symbol.data.map_or(Value::Null, |data| data);

    serde_json::from_value(item).map_err(|_| Error::internal_error())
}

pub(crate) async fn links(
    server: &Server,
    parameters: DocumentLinkParams,
) -> Result<Option<Vec<DocumentLink>>> {
    let options = server.options.child();
    let (snapshot, source) = server.capture(&parameters.text_document.uri, &options)?;

    let report = server
        .query(
            snapshot,
            Query::Links(LinkQuery {
                source: source.version,
            }),
            parameters.work_done_progress_params.work_done_token,
            &options,
        )
        .await?;

    let QueryOutput::Links(links) = &report.output else {
        return Err(Error::internal_error());
    };

    let tooltips = server
        .state
        .lock()
        .expect("session state")
        .capabilities
        .supports("/textDocument/documentLink/tooltipSupport");

    let mut result = Vec::new();

    for link in links {
        let target = if let Some(identity) = link.target {
            Some(
                server
                    .state
                    .lock()
                    .expect("session state")
                    .uri(&report.snapshot, identity)?,
            )
        } else if let Some(address) = &link.address {
            let address = address
                .parse::<Uri>()
                .map_err(|_| Error::invalid_params("invalid document link address"))?;

            if !matches!(address.scheme(), "http" | "https") || address.host_str().is_none() {
                return Err(Error::invalid_params(
                    "unsupported documentation link address",
                ));
            }

            Some(address)
        } else {
            None
        };

        let mut item = DocumentLink {
            range: server.range(&report.snapshot, link.location)?,
            target: target.clone(),
            tooltip: target
                .as_ref()
                .filter(|_| tooltips)
                .map(|target| target.as_str().to_owned()),
            data: None,
        };

        item.data = Some(retain(
            server,
            &report,
            Some(source.version),
            "link",
            json!({"item":item,"target":target}),
        )?);

        result.push(item);
    }

    Ok(Some(result))
}

pub(crate) async fn link_resolve(server: &Server, link: DocumentLink) -> Result<DocumentLink> {
    let record = lazy(server, link.data.as_ref(), "link")?;

    let mut result: DocumentLink = serde_json::from_value(record.payload["item"].clone())
        .map_err(|_| Error::internal_error())?;

    result.target = serde_json::from_value(record.payload["target"].clone())
        .map_err(|_| Error::internal_error())?;

    result.data = link.data;

    Ok(result)
}

fn render(
    symbol: Symbol,
    source: &str,
    encoding: &PositionEncodingKind,
    kinds: Option<&[SymbolKind]>,
) -> Result<DocumentSymbol> {
    if symbol.range[0] > symbol.selection[0] || symbol.selection[1] > symbol.range[1] {
        return Err(Error::invalid_params(
            "symbol selection is outside its declaration",
        ));
    }

    let range = position::range(source, symbol.range, encoding).map_err(|error| failure(&error))?;

    let selection =
        position::range(source, symbol.selection, encoding).map_err(|error| failure(&error))?;

    let children = symbol
        .children
        .into_iter()
        .map(|child| render(child, source, encoding, kinds))
        .collect::<Result<Vec<_>>>()?;

    Ok(DocumentSymbol::new(
        symbol.name,
        None,
        kind(symbol.kind, kinds),
        None,
        None,
        range,
        selection,
        (!children.is_empty()).then_some(children),
    ))
}

fn flatten(
    symbols: Vec<DocumentSymbol>,
    uri: &Uri,
    parent: Option<&str>,
    output: &mut Vec<SymbolInformation>,
) {
    for symbol in symbols {
        output.push(SymbolInformation::new(
            None,
            ProtocolLocation {
                uri: uri.clone(),
                range: symbol.range,
            },
            BaseSymbolInformation::new(
                symbol.name.clone(),
                symbol.kind,
                None,
                parent.map(str::to_owned),
            ),
        ));

        flatten(
            symbol.children.unwrap_or_default(),
            uri,
            Some(&symbol.name),
            output,
        );
    }
}

fn kind(kind: DomainSymbolKind, supported: Option<&[SymbolKind]>) -> SymbolKind {
    let kind = match kind {
        DomainSymbolKind::Variable | DomainSymbolKind::Object => SymbolKind::Variable,
        DomainSymbolKind::Constant => SymbolKind::Constant,
        DomainSymbolKind::Function => SymbolKind::Function,
        DomainSymbolKind::Method => SymbolKind::Method,
        DomainSymbolKind::Type => SymbolKind::Interface,
        DomainSymbolKind::Class => SymbolKind::Class,
        DomainSymbolKind::Field => SymbolKind::Field,
    };

    match supported {
        Some(kinds) if !kinds.contains(&kind) => kinds
            .iter()
            .find(|kind| **kind == SymbolKind::Variable)
            .or_else(|| kinds.first())
            .copied()
            .unwrap_or(kind),

        _ => kind,
    }
}
