use std::{
    collections::BTreeSet,
    ops::{ControlFlow, Range},
    path::Path,
};

use instar_bridge::{Frontend, Occurrence, OccurrenceRole, SourceLocation};
use instar_core::analysis::{BindingKind, Symbol, SymbolKind as SourceSymbolKind};

use tower_lsp_server::{
    gen_lsp_types::{
        BaseSymbolInformation, Definition, DefinitionLink, DefinitionParams, DefinitionResponse,
        DocumentHighlight, DocumentHighlightKind, DocumentHighlightParams, DocumentLink,
        DocumentLinkParams, DocumentSymbol, DocumentSymbolParams, DocumentSymbolResponse, Location,
        Range as ProtocolRange, ReferenceParams, SymbolInformation, SymbolKind,
        TextDocumentPositionParams, TypeDefinitionParams, TypeDefinitionResponse, Uri,
        WorkspaceSymbolParams,
    },
    jsonrpc::Result,
};

use super::{
    document::{self, Document, Encoding},
    error::failure,
    project::Project,
};

pub(super) fn goto_definition(
    project: &mut Project,
    frontend: &mut Frontend<'_>,
    parameters: &DefinitionParams,
) -> Result<Option<DefinitionResponse>> {
    let links = definitions(
        project,
        frontend,
        &parameters.text_document_position_params,
        false,
    )?;

    Ok(Some(
        if project.has_capability("/textDocument/definition/linkSupport") {
            DefinitionResponse::DefinitionLinkList(links)
        } else {
            DefinitionResponse::Definition(Definition::LocationList(locations(links)))
        },
    ))
}

pub(super) fn goto_type_definition(
    project: &mut Project,
    frontend: &mut Frontend<'_>,
    parameters: &TypeDefinitionParams,
) -> Result<Option<TypeDefinitionResponse>> {
    let links = definitions(
        project,
        frontend,
        &parameters.text_document_position_params,
        true,
    )?;

    Ok(Some(
        if project.has_capability("/textDocument/typeDefinition/linkSupport") {
            TypeDefinitionResponse::DefinitionLinkList(links)
        } else {
            TypeDefinitionResponse::Definition(Definition::LocationList(locations(links)))
        },
    ))
}

pub(super) fn references_source(
    project: &mut Project,
    parameters: &ReferenceParams,
) -> Result<ControlFlow<Option<Vec<Location>>>> {
    let position = &parameters.text_document_position_params;
    let module = project.module(&position.text_document.uri)?;
    let document = project.snapshot(&module)?;
    let offset = document.offset(position.position, project.encoding)?;

    if document.analysis.prepare_rename(offset).is_none() {
        return Ok(ControlFlow::Continue(()));
    }

    let Some(binding) = document.analysis.bindings.iter().find(|binding| {
        matches!(
            binding.kind,
            BindingKind::Local | BindingKind::Parameter | BindingKind::Loop
        ) && binding
            .occurrences
            .iter()
            .any(|occurrence| occurrence.range.contains(&offset))
    }) else {
        return Ok(ControlFlow::Continue(()));
    };

    binding
        .occurrences
        .iter()
        .filter(|occurrence| {
            parameters.context.include_declaration || occurrence.range != binding.declaration
        })
        .map(|occurrence| {
            Ok(Location {
                uri: document.uri.clone(),
                range: document.range(occurrence.range.clone(), project.encoding)?,
            })
        })
        .collect::<Result<Vec<_>>>()
        .map(|locations| ControlFlow::Break(Some(locations)))
}

