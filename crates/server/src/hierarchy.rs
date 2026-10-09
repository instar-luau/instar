use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    future::{Ready, ready},
    io,
    path::{Path, PathBuf},
    sync::Arc,
    time::Instant,
};

use instar_analysis::Options;
use instar_bridge::frontend::{HierarchyKind, Inheritance};
use instar_core::{analysis::Origin, project::Project, snapshot::Snapshot};
use serde_json::{Value, json};

use tower_lsp_server::{
    gen_lsp_types::{
        CallHierarchyIncomingCall, CallHierarchyIncomingCallsParams, CallHierarchyItem,
        CallHierarchyOutgoingCall, CallHierarchyOutgoingCallsParams, Location, Position,
        PositionEncodingKind, Range, SymbolKind, TextDocumentPositionParams, TypeHierarchyItem,
        TypeHierarchySubtypesParams, TypeHierarchySupertypesParams,
    },
    jsonrpc::{Error, Result},
};

use crate::{
    location, position,
    session::Session,
    workspace::{self, Workspace},
};

const RETAINED_GRAPHS: usize = 32;

#[derive(Default)]
pub(crate) struct Hierarchies {
    generation: u64,
    records: VecDeque<Record>,
}

impl Hierarchies {
    fn record<'records>(
        &'records self,
        item: &CallHierarchyItem,
        workspace: &Workspace,
    ) -> Result<(&'records Graph, usize)> {
        let data = item
            .data
            .as_ref()
            .ok_or_else(|| Error::invalid_params("missing hierarchy identity"))?;

        let generation = data
            .get("hierarchy")
            .and_then(Value::as_u64)
            .ok_or_else(|| Error::invalid_params("invalid hierarchy identity"))?;

        let index = data
            .get("item")
            .and_then(Value::as_u64)
            .and_then(|value| usize::try_from(value).ok())
            .ok_or_else(|| Error::invalid_params("invalid hierarchy item"))?;

        let record = self
            .records
            .iter()
            .find(|record| record.generation == generation)
            .ok_or_else(Error::content_modified)?;

        if workspace.snapshot.revision() != record.snapshot.revision()
            || !workspace.current(&record.snapshot, &record.inputs)
        {
            return Err(Error::content_modified());
        }

        if record
            .graph
            .items
            .get(index)
            .is_none_or(|candidate| &candidate.item != item)
        {
            return Err(Error::invalid_params("hierarchy item does not match"));
        }

        Ok((&record.graph, index))
    }
}

struct Record {
    generation: u64,
    snapshot: Snapshot,
    inputs: BTreeSet<PathBuf>,
    graph: Graph,
}

#[derive(Default)]
struct Graph {
    items: Vec<Item>,
    calls: Vec<Call>,
    inheritance: Vec<Inheritance>,
    selected: BTreeSet<usize>,
}

impl Graph {
    fn relatives(&self, index: usize, parents: bool) -> BTreeSet<usize> {
        self.inheritance
            .iter()
            .filter_map(|relation| {
                if parents {
                    (relation.child == index).then_some(relation.parent)
                } else {
                    (relation.parent == index).then_some(relation.child)
                }
            })
            .collect()
    }
}

struct Item {
    item: CallHierarchyItem,
    content: Option<Arc<str>>,
}

#[derive(PartialEq)]
struct Call {
    caller: usize,
    callee: usize,
    range: Range,
}

impl Session {
    pub(crate) async fn prepare_calls(
        &self,
        parameters: TextDocumentPositionParams,
    ) -> Result<Vec<CallHierarchyItem>> {
        self.prepare_hierarchy(parameters, SymbolKind::Function)
            .await
    }

    pub(crate) async fn prepare_types(
        &self,
        parameters: TextDocumentPositionParams,
    ) -> Result<Vec<TypeHierarchyItem>> {
        Ok(self
            .prepare_hierarchy(parameters, SymbolKind::Class)
            .await?
            .into_iter()
            .map(type_item)
            .collect())
    }

