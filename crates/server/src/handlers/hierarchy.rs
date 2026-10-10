use instar_luau::{FunctionFact, NominalFact};

use instar_project::{
    HierarchyQuery, HierarchyQueryKind, PositionQueryKind, Query, QueryOutput, QueryReport,
};

use tower_lsp_server::{
    gen_lsp_types::{
        CallHierarchyIncomingCall, CallHierarchyIncomingCallsParams, CallHierarchyItem,
        CallHierarchyOutgoingCall, CallHierarchyOutgoingCallsParams, CallHierarchyPrepareParams,
        ProgressToken, SymbolKind, SymbolTag, TypeHierarchyItem, TypeHierarchyPrepareParams,
        TypeHierarchySubtypesParams, TypeHierarchySupertypesParams,
    },
    jsonrpc::{Error, Result},
};

use super::assistance::{lazy, retain};

use crate::{
    Server,
    session::{RetainedResult, failure},
};

pub(crate) async fn prepare_calls(
    server: &Server,
    parameters: CallHierarchyPrepareParams,
) -> Result<Option<Vec<CallHierarchyItem>>> {
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
            Query::Hierarchy(HierarchyQuery {
                position: Some(position),
                identity: None,
                kind: HierarchyQueryKind::PrepareCalls,
            }),
            parameters.work_done_progress_params.work_done_token,
            &options,
        )
        .await?;

    let QueryOutput::Hierarchy(hierarchy) = &report.output else {
        return Err(Error::internal_error());
    };

    hierarchy
        .functions
        .iter()
        .filter(|function| hierarchy.selected.contains(&function.identity))
        .map(|function| call_item(server, &report, function))
        .collect::<Result<Vec<_>>>()
        .map(Some)
}

pub(crate) async fn incoming(
    server: &Server,
    parameters: CallHierarchyIncomingCallsParams,
) -> Result<Option<Vec<CallHierarchyIncomingCall>>> {
    let record = lazy(server, parameters.item.data.as_ref(), "calls")?;

    let RetainedResult::CallHierarchy {
        item: mut retained,
        identity,
    } = record.result
    else {
        return Err(Error::internal_error());
    };

    retained.data.clone_from(&parameters.item.data);

    if retained != parameters.item {
        return Err(Error::invalid_params(
            "hierarchy item does not match its result identity",
        ));
    }

    let options = server.options.child();

    let report = server
        .query(
            record.snapshot,
            Query::Hierarchy(HierarchyQuery {
                position: None,
                identity: Some(identity.clone()),
                kind: HierarchyQueryKind::IncomingCalls,
            }),
            parameters.work_done_progress_params.work_done_token,
            &options,
        )
        .await?;

    let QueryOutput::Hierarchy(hierarchy) = &report.output else {
        return Err(Error::internal_error());
    };

    let mut result: Vec<CallHierarchyIncomingCall> = Vec::new();

    for call in hierarchy.calls.iter().filter(|call| call.to == identity) {
        let function = hierarchy
            .functions
            .iter()
            .find(|function| function.identity == call.from)
            .ok_or_else(Error::internal_error)?;

        let from = call_item(server, &report, function)?;
        let range = server.range(&report.snapshot, call.location)?;

        if let Some(previous) = result.iter_mut().find(|previous| {
            previous.from.uri == from.uri && previous.from.selection_range == from.selection_range
        }) {
            previous.from_ranges.push(range);
        } else {
            result.push(CallHierarchyIncomingCall {
                from,
                from_ranges: vec![range],
            });
        }
    }

    Ok(Some(result))
}

pub(crate) async fn outgoing(
    server: &Server,
    parameters: CallHierarchyOutgoingCallsParams,
) -> Result<Option<Vec<CallHierarchyOutgoingCall>>> {
    let record = lazy(server, parameters.item.data.as_ref(), "calls")?;

    let RetainedResult::CallHierarchy {
        item: mut retained,
        identity,
    } = record.result
    else {
        return Err(Error::internal_error());
    };

    retained.data.clone_from(&parameters.item.data);

    if retained != parameters.item {
        return Err(Error::invalid_params(
            "hierarchy item does not match its result identity",
        ));
    }

    let options = server.options.child();

    let report = server
        .query(
            record.snapshot,
            Query::Hierarchy(HierarchyQuery {
                position: None,
                identity: Some(identity.clone()),
                kind: HierarchyQueryKind::OutgoingCalls,
            }),
            parameters.work_done_progress_params.work_done_token,
            &options,
        )
        .await?;

    let QueryOutput::Hierarchy(hierarchy) = &report.output else {
        return Err(Error::internal_error());
    };

    let mut result: Vec<CallHierarchyOutgoingCall> = Vec::new();

    for call in hierarchy.calls.iter().filter(|call| call.from == identity) {
        let function = hierarchy
            .functions
            .iter()
            .find(|function| function.identity == call.to)
            .ok_or_else(Error::internal_error)?;

        let to = call_item(server, &report, function)?;
        let range = server.range(&report.snapshot, call.location)?;

        if let Some(previous) = result.iter_mut().find(|previous| {
            previous.to.uri == to.uri && previous.to.selection_range == to.selection_range
        }) {
            previous.from_ranges.push(range);
        } else {
            result.push(CallHierarchyOutgoingCall {
                to,
                from_ranges: vec![range],
            });
        }
    }

    Ok(Some(result))
}