pub(super) fn references(
    project: &mut Project,
    frontend: &mut Frontend<'_>,
    parameters: ReferenceParams,
) -> Result<Option<Vec<Location>>> {
    let position = parameters.text_document_position_params;
    let module = project.module(&position.text_document.uri)?;
    let document = project.snapshot(&module)?;
    let offset = document.offset(position.position, project.encoding)?;
    let (line, column) = document.native(position.position, project.encoding)?;
    let occurrences = frontend.occurrences(&module).map_err(failure)?;
    let declarations = declarations(&occurrences, &module, line, column);
    let mut result = Vec::new();

    if declarations.is_empty() {
        if let Some(binding) = document.analysis.bindings.iter().find(|binding| {
            binding
                .occurrences
                .iter()
                .any(|occurrence| is_contained(&occurrence.range, offset))
        }) {
            for occurrence in &binding.occurrences {
                if parameters.context.include_declaration || occurrence.range != binding.declaration
                {
                    result.push(Location {
                        uri: document.uri.clone(),
                        range: document.range(occurrence.range.clone(), project.encoding)?,
                    });
                }
            }
        }
    } else {
        if parameters.context.include_declaration {
            for (declaration, _) in &declarations {
                let location = location(project, declaration)?;

                if !result.contains(&location) {
                    result.push(location);
                }
            }
        }

        let mut modules = project.index()?;

        if !modules.contains(&module) {
            modules.push(module);
        }

        for module in modules {
            for occurrence in frontend.occurrences(&module).map_err(failure)? {
                if declarations.iter().any(|(declaration, is_type_namespace)| {
                    declaration == &occurrence.declaration
                        && *is_type_namespace == occurrence.type_namespace
                }) && (parameters.context.include_declaration
                    || occurrence.role != OccurrenceRole::Declaration)
                {
                    let location = location(project, &occurrence.location)?;

                    if !result.contains(&location) {
                        result.push(location);
                    }
                }
            }
        }
    }

    Ok(Some(result))
}

pub(super) fn document_highlight_source(
    project: &mut Project,
    parameters: &DocumentHighlightParams,
) -> Result<ControlFlow<Option<Vec<DocumentHighlight>>>> {
    let position = &parameters.text_document_position_params;
    let module = project.module(&position.text_document.uri)?;
    let document = project.snapshot(&module)?;
    let offset = document.offset(position.position, project.encoding)?;

    if let Some(binding) = document.analysis.bindings.iter().find(|binding| {
        binding
            .occurrences
            .iter()
            .any(|occurrence| is_contained(&occurrence.range, offset))
    }) {
        return binding
            .occurrences
            .iter()
            .map(|occurrence| {
                Ok(DocumentHighlight {
                    range: document.range(occurrence.range.clone(), project.encoding)?,
                    kind: Some(if occurrence.access.is_write() {
                        DocumentHighlightKind::Write
                    } else {
                        DocumentHighlightKind::Read
                    }),
                })
            })
            .collect::<Result<Vec<_>>>()
            .map(|highlights| ControlFlow::Break(Some(highlights)));
    }

    Ok(ControlFlow::Continue(()))
}

pub(super) fn document_highlight(
    project: &mut Project,
    frontend: &mut Frontend<'_>,
    parameters: DocumentHighlightParams,
) -> Result<Option<Vec<DocumentHighlight>>> {
    let position = parameters.text_document_position_params;
    let module = project.module(&position.text_document.uri)?;
    let document = project.snapshot(&module)?;

    let (line, column) = document.native(position.position, project.encoding)?;
    let occurrences = frontend.occurrences(&module).map_err(failure)?;
    let declarations = declarations(&occurrences, &module, line, column);
    let mut result = Vec::new();

    for occurrence in occurrences {
        if occurrence.location.module != module
            || !declarations.iter().any(|(declaration, is_type_namespace)| {
                declaration == &occurrence.declaration
                    && *is_type_namespace == occurrence.type_namespace
            })
        {
            continue;
        }

        let highlight = DocumentHighlight {
            range: document.native_range(occurrence.location.range, project.encoding)?,
            kind: Some(match occurrence.role {
                OccurrenceRole::Declaration | OccurrenceRole::Write | OccurrenceRole::ReadWrite => {
                    DocumentHighlightKind::Write
                }

                OccurrenceRole::Read | OccurrenceRole::Type => DocumentHighlightKind::Read,
                _ => DocumentHighlightKind::Text,
            }),
        };

        if !result.contains(&highlight) {
            result.push(highlight);
        }
    }

    Ok(Some(result))
}

pub(super) fn document_symbol(
    project: &mut Project,
    parameters: &DocumentSymbolParams,
) -> Result<Option<DocumentSymbolResponse>> {
    let module = project.module(&parameters.text_document.uri)?;
    let document = project.snapshot(&module)?;

    Ok(Some(
        if project.has_capability("/textDocument/documentSymbol/hierarchicalDocumentSymbolSupport")
        {
            DocumentSymbolResponse::DocumentSymbolList(
                document
                    .analysis
                    .symbols
                    .iter()
                    .map(|symbol| outline(&document, symbol, project.encoding))
                    .collect::<Result<_>>()?,
            )
        } else {
            let mut result = Vec::new();

            flatten(
                &document,
                &document.analysis.symbols,
                None,
                project.encoding,
                &mut result,
            )?;

            DocumentSymbolResponse::SymbolInformationList(result)
        },
    ))
}