    pub(crate) fn incoming_calls(
        &self,
        CallHierarchyIncomingCallsParams { item, .. }: CallHierarchyIncomingCallsParams,
    ) -> Ready<Result<Vec<CallHierarchyIncomingCall>>> {
        let workspace = self.workspace.lock().expect("workspace lock");

        ready(
            workspace
                .hierarchies
                .record(&item, &workspace)
                .map(|(graph, index)| {
                    let mut callers: BTreeMap<usize, Vec<Range>> = BTreeMap::new();

                    for call in &graph.calls {
                        if call.callee == index {
                            callers.entry(call.caller).or_default().push(call.range);
                        }
                    }

                    callers
                        .into_iter()
                        .map(|(caller, from_ranges)| CallHierarchyIncomingCall {
                            from: graph.items[caller].item.clone(),
                            from_ranges,
                        })
                        .collect()
                }),
        )
    }

    pub(crate) fn outgoing_calls(
        &self,
        CallHierarchyOutgoingCallsParams { item, .. }: CallHierarchyOutgoingCallsParams,
    ) -> Ready<Result<Vec<CallHierarchyOutgoingCall>>> {
        let workspace = self.workspace.lock().expect("workspace lock");

        ready(
            workspace
                .hierarchies
                .record(&item, &workspace)
                .map(|(graph, index)| {
                    let mut callees: BTreeMap<usize, Vec<Range>> = BTreeMap::new();

                    for call in &graph.calls {
                        if call.caller == index {
                            callees.entry(call.callee).or_default().push(call.range);
                        }
                    }

                    callees
                        .into_iter()
                        .map(|(callee, from_ranges)| CallHierarchyOutgoingCall {
                            to: graph.items[callee].item.clone(),
                            from_ranges,
                        })
                        .collect()
                }),
        )
    }

    pub(crate) fn supertypes(
        &self,
        parameters: TypeHierarchySupertypesParams,
    ) -> Ready<Result<Vec<TypeHierarchyItem>>> {
        ready(self.related_types(parameters.item, true))
    }

    pub(crate) fn subtypes(
        &self,
        parameters: TypeHierarchySubtypesParams,
    ) -> Ready<Result<Vec<TypeHierarchyItem>>> {
        ready(self.related_types(parameters.item, false))
    }

    pub(crate) async fn implementations(
        &self,
        parameters: TextDocumentPositionParams,
    ) -> Result<Vec<Location>> {
        let (graph, _, _) = self.hierarchy(parameters).await?;
        let mut selected = graph.selected.clone();
        let mut pending = selected.iter().copied().collect::<Vec<_>>();

        while let Some(index) = pending.pop() {
            for child in graph.relatives(index, false) {
                if selected.insert(child) {
                    pending.push(child);
                }
            }
        }

        Ok(selected
            .into_iter()
            .filter(|index| {
                graph.items[*index].item.kind == SymbolKind::Function
                    || !graph.selected.contains(index)
            })
            .map(|index| Location {
                uri: graph.items[index].item.uri.clone(),
                range: graph.items[index].item.selection_range,
            })
            .collect())
    }

    async fn prepare_hierarchy(
        &self,
        parameters: TextDocumentPositionParams,
        kind: SymbolKind,
    ) -> Result<Vec<CallHierarchyItem>> {
        let (mut graph, snapshot, inputs) = self.hierarchy(parameters).await?;
        let mut workspace = self.workspace.lock().expect("workspace lock");

        if workspace.snapshot.revision() != snapshot.revision()
            || !workspace.current(&snapshot, &inputs)
        {
            return Err(Error::content_modified());
        }

        let hierarchies = &mut workspace.hierarchies;

        let generation = hierarchies
            .generation
            .checked_add(1)
            .ok_or_else(Error::internal_error)?;

        hierarchies.generation = generation;

        for (index, item) in graph.items.iter_mut().enumerate() {
            item.item.data = Some(json!({"hierarchy": generation, "item": index}));
        }

        let result = graph
            .selected
            .iter()
            .filter_map(|&index| {
                let item = &graph.items[index].item;

                (item.kind == kind).then(|| item.clone())
            })
            .collect();

        hierarchies.records.push_back(Record {
            generation,
            snapshot,
            inputs,
            graph,
        });

        while hierarchies.records.len() > RETAINED_GRAPHS {
            hierarchies.records.pop_front();
        }

        Ok(result)
    }

