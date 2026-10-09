use std::{
    collections::{BTreeSet, VecDeque},
    io,
    path::PathBuf,
    time::Instant,
};

use instar_analysis::{Options, error::invalid};

use instar_core::{
    snapshot::Snapshot,
    tokens::{Kind, Modifier, Token},
};

use tower_lsp_server::{
    gen_lsp_types::{
        ClientSemanticTokensRequestOptionsFull, ClientSemanticTokensRequestOptionsRange, Full,
        Position, PositionEncodingKind, Range, SemanticToken, SemanticTokens,
        SemanticTokensClientCapabilities, SemanticTokensDelta, SemanticTokensDeltaParams,
        SemanticTokensDeltaResponse, SemanticTokensEdit, SemanticTokensFullDelta,
        SemanticTokensLegend, SemanticTokensOptions, SemanticTokensParams,
        SemanticTokensRangeParams, TokenFormat, Uri,
    },
    jsonrpc::{Error, Result},
};

use crate::{
    position,
    session::{Session, failure},
};

const RETAINED_LISTS: usize = 32;

const KINDS: &[(Kind, &str)] = &[
    (Kind::Comment, "comment"),
    (Kind::Keyword, "keyword"),
    (Kind::String, "string"),
    (Kind::Number, "number"),
    (Kind::Operator, "operator"),
    (Kind::Decorator, "decorator"),
    (Kind::Type, "type"),
    (Kind::TypeParameter, "typeParameter"),
    (Kind::Variable, "variable"),
    (Kind::Parameter, "parameter"),
    (Kind::Function, "function"),
    (Kind::Property, "property"),
    (Kind::Method, "method"),
];

const MODIFIERS: &[(Modifier, &str)] = &[
    (Modifier::Declaration, "declaration"),
    (Modifier::Modification, "modification"),
    (Modifier::DefaultLibrary, "defaultLibrary"),
    (Modifier::Deprecated, "deprecated"),
];

#[derive(Default)]
pub(crate) struct Tokens {
    pub(crate) capabilities: Option<SemanticTokensClientCapabilities>,
    generation: u64,
    records: VecDeque<Record>,
}

impl Tokens {
    pub(crate) fn provider(&self) -> Option<SemanticTokensOptions> {
        let capabilities = self.capabilities.as_ref()?;

        if !capabilities.formats.contains(&TokenFormat::Relative) {
            return None;
        }

        let legend = SemanticTokensLegend {
            token_types: KINDS
                .iter()
                .filter(|(_, name)| {
                    capabilities
                        .token_types
                        .iter()
                        .any(|supported| supported.as_str() == *name)
                })
                .map(|(_, name)| (*name).to_owned())
                .collect(),
            token_modifiers: MODIFIERS
                .iter()
                .filter(|(_, name)| {
                    capabilities
                        .token_modifiers
                        .iter()
                        .any(|supported| supported.as_str() == *name)
                })
                .map(|(_, name)| (*name).to_owned())
                .collect(),
        };

        if legend.token_types.is_empty() {
            return None;
        }

        let full = match capabilities.requests.full {
            Some(ClientSemanticTokensRequestOptionsFull::ClientSemanticTokensRequestFullDelta(
                options,
            )) if options.delta == Some(true) => {
                Some(SemanticTokensFullDelta { delta: Some(true) }.into())
            }

            Some(
                ClientSemanticTokensRequestOptionsFull::Bool(true)
                | ClientSemanticTokensRequestOptionsFull::ClientSemanticTokensRequestFullDelta(_),
            ) => Some(Full::Bool(true)),

            _ => None,
        };

        let range = matches!(
            capabilities.requests.range,
            Some(
                ClientSemanticTokensRequestOptionsRange::Bool(true)
                    | ClientSemanticTokensRequestOptionsRange::Object(_)
            )
        )
        .then(|| true.into());

        if full.is_none() && range.is_none() {
            return None;
        }

        Some(SemanticTokensOptions {
            legend,
            full,
            range,
            ..SemanticTokensOptions::default()
        })
    }

    pub(crate) fn forget(&mut self, uri: &Uri) {
        self.records.retain(|record| &record.uri != uri);
    }