pub(super) fn symbol(
    project: &mut Project,
    parameters: &WorkspaceSymbolParams,
) -> Result<Vec<SymbolInformation>> {
    let query = parameters.query.to_lowercase();
    let mut information = Vec::new();

    for module in project.index()? {
        let document = project.snapshot(&module)?;
        let mut entries = Vec::new();
        indexed(&document.analysis.symbols, None, &mut entries);

        for (symbol, container) in entries {
            if !is_match(&symbol.name, &query) {
                continue;
            }

            let base = BaseSymbolInformation {
                name: symbol.name.clone(),
                kind: kind(symbol.kind),
                tags: None,
                container_name: container,
            };

            information.push(SymbolInformation::new(
                None,
                Location {
                    uri: document.uri.clone(),
                    range: document.range(symbol.range.clone(), project.encoding)?,
                },
                base,
            ));
        }
    }

    Ok(information)
}

pub(super) fn document_link(
    project: &mut Project,
    frontend: &mut Frontend<'_>,
    parameters: &DocumentLinkParams,
) -> Result<Option<Vec<DocumentLink>>> {
    let module = project.module(&parameters.text_document.uri)?;

    Ok(Some(
        links(project, frontend, &module)?
            .into_iter()
            .map(|(range, target)| DocumentLink {
                range,
                target: Some(target),
                tooltip: None,
                data: None,
            })
            .collect(),
    ))
}

fn definitions(
    project: &mut Project,
    frontend: &mut Frontend<'_>,
    position: &TextDocumentPositionParams,
    should_find_types: bool,
) -> Result<Vec<DefinitionLink>> {
    let module = project.module(&position.text_document.uri)?;
    let document = project.snapshot(&module)?;
    let offset = document.offset(position.position, project.encoding)?;
    let (line, column) = document.native(position.position, project.encoding)?;

    let origin = document
        .analysis
        .tokens
        .iter()
        .find(|token| token.range.contains(&offset))
        .or_else(|| {
            document
                .analysis
                .tokens
                .iter()
                .find(|token| token.range.end == offset)
        })
        .map(|token| document.range(token.range.clone(), project.encoding))
        .transpose()?;

    let mut result = Vec::new();

    for declaration in frontend
        .definitions(&module, line, column, should_find_types)
        .map_err(failure)?
    {
        let target = project.snapshot(&declaration.module)?;
        let bytes = target.native_bytes(declaration.range)?;
        let selection = target.range(bytes.clone(), project.encoding)?;

        let range = enclosing(&target.analysis.symbols, &bytes)
            .map(|symbol| target.range(symbol.range.clone(), project.encoding))
            .transpose()?
            .unwrap_or(selection);

        let definition = DefinitionLink {
            origin_selection_range: origin,
            target_uri: target.uri,
            target_range: range,
            target_selection_range: selection,
        };

        if !result.contains(&definition) {
            result.push(definition);
        }
    }

    if result.is_empty()
        && !should_find_types
        && let Some(binding) = document.analysis.bindings.iter().find(|binding| {
            binding
                .occurrences
                .iter()
                .any(|occurrence| is_contained(&occurrence.range, offset))
        })
    {
        let selection = document.range(binding.declaration.clone(), project.encoding)?;

        let range = enclosing(&document.analysis.symbols, &binding.declaration)
            .map(|symbol| document.range(symbol.range.clone(), project.encoding))
            .transpose()?
            .unwrap_or(selection);

        result.push(DefinitionLink {
            origin_selection_range: origin,
            target_uri: document.uri.clone(),
            target_range: range,
            target_selection_range: selection,
        });
    }

    Ok(result)
}

fn locations(links: Vec<DefinitionLink>) -> Vec<Location> {
    links
        .into_iter()
        .map(|link| Location {
            uri: link.target_uri,
            range: link.target_selection_range,
        })
        .collect()
}

fn location(project: &mut Project, source: &SourceLocation) -> Result<Location> {
    let document = project.snapshot(&source.module)?;

    Ok(Location {
        uri: document.uri.clone(),
        range: document.native_range(source.range, project.encoding)?,
    })
}

