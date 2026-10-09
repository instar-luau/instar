use std::{io, time::Instant};

use instar_analysis::Options;
use instar_core::symbols::{Kind, Symbol};

use tower_lsp_server::{
    gen_lsp_types::{
        BaseSymbolInformation, DocumentSymbol, DocumentSymbolParams, DocumentSymbolResponse,
        Location, PositionEncodingKind, SymbolInformation, SymbolKind, Uri, WorkspaceSymbolParams,
    },
    jsonrpc::Result,
};

use crate::{location, position, session::Session};

impl Session {
    pub(crate) async fn symbols(
        &self,
        parameters: DocumentSymbolParams,
    ) -> Result<DocumentSymbolResponse> {
        let capabilities = self
            .workspace
            .lock()
            .expect("workspace lock")
            .symbol_capabilities
            .clone();

        let uri = parameters.text_document.uri.clone();

        self.query(
            &parameters.text_document.uri,
            &self.workers,
            move |path, encoding, project, options, started| {
                let source = project.source(path)?;
                let symbols = project.symbols(path, &options.remaining(started))?;

                let kinds = capabilities
                    .symbol_kind
                    .as_ref()
                    .and_then(|kinds| kinds.value_set.as_deref());

                let symbols = render(symbols, &source.text, encoding, kinds, options, started)?;

                if capabilities
                    .hierarchical_document_symbol_support
                    .unwrap_or(false)
                {
                    return Ok(DocumentSymbolResponse::DocumentSymbolList(symbols));
                }

                let mut information = Vec::new();
                flatten(symbols, &uri, None, &mut information, options, started)?;

                Ok(DocumentSymbolResponse::SymbolInformationList(information))
            },
        )
        .await
    }

    pub(crate) async fn search_symbols(
        &self,
        parameters: WorkspaceSymbolParams,
    ) -> Result<Vec<SymbolInformation>> {
        let (capabilities, locations) = {
            let workspace = self.workspace.lock().expect("workspace lock");

            (workspace.search_capabilities.clone(), workspace.locations())
        };

        self.workspace_query(
            &self.workers,
            move |roots, encoding, project, options, started| {
                let matches = project.search_symbols(
                    roots,
                    &parameters.query,
                    &options.remaining(started),
                )?;

                let kinds = capabilities
                    .symbol_kind
                    .as_ref()
                    .and_then(|kinds| kinds.value_set.as_deref());

                let mut information = Vec::new();

                for symbol in matches {
                    options.check(started)?;
                    let source = project.source(&symbol.location.module)?;

                    if source.revision != symbol.location.revision {
                        return Err(io::Error::other("symbol source revision mismatch"));
                    }

                    let uri = locations
                        .get(&symbol.location.module)
                        .cloned()
                        .map_or_else(|| location::uri(&symbol.location.module), Ok)?;

                    information.push(SymbolInformation::new(
                        None,
                        Location {
                            uri,
                            range: position::range(&source.text, symbol.location.range, encoding)?,
                        },
                        BaseSymbolInformation::new(
                            symbol.name,
                            kind(symbol.kind, kinds),
                            None,
                            symbol.container,
                        ),
                    ));
                }

                options.check(started)?;

                Ok(information)
            },
        )
        .await
    }
}

fn render(
    symbols: Vec<Symbol>,
    source: &str,
    encoding: &PositionEncodingKind,
    kinds: Option<&[SymbolKind]>,
    options: &Options,
    started: Instant,
) -> io::Result<Vec<DocumentSymbol>> {
    symbols
        .into_iter()
        .map(|symbol| {
            options.check(started)?;

            if symbol.range[0] > symbol.selection[0] || symbol.selection[1] > symbol.range[1] {
                return Err(io::Error::other(
                    "symbol selection is outside its declaration",
                ));
            }

            let range = position::range(source, symbol.range, encoding)?;
            let selection = position::range(source, symbol.selection, encoding)?;
            let children = render(symbol.children, source, encoding, kinds, options, started)?;

            let kind = kind(symbol.kind, kinds);

            Ok(DocumentSymbol::new(
                symbol.name,
                None,
                kind,
                None,
                None,
                range,
                selection,
                (!children.is_empty()).then_some(children),
            ))
        })
        .collect()
}

fn flatten(
    symbols: Vec<DocumentSymbol>,
    uri: &Uri,
    parent: Option<&str>,
    information: &mut Vec<SymbolInformation>,
    options: &Options,
    started: Instant,
) -> io::Result<()> {
    for symbol in symbols {
        options.check(started)?;

        information.push(SymbolInformation::new(
            None,
            Location {
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
            information,
            options,
            started,
        )?;
    }

    Ok(())
}

fn kind(kind: Kind, supported: Option<&[SymbolKind]>) -> SymbolKind {
    let kind = match kind {
        Kind::Variable | Kind::Object => SymbolKind::Variable,
        Kind::Constant => SymbolKind::Constant,
        Kind::Function => SymbolKind::Function,
        Kind::Method => SymbolKind::Method,
        Kind::Type => SymbolKind::Interface,
        Kind::Class => SymbolKind::Class,
        Kind::Field => SymbolKind::Field,
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