    fn store(&mut self, uri: Uri, data: Vec<SemanticToken>) -> Result<SemanticTokens> {
        let retained = self.provider().is_some_and(|provider| matches!(provider.full, Some(Full::SemanticTokensFullDelta(options)) if options.delta == Some(true)));

        if !retained {
            return Ok(SemanticTokens {
                result_id: None,
                data,
            });
        }

        self.generation = self
            .generation
            .checked_add(1)
            .ok_or_else(Error::internal_error)?;

        let identifier = self.generation.to_string();

        self.records.push_back(Record {
            identifier: identifier.clone(),
            uri,
            data: data.clone(),
        });

        while self.records.len() > RETAINED_LISTS {
            self.records.pop_front();
        }

        Ok(SemanticTokens {
            result_id: Some(identifier),
            data,
        })
    }
}

struct Record {
    identifier: String,
    uri: Uri,
    data: Vec<SemanticToken>,
}

struct Prepared {
    snapshot: Snapshot,
    inputs: BTreeSet<PathBuf>,
    data: Vec<SemanticToken>,
}

#[derive(Clone, Copy)]
enum Request {
    Full,
    Delta,
    Range(Range),
}

impl Session {
    pub(crate) async fn tokens(&self, parameters: SemanticTokensParams) -> Result<SemanticTokens> {
        let uri = parameters.text_document.uri;
        let prepared = self.prepare_tokens(&uri, Request::Full).await?;
        let mut workspace = self.workspace.lock().expect("workspace lock");

        if !workspace.current(&prepared.snapshot, &prepared.inputs) {
            return Err(Error::content_modified());
        }

        workspace.tokens.store(uri, prepared.data)
    }

    pub(crate) async fn token_changes(
        &self,
        parameters: SemanticTokensDeltaParams,
    ) -> Result<SemanticTokensDeltaResponse> {
        let uri = parameters.text_document.uri;
        let prepared = self.prepare_tokens(&uri, Request::Delta).await?;
        let mut workspace = self.workspace.lock().expect("workspace lock");

        if !workspace.current(&prepared.snapshot, &prepared.inputs) {
            return Err(Error::content_modified());
        }

        let changes = workspace
            .tokens
            .records
            .iter()
            .find(|record| record.uri == uri && record.identifier == parameters.previous_result_id)
            .map(|record| difference(&record.data, &prepared.data))
            .transpose()?;

        let response = workspace.tokens.store(uri, prepared.data)?;

        Ok(match changes {
            Some(edits) => SemanticTokensDelta {
                result_id: response.result_id,
                edits,
            }
            .into(),

            None => response.into(),
        })
    }

    pub(crate) async fn token_range(
        &self,
        parameters: SemanticTokensRangeParams,
    ) -> Result<SemanticTokens> {
        let prepared = self
            .prepare_tokens(
                &parameters.text_document.uri,
                Request::Range(parameters.range),
            )
            .await?;

        let workspace = self.workspace.lock().expect("workspace lock");

        if !workspace.current(&prepared.snapshot, &prepared.inputs) {
            return Err(Error::content_modified());
        }

        Ok(SemanticTokens {
            result_id: None,
            data: prepared.data,
        })
    }

    async fn prepare_tokens(&self, uri: &Uri, request: Request) -> Result<Prepared> {
        let (snapshot, provider) = {
            let workspace = self.workspace.lock().expect("workspace lock");

            let provider = workspace
                .tokens
                .provider()
                .ok_or_else(|| Error::invalid_params("semantic tokens were not negotiated"))?;

            let supported = match request {
                Request::Full => provider.full.is_some(),

                Request::Delta => {
                    matches!(provider.full, Some(Full::SemanticTokensFullDelta(options)) if options.delta == Some(true))
                }

                Request::Range(_) => provider.range.is_some(),
            };

            if !supported {
                return Err(Error::invalid_params(
                    "semantic token request was not negotiated",
                ));
            }

            (workspace.snapshot.clone(), provider)
        };

        let (data, inputs) = self
            .query(
                uri,
                &self.queries,
                move |path, encoding, project, options, started| {
                    let source = project.source(path)?;

                    let range = if let Request::Range(range) = request {
                        [
                            position::offset(&source.text, range.start, encoding)
                                .map_err(|error| invalid(error.to_string()))?,
                            position::offset(&source.text, range.end, encoding)
                                .map_err(|error| invalid(error.to_string()))?,
                        ]
                    } else {
                        [0, source.text.len()]
                    };

                    if source.text.get(range[0]..range[1]).is_none() {
                        return Err(invalid("invalid semantic token range"));
                    }

                    let entries =
                        project.entries(&[path.to_path_buf()], &options.remaining(started))?;

                    let [entry] = entries.as_slice() else {
                        return Err(invalid("semantic tokens require one module context"));
                    };

                    let tokens = project.tokens(entry, &options.remaining(started))?;

                    if tokens.location.revision != source.revision
                        || tokens.location.module.source() != path
                    {
                        return Err(invalid("semantic token source revision mismatch"));
                    }

                    let data = render(
                        &tokens.entries,
                        &source.text,
                        range,
                        encoding,
                        &provider.legend,
                        options,
                        started,
                    )?;

                    Ok((data, project.watch_inputs()))
                },
            )
            .await?;

        Ok(Prepared {
            snapshot,
            inputs,
            data,
        })
    }
}