fn declarations(
    occurrences: &[Occurrence],
    module: &str,
    line: u32,
    column: u32,
) -> Vec<(SourceLocation, bool)> {
    occurrences
        .iter()
        .filter(|occurrence| {
            let range = occurrence.location.range;

            occurrence.location.module == module
                && (range.start.line, range.start.column) <= (line, column)
                && (line, column) <= (range.end.line, range.end.column)
        })
        .map(|occurrence| (occurrence.declaration.clone(), occurrence.type_namespace))
        .collect()
}

fn is_contained(range: &Range<usize>, offset: usize) -> bool {
    range.start <= offset && offset <= range.end
}

fn enclosing<'source>(symbols: &'source [Symbol], range: &Range<usize>) -> Option<&'source Symbol> {
    symbols.iter().find_map(|symbol| {
        if symbol.range.start <= range.start && range.end <= symbol.range.end {
            enclosing(&symbol.children, range).or(Some(symbol))
        } else {
            None
        }
    })
}

fn outline(document: &Document, symbol: &Symbol, encoding: Encoding) -> Result<DocumentSymbol> {
    let children = symbol
        .children
        .iter()
        .map(|child| outline(document, child, encoding))
        .collect::<Result<Vec<_>>>()?;

    Ok(DocumentSymbol::new(
        symbol.name.clone(),
        None,
        kind(symbol.kind),
        None,
        None,
        document.range(symbol.range.clone(), encoding)?,
        document.range(symbol.selection.clone(), encoding)?,
        (!children.is_empty()).then_some(children),
    ))
}

fn flatten(
    document: &Document,
    symbols: &[Symbol],
    container: Option<&str>,
    encoding: Encoding,
    result: &mut Vec<SymbolInformation>,
) -> Result<()> {
    for symbol in symbols {
        result.push(SymbolInformation::new(
            None,
            Location {
                uri: document.uri.clone(),
                range: document.range(symbol.range.clone(), encoding)?,
            },
            BaseSymbolInformation {
                name: symbol.name.clone(),
                kind: kind(symbol.kind),
                tags: None,
                container_name: container.map(str::to_owned),
            },
        ));

        flatten(
            document,
            &symbol.children,
            Some(&symbol.name),
            encoding,
            result,
        )?;
    }

    Ok(())
}

fn indexed<'source>(
    symbols: &'source [Symbol],
    container: Option<&str>,
    result: &mut Vec<(&'source Symbol, Option<String>)>,
) {
    for symbol in symbols {
        indexed(&symbol.children, Some(&symbol.name), result);
        result.push((symbol, container.map(str::to_owned)));
    }
}

fn kind(kind: SourceSymbolKind) -> SymbolKind {
    match kind {
        SourceSymbolKind::Function => SymbolKind::Function,
        SourceSymbolKind::Method => SymbolKind::Method,
        SourceSymbolKind::Variable => SymbolKind::Variable,
        SourceSymbolKind::Constant => SymbolKind::Constant,
        SourceSymbolKind::Type => SymbolKind::Interface,
        SourceSymbolKind::Class => SymbolKind::Class,
        SourceSymbolKind::Property => SymbolKind::Property,
    }
}

fn is_match(name: &str, query: &str) -> bool {
    let name = name.to_lowercase();
    let mut characters = name.chars();

    query
        .chars()
        .all(|expected| characters.by_ref().any(|character| character == expected))
}

fn links(
    project: &mut Project,
    frontend: &mut Frontend<'_>,
    module: &str,
) -> Result<Vec<(ProtocolRange, Uri)>> {
    let document = project.snapshot(module)?;
    let mut result = Vec::new();
    let mut seen = BTreeSet::new();

    for import in frontend.imports(module).map_err(failure)? {
        let range = document.native_range(import.location.range, project.encoding)?;
        let target = document::uri(Path::new(&import.target))?;

        let identity = (
            range.start.line,
            range.start.character,
            range.end.line,
            range.end.character,
            target.to_string(),
        );

        if seen.insert(identity) {
            result.push((range, target));
        }
    }

    for link in &document.analysis.links {
        result.push((
            document.range(link.range.clone(), project.encoding)?,
            link.target.parse().map_err(failure)?,
        ));
    }

    Ok(result)
}
