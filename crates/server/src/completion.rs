use std::{
    collections::{BTreeSet, VecDeque},
    io,
    path::{Path, PathBuf},
    time::Instant,
};

use instar_analysis::{Edit, Options};

use instar_bridge::frontend::{
    Notation, Parentheses, Suggestion, SuggestionContext, SuggestionKind,
};

use instar_core::{
    analysis::Origin,
    project::Project,
    snapshot::Snapshot,
    suggestions::{self, Suggestions},
};

use instar_syntax::literal::quote;
use serde_json::{Value, json};

use tower_lsp_server::{
    gen_lsp_types::{
        CompletionClientCapabilities, CompletionItem, CompletionItemKind, CompletionItemTag,
        CompletionList, CompletionParams, InsertTextFormat, MarkupKind, Position,
        PositionEncodingKind, TextEdit, Uri,
    },
    jsonrpc::{Error, Result},
};

use crate::{markup, position, session::Session};

const RETAINED_LISTS: usize = 32;

#[derive(Default)]
pub(crate) struct Completions {
    pub(crate) capabilities: CompletionClientCapabilities,
    generation: u64,
    records: VecDeque<Record>,
}

struct Record {
    generation: u64,
    snapshot: Snapshot,
    inputs: BTreeSet<PathBuf>,
    uri: Uri,
    candidates: Vec<Candidate>,
}

#[derive(Clone)]
struct Candidate {
    item: CompletionItem,
    symbol: Option<String>,
    edits: Vec<TextEdit>,
}

struct Insertion {
    text: String,
    filter: String,
    range: [usize; 2],
    snippet: bool,
}

struct Request {
    position: Position,
    capabilities: CompletionClientCapabilities,
    roots: Vec<PathBuf>,
}

impl Session {
    pub(crate) async fn complete(&self, parameters: CompletionParams) -> Result<CompletionList> {
        let parameters = parameters.text_document_position_params;
        let uri = parameters.text_document.uri;

        let (snapshot, request) = {
            let workspace = self.workspace.lock().expect("workspace lock");
            workspace.editable(&uri)?;

            (
                workspace.snapshot.clone(),
                Request {
                    position: parameters.position,
                    capabilities: workspace.completions.capabilities.clone(),
                    roots: workspace.roots.iter().cloned().collect(),
                },
            )
        };

        let (mut candidates, incomplete, inputs) = self
            .query(
                &uri,
                &self.queries,
                move |path, encoding, project, options, started| {
                    let (candidates, incomplete) =
                        query(path, &request, encoding, project, options, started)?;

                    Ok((candidates, incomplete, project.watch_inputs()))
                },
            )
            .await?;

        let mut workspace = self.workspace.lock().expect("workspace lock");

        if !workspace.current(&snapshot, &inputs) {
            return Err(Error::content_modified());
        }

        let completions = &mut workspace.completions;

        let generation = completions
            .generation
            .checked_add(1)
            .ok_or_else(Error::internal_error)?;

        completions.generation = generation;

        for (index, candidate) in candidates.iter_mut().enumerate() {
            candidate.item.data = Some(json!({"completion": generation, "item": index}));
        }

        let items = candidates
            .iter()
            .map(|candidate| candidate.item.clone())
            .collect();

        completions.records.push_back(Record {
            generation,
            snapshot,
            inputs,
            uri,
            candidates,
        });

        while completions.records.len() > RETAINED_LISTS {
            completions.records.pop_front();
        }

        Ok(CompletionList {
            is_incomplete: incomplete,
            items,
            ..CompletionList::default()
        })
    }