pub(crate) async fn prepare_types(
    server: &Server,
    parameters: TypeHierarchyPrepareParams,
) -> Result<Option<Vec<TypeHierarchyItem>>> {
    let parameters_position = parameters.text_document_position_params;

    let options = server.options.child();

    let (snapshot, position) = server.position(
        &parameters_position.text_document.uri,
        parameters_position.position,
        PositionQueryKind::TypeDefinition,
        &options,
    )?;

    let report = server
        .query(
            snapshot,
            Query::Hierarchy(HierarchyQuery {
                position: Some(position),
                identity: None,
                kind: HierarchyQueryKind::PrepareTypes,
            }),
            parameters.work_done_progress_params.work_done_token,
            &options,
        )
        .await?;

    type_items(server, &report).map(Some)
}

pub(crate) async fn supertypes(
    server: &Server,
    parameters: TypeHierarchySupertypesParams,
) -> Result<Option<Vec<TypeHierarchyItem>>> {
    relatives(
        server,
        parameters.item,
        HierarchyQueryKind::Supertypes,
        parameters.work_done_progress_params.work_done_token,
    )
    .await
}

pub(crate) async fn subtypes(
    server: &Server,
    parameters: TypeHierarchySubtypesParams,
) -> Result<Option<Vec<TypeHierarchyItem>>> {
    relatives(
        server,
        parameters.item,
        HierarchyQueryKind::Subtypes,
        parameters.work_done_progress_params.work_done_token,
    )
    .await
}

async fn relatives(
    server: &Server,
    item: TypeHierarchyItem,
    kind: HierarchyQueryKind,
    token: Option<ProgressToken>,
) -> Result<Option<Vec<TypeHierarchyItem>>> {
    let record = lazy(server, item.data.as_ref(), "types")?;

    let RetainedResult::TypeHierarchy {
        item: mut retained,
        identity,
    } = record.result
    else {
        return Err(Error::internal_error());
    };

    retained.data.clone_from(&item.data);

    if retained != item {
        return Err(Error::invalid_params(
            "hierarchy item does not match its result identity",
        ));
    }

    let options = server.options.child();

    let report = server
        .query(
            record.snapshot,
            Query::Hierarchy(HierarchyQuery {
                position: None,
                identity: Some(identity),
                kind,
            }),
            token,
            &options,
        )
        .await?;

    type_items(server, &report).map(Some)
}

fn type_items(server: &Server, report: &QueryReport) -> Result<Vec<TypeHierarchyItem>> {
    let QueryOutput::Hierarchy(hierarchy) = &report.output else {
        return Err(Error::internal_error());
    };

    hierarchy
        .types
        .iter()
        .filter(|nominal| hierarchy.selected.contains(&nominal.identity))
        .map(|nominal| type_item(server, report, nominal))
        .collect()
}

fn call_item(
    server: &Server,
    report: &QueryReport,
    function: &FunctionFact,
) -> Result<CallHierarchyItem> {
    let location = server.location(&report.snapshot, function.location)?;

    let source = report
        .snapshot
        .source(function.selection.source.identity)
        .ok_or_else(Error::content_modified)?;

    function
        .selection
        .validate(source)
        .map_err(|error| failure(&error))?;

    let name = source.text[function.selection.span.start..function.selection.span.end].to_owned();

    let mut item = CallHierarchyItem {
        name,
        kind: SymbolKind::Function,
        tags: None,
        detail: None,
        uri: location.uri,
        range: location.range,
        selection_range: server.range(&report.snapshot, function.selection)?,
        data: None,
    };

    item.data = Some(retain(
        server,
        report,
        Some(function.location.source),
        RetainedResult::CallHierarchy {
            item: item.clone(),
            identity: function.identity.clone(),
        },
    )?);

    Ok(item)
}

fn type_item(
    server: &Server,
    report: &QueryReport,
    nominal: &NominalFact,
) -> Result<TypeHierarchyItem> {
    let location = server.location(&report.snapshot, nominal.declaration)?;

    let mut item = TypeHierarchyItem {
        name: nominal.name.clone(),
        kind: SymbolKind::Class,
        tags: nominal.deprecated.then_some(vec![SymbolTag::Deprecated]),
        detail: None,
        uri: location.uri,
        range: location.range,
        selection_range: server.range(&report.snapshot, nominal.selection)?,
        data: None,
    };

    item.data = Some(retain(
        server,
        report,
        Some(nominal.declaration.source),
        RetainedResult::TypeHierarchy {
            item: item.clone(),
            identity: nominal.identity.clone(),
        },
    )?);

    Ok(item)
}