fn render(
    tokens: &[Token],
    source: &str,
    range: [usize; 2],
    encoding: &PositionEncodingKind,
    legend: &SemanticTokensLegend,
    options: &Options,
    started: Instant,
) -> io::Result<Vec<SemanticToken>> {
    let mut result = Vec::new();
    let mut offset = 0;
    let mut current = Position::default();
    let mut previous = Position::default();

    for token in tokens {
        options.check(started)?;
        let start = token.range[0].max(range[0]);
        let end = token.range[1].min(range[1]);

        if start >= end {
            continue;
        }

        let Some((_, name)) = KINDS.iter().find(|(kind, _)| *kind == token.kind) else {
            continue;
        };

        let Some(kind) = legend
            .token_types
            .iter()
            .position(|kind| kind.as_str() == *name)
        else {
            continue;
        };

        let kind = u32::try_from(kind).map_err(invalid)?;

        let modifiers =
            legend
                .token_modifiers
                .iter()
                .enumerate()
                .fold(0_u32, |mask, (index, modifier)| {
                    let present = MODIFIERS
                        .iter()
                        .any(|(kind, name)| *name == modifier && token.modifiers.contains(kind));

                    if present { mask | (1 << index) } else { mask }
                });

        advance(&source[offset..start], &mut current, encoding)?;
        offset = start;

        while offset < end {
            options.check(started)?;
            let text = &source[offset..end];
            let length = text.find(['\r', '\n']).unwrap_or(text.len());

            if length != 0 {
                let width = position::end(&text[..length], encoding).character;

                result.push(SemanticToken {
                    delta_line: current.line - previous.line,
                    delta_start: if current.line == previous.line {
                        current.character - previous.character
                    } else {
                        current.character
                    },
                    length: width,
                    token_type: kind,
                    token_modifiers_bitset: modifiers,
                });

                previous = current;

                current.character = current
                    .character
                    .checked_add(width)
                    .ok_or_else(|| invalid("semantic token position overflow"))?;

                offset += length;
            }

            if offset < end {
                let length = if source[offset..end].starts_with("\r\n") {
                    2
                } else {
                    1
                };

                advance(&source[offset..offset + length], &mut current, encoding)?;
                offset += length;
            }
        }
    }

    options.check(started)?;

    Ok(result)
}

fn advance(text: &str, position: &mut Position, encoding: &PositionEncodingKind) -> io::Result<()> {
    let ending = position::end(text, encoding);

    position.line = position
        .line
        .checked_add(ending.line)
        .ok_or_else(|| invalid("semantic token line overflow"))?;

    position.character = if ending.line == 0 {
        position
            .character
            .checked_add(ending.character)
            .ok_or_else(|| invalid("semantic token column overflow"))?
    } else {
        ending.character
    };

    Ok(())
}

fn difference(
    previous: &[SemanticToken],
    current: &[SemanticToken],
) -> Result<Vec<SemanticTokensEdit>> {
    let prefix = previous
        .iter()
        .zip(current)
        .take_while(|(left, right)| left == right)
        .count();

    let suffix = previous[prefix..]
        .iter()
        .rev()
        .zip(current[prefix..].iter().rev())
        .take_while(|(left, right)| left == right)
        .count();

    if prefix == previous.len() && prefix == current.len() {
        return Ok(Vec::new());
    }

    let start = u32::try_from(prefix * 5).map_err(|error| failure(&invalid(error)))?;

    let delete_count = u32::try_from((previous.len() - prefix - suffix) * 5)
        .map_err(|error| failure(&invalid(error)))?;

    let data = current[prefix..current.len() - suffix]
        .iter()
        .flat_map(|token| {
            [
                token.delta_line,
                token.delta_start,
                token.length,
                token.token_type,
                token.token_modifiers_bitset,
            ]
        })
        .collect::<Vec<_>>();

    Ok(vec![SemanticTokensEdit {
        start,
        delete_count,
        data: (!data.is_empty()).then_some(data),
    }])
}