    pub(crate) async fn resolve_completion(&self, item: CompletionItem) -> Result<CompletionItem> {
        let data = item
            .data
            .as_ref()
            .ok_or_else(|| Error::invalid_params("missing completion identity"))?;

        let generation = data
            .get("completion")
            .and_then(Value::as_u64)
            .ok_or_else(|| Error::invalid_params("invalid completion identity"))?;

        let index = data
            .get("item")
            .and_then(Value::as_u64)
            .and_then(|index| usize::try_from(index).ok())
            .ok_or_else(|| Error::invalid_params("invalid completion item"))?;

        let (snapshot, inputs, uri, mut candidate, markup) = {
            let workspace = self.workspace.lock().expect("workspace lock");

            let record = workspace
                .completions
                .records
                .iter()
                .find(|record| record.generation == generation)
                .ok_or_else(Error::content_modified)?;

            if !workspace.current(&record.snapshot, &record.inputs) {
                return Err(Error::content_modified());
            }

            let candidate = record
                .candidates
                .get(index)
                .ok_or_else(|| Error::invalid_params("invalid completion item"))?;

            let markup = workspace
                .completions
                .capabilities
                .completion_item
                .as_ref()
                .and_then(|capabilities| capabilities.documentation_format.as_ref())
                .and_then(|formats| {
                    formats.iter().find(|format| {
                        matches!(format, MarkupKind::PlainText | MarkupKind::Markdown)
                    })
                })
                .cloned()
                .unwrap_or(MarkupKind::PlainText);

            (
                record.snapshot.clone(),
                record.inputs.clone(),
                record.uri.clone(),
                candidate.clone(),
                markup,
            )
        };

        if let Some(symbol) = candidate.symbol {
            candidate.item.documentation = self
                .query(
                    &uri,
                    &self.queries,
                    move |path, _, project, options, started| {
                        project.source(path)?;

                        let documentation = project.documentation_for(
                            path,
                            &symbol,
                            &options.remaining(started),
                        )?;

                        options.check(started)?;

                        Ok(documentation.map(|entry| markup::documentation(entry, markup)))
                    },
                )
                .await?;
        }

        candidate.item.additional_text_edits =
            (!candidate.edits.is_empty()).then_some(candidate.edits);

        if !self
            .workspace
            .lock()
            .expect("workspace lock")
            .current(&snapshot, &inputs)
        {
            return Err(Error::content_modified());
        }

        Ok(candidate.item)
    }
}

fn query(
    path: &Path,
    request: &Request,
    encoding: &PositionEncodingKind,
    project: &mut Project,
    options: &Options,
    started: Instant,
) -> io::Result<(Vec<Candidate>, bool)> {
    options.check(started)?;
    let source = project.source(path)?;

    let offset = position::offset(&source.text, request.position, encoding)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error.to_string()))?;

    let entries = project.entries(&[path.to_path_buf()], &options.remaining(started))?;

    let [entry] = entries.as_slice() else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "completion requires one unambiguous module context",
        ));
    };

    let resolve_edits = request
        .capabilities
        .completion_item
        .as_ref()
        .and_then(|capabilities| capabilities.resolve_support.as_ref())
        .is_some_and(|support| {
            support
                .properties
                .iter()
                .any(|property| property == "additionalTextEdits")
        });

    let roots = if resolve_edits {
        if request.roots.is_empty() {
            vec![
                path.parent()
                    .ok_or_else(|| io::Error::other("source has no parent"))?
                    .to_path_buf(),
            ]
        } else {
            request.roots.clone()
        }
    } else {
        Vec::new()
    };

    let suggestions = project.suggest(entry, offset, &roots, &options.remaining(started))?;
    let mut candidates = Vec::new();

    for suggestion in &suggestions.entries {
        options.check(started)?;

        if let Some(candidate) = candidate(
            &source.text,
            &suggestions,
            suggestion,
            encoding,
            &request.capabilities,
        )? {
            candidates.push(candidate);
        }
    }

    options.check(started)?;

    let incomplete = suggestions.require
        || (!roots.is_empty()
            && suggestions.loader
            && matches!(
                suggestions.context,
                SuggestionContext::Expression | SuggestionContext::Statement
            ));

    Ok((candidates, incomplete))
}

fn candidate(
    source: &str,
    suggestions: &Suggestions,
    candidate: &suggestions::Candidate,
    encoding: &PositionEncodingKind,
    capabilities: &CompletionClientCapabilities,
) -> io::Result<Option<Candidate>> {
    let suggestion = &candidate.entry;
    let item_capabilities = capabilities.completion_item.as_ref();

    let snippets =
        item_capabilities.and_then(|capabilities| capabilities.snippet_support) == Some(true);

    let insertion = insertion(source, suggestions, suggestion, snippets);

    let edits = supporting_edits(
        source,
        suggestions,
        &candidate.edits,
        insertion.range,
        encoding,
    )?;

    let range = position::range(source, insertion.range, encoding)?;

    if range.start.line != range.end.line {
        return Ok(None);
    }

    let deprecated = suggestion.deprecated
        && item_capabilities
            .and_then(|capabilities| capabilities.tag_support.as_ref())
            .is_some_and(|tags| tags.value_set.contains(&CompletionItemTag::Deprecated));

    let commits = item_capabilities.and_then(|capabilities| capabilities.commit_characters_support)
        == Some(true)
        && !insertion.snippet
        && suggestions.context != SuggestionContext::String
        && suggestion.parentheses == Parentheses::None;

    let kind = kind(suggestion);

    let supported = capabilities
        .completion_item_kind
        .as_ref()
        .and_then(|capabilities| capabilities.value_set.as_ref())
        .map_or(kind != CompletionItemKind::Folder, |kinds| {
            kinds.contains(&kind)
        });

    Ok(Some(Candidate {
        symbol: suggestion.documentation.clone(),
        edits,
        item: CompletionItem {
            label: suggestion.name.clone(),
            kind: Some(if supported {
                kind
            } else {
                CompletionItemKind::Text
            }),
            detail: (!suggestion.detail.is_empty()).then(|| suggestion.detail.clone()),
            tags: deprecated.then_some(vec![CompletionItemTag::Deprecated]),
            sort_text: Some(format!(
                "{}{}",
                if suggestion.preferred { "0" } else { "1" },
                suggestion.name
            )),
            filter_text: Some(insertion.filter),
            insert_text_format: insertion.snippet.then_some(InsertTextFormat::Snippet),
            text_edit: Some(
                TextEdit {
                    range,
                    new_text: insertion.text,
                }
                .into(),
            ),
            commit_characters: commits.then(|| vec![".".to_owned(), ":".to_owned()]),
            ..CompletionItem::default()
        },
    }))
}