    async fn hierarchy(
        &self,
        parameters: TextDocumentPositionParams,
    ) -> Result<(Graph, Snapshot, BTreeSet<PathBuf>)> {
        let (snapshot, roots) = {
            let workspace = self.workspace.lock().expect("workspace lock");

            (workspace.snapshot.clone(), workspace.index_roots())
        };

        let (mut graph, inputs) = self
            .query(
                &parameters.text_document.uri,
                &self.queries,
                move |path, encoding, project, options, started| {
                    let graph = query(
                        path,
                        parameters.position,
                        encoding,
                        &roots,
                        project,
                        options,
                        started,
                    )?;

                    Ok((graph, project.watch_inputs()))
                },
            )
            .await?;

        let mut workspace = self.workspace.lock().expect("workspace lock");

        if workspace.snapshot.revision() != snapshot.revision()
            || !workspace.current(&snapshot, &inputs)
        {
            return Err(Error::content_modified());
        }

        let locations = workspace.locations();

        for item in &mut graph.items {
            if let Some(content) = item.content.take() {
                item.item.uri = workspace.definition(item.item.uri.clone(), content)?;
            } else if let Some(uri) = locations.get(&workspace::path(&item.item.uri)?) {
                item.item.uri = uri.clone();
            }
        }

        Ok((graph, snapshot, inputs))
    }

    fn related_types(
        &self,
        item: TypeHierarchyItem,
        parents: bool,
    ) -> Result<Vec<TypeHierarchyItem>> {
        let workspace = self.workspace.lock().expect("workspace lock");
        let item = call_item(item);
        let (graph, index) = workspace.hierarchies.record(&item, &workspace)?;

        Ok(graph
            .relatives(index, parents)
            .into_iter()
            .map(|index| type_item(graph.items[index].item.clone()))
            .collect())
    }
}

fn query(
    path: &Path,
    point: Position,
    encoding: &PositionEncodingKind,
    roots: &[PathBuf],
    project: &mut Project,
    options: &Options,
    started: Instant,
) -> io::Result<Graph> {
    let source = project.source(path)?;

    let offset = position::offset(&source.text, point, encoding)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error.to_string()))?;

    let entries = project.entries(&[path.to_path_buf()], &options.remaining(started))?;
    let mut output = Graph::default();

    for entry in entries {
        for graph in project.hierarchy(&entry, offset, roots, &options.remaining(started))? {
            let mut indices = Vec::new();

            for item in graph.items {
                options.check(started)?;
                let location = location::convert(&item.location, project, encoding)?;
                let document = project.source(item.location.module.source())?;

                let content = matches!(item.location.module, Origin::Definition(_))
                    .then_some(Arc::clone(&document.text));

                let item = CallHierarchyItem {
                    name: item.name,
                    kind: match item.kind {
                        HierarchyKind::Function => SymbolKind::Function,
                        HierarchyKind::Class => SymbolKind::Class,
                        HierarchyKind::Module => SymbolKind::Module,
                    },
                    tags: None,
                    detail: None,
                    uri: location.uri,
                    range: location.range,
                    selection_range: position::range(&document.text, item.selection, encoding)?,
                    data: None,
                };

                let index = output
                    .items
                    .iter()
                    .position(|candidate| candidate.item == item)
                    .unwrap_or_else(|| {
                        let index = output.items.len();
                        output.items.push(Item { item, content });

                        index
                    });

                indices.push(index);
            }

            output
                .selected
                .extend(graph.selected.into_iter().map(|index| indices[index]));

            for call in graph.calls {
                options.check(started)?;

                let call = Call {
                    caller: indices[call.caller],
                    callee: indices[call.callee],
                    range: location::convert(&call.location, project, encoding)?.range,
                };

                if !output.calls.contains(&call) {
                    output.calls.push(call);
                }
            }

            for relation in graph.inheritance {
                let relation = Inheritance {
                    child: indices[relation.child],
                    parent: indices[relation.parent],
                };

                if !output.inheritance.contains(&relation) {
                    output.inheritance.push(relation);
                }
            }
        }
    }

    options.check(started)?;

    Ok(output)
}

fn type_item(item: CallHierarchyItem) -> TypeHierarchyItem {
    TypeHierarchyItem {
        name: item.name,
        kind: item.kind,
        tags: item.tags,
        detail: item.detail,
        uri: item.uri,
        range: item.range,
        selection_range: item.selection_range,
        data: item.data,
    }
}

fn call_item(item: TypeHierarchyItem) -> CallHierarchyItem {
    CallHierarchyItem {
        name: item.name,
        kind: item.kind,
        tags: item.tags,
        detail: item.detail,
        uri: item.uri,
        range: item.range,
        selection_range: item.selection_range,
        data: item.data,
    }
}