fn insertion(
    source: &str,
    suggestions: &Suggestions,
    suggestion: &Suggestion,
    snippets: bool,
) -> Insertion {
    let mut range = suggestions.location.range;

    let insertion = suggestion
        .insert_text
        .as_deref()
        .unwrap_or(&suggestion.name);

    let mut text = insertion.to_owned();
    let mut filter = suggestion.name.clone();

    if suggestions.context == SuggestionContext::String {
        text = quote(insertion);
        let original = &source[range[0]..range[1]];

        let opening = if let Some(content) = original.strip_prefix('[') {
            content.find('[').map_or("", |end| &original[..end + 2])
        } else {
            original.get(..1).unwrap_or("")
        };

        filter = format!("{opening}{}", suggestion.name);
    } else if let Some(index) = suggestions.index {
        match suggestion.notation {
            Notation::Identifier => {}

            Notation::Index | Notation::Method => {
                range[0] = index[0];
                filter = format!("{}{}", &source[index[0]..index[1]], suggestion.name);

                text = if suggestion.notation == Notation::Index {
                    format!("[{}]", quote(insertion))
                } else {
                    format!(":{insertion}")
                };
            }
        }
    }

    let mut snippet = false;

    if snippets && suggestion.kind == SuggestionKind::Function && text.ends_with("  end") {
        text = format!("{}\n\t$0\nend", escape(&text[..text.len() - 5]));
        snippet = true;
    } else if suggestions.context != SuggestionContext::String
        && !source[suggestions.location.range[1]..]
            .trim_start()
            .starts_with('(')
    {
        match suggestion.parentheses {
            Parentheses::After => text.push_str("()"),

            Parentheses::Inside if snippets => {
                text = format!("{}($0)", escape(&text));
                snippet = true;
            }

            Parentheses::None | Parentheses::Inside => {}
        }
    }

    Insertion {
        text,
        filter,
        range,
        snippet,
    }
}

fn supporting_edits(
    source: &str,
    suggestions: &Suggestions,
    edits: &[Edit<Origin>],
    replacement: [usize; 2],
    encoding: &PositionEncodingKind,
) -> io::Result<Vec<TextEdit>> {
    let mut ranges = vec![replacement];
    let mut output = Vec::new();

    for edit in edits {
        if edit.location.module != suggestions.location.module
            || edit.location.revision != suggestions.location.revision
        {
            return Err(io::Error::other(
                "completion edit has a different source revision",
            ));
        }

        let [start, end] = edit.location.range;

        if start > end
            || ranges
                .iter()
                .any(|range| start <= range[1] && range[0] <= end)
        {
            return Err(io::Error::other("completion edits must be disjoint"));
        }

        output.push(TextEdit {
            range: position::range(source, edit.location.range, encoding)?,
            new_text: edit.text.clone(),
        });

        ranges.push(edit.location.range);
    }

    Ok(output)
}

fn kind(suggestion: &Suggestion) -> CompletionItemKind {
    match suggestion.kind {
        SuggestionKind::Property if suggestion.callable => CompletionItemKind::Method,
        SuggestionKind::Binding if suggestion.callable => CompletionItemKind::Function,
        SuggestionKind::Property => CompletionItemKind::Field,
        SuggestionKind::Binding => CompletionItemKind::Variable,
        SuggestionKind::Keyword | SuggestionKind::Comment => CompletionItemKind::Keyword,
        SuggestionKind::String => CompletionItemKind::Value,
        SuggestionKind::Type => CompletionItemKind::Class,
        SuggestionKind::Module => CompletionItemKind::Module,
        SuggestionKind::Function => CompletionItemKind::Snippet,
        SuggestionKind::Path if suggestion.name.ends_with('/') => CompletionItemKind::Folder,
        SuggestionKind::Path => CompletionItemKind::File,
    }
}

fn escape(text: &str) -> String {
    text.replace('\\', "\\\\")
        .replace('$', "\\$")
        .replace('}', "\\}")
}
