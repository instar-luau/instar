use std::{
    io,
    sync::{Arc, mpsc},
    thread,
};

use instar_core::project::Project;
use serde_json::{Value, json};
use tokio::sync::oneshot;
use tower_lsp_server::ls_types::{CompletionItemKind, Range};

use crate::{
    Snapshot,
    document::{Document, uri},
    worker::Query,
};

pub(crate) enum Request {
    Links,
    Definition(usize),

    Completion {
        prefix: String,
        range: Range,
        delimiter: String,
    },
}

pub(crate) struct Command {
    pub(crate) snapshot: Snapshot,
    pub(crate) document: Arc<Document>,
    pub(crate) request: Request,
    pub(crate) reply: oneshot::Sender<Result<Value, String>>,
}

pub(crate) fn start() -> (mpsc::Sender<Option<Command>>, thread::JoinHandle<()>) {
    let (sender, receiver) = mpsc::channel::<Option<Command>>();

    let thread = thread::spawn(move || {
        let mut project = Project::default();
        let mut previous = Snapshot::default();

        while let Ok(Some(command)) = receiver.recv() {
            if command.reply.is_closed() {
                continue;
            }

            let result = update(&mut project, &previous, &command.snapshot)
                .and_then(|()| resolve(&mut project, &command.document, command.request));

            previous = command.snapshot;

            drop(
                command
                    .reply
                    .send(result.map_err(|error| error.to_string())),
            );
        }
    });

    (sender, thread)
}

fn update(project: &mut Project, previous: &Snapshot, current: &Snapshot) -> io::Result<()> {
    if previous.epoch == current.epoch {
        for path in previous
            .documents
            .keys()
            .filter(|path| !current.documents.contains_key(*path))
        {
            project.set_source(path, None)?;
        }
    } else {
        *project = Project::default();
    }

    for (path, document) in &current.documents {
        if previous.epoch != current.epoch
            || previous
                .documents
                .get(path)
                .is_none_or(|old| !Arc::ptr_eq(old, document))
        {
            project.set_source(path, Some(&document.text))?;
        }
    }

    Ok(())
}

pub(crate) fn at(document: &Document, query: &Query, offset: usize) -> io::Result<Option<Request>> {
    if !matches!(query, Query::Definition(..) | Query::Completion(..)) {
        return Ok(None);
    }

    let span = document
        .bindings()
        .import_at(offset)
        .map(|site| site.span)
        .or_else(|| {
            matches!(query, Query::Completion(..))
                .then(|| recover_literal(document, offset))
                .flatten()
        });

    let Some(span) = span else {
        return Ok(None);
    };

    if matches!(query, Query::Definition(..)) {
        return Ok(Some(Request::Definition(offset)));
    }

    let raw = &document.text[span.start..span.end];

    let Some((opening, delimiter)) = delimiters(raw) else {
        return Ok(None);
    };

    let start = span.start + opening;

    let end = if raw.len() >= opening + delimiter.len() && raw.ends_with(&delimiter) {
        span.end - delimiter.len()
    } else {
        span.end
    };

    if offset < start || offset > end {
        return Ok(None);
    }

    let literal = format!("{}{}", &document.text[span.start..offset], delimiter);
    let prefix = instar_core::string_value(literal.as_bytes())?;

    Ok(Some(Request::Completion {
        prefix,
        range: document.range(start, end),
        delimiter,
    }))
}

fn delimiters(raw: &str) -> Option<(usize, String)> {
    match raw.as_bytes().first()? {
        b'\'' | b'"' => Some((1, raw[..1].to_owned())),

        b'[' => {
            let equal = raw
                .as_bytes()
                .iter()
                .skip(1)
                .take_while(|&&byte| byte == b'=')
                .count();

            (raw.as_bytes().get(equal + 1) == Some(&b'['))
                .then(|| (equal + 2, format!("]{}]", "=".repeat(equal))))
        }

        _ => None,
    }
}

fn recover_literal(document: &Document, offset: usize) -> Option<vermis::Span> {
    let span = document.bindings().literal_at(offset)?;
    let raw = &document.text[span.start..span.end];
    let (opening, delimiter) = delimiters(raw)?;

    let close = if raw.len() >= opening + delimiter.len() && raw.ends_with(&delimiter) {
        ""
    } else {
        &delimiter
    };

    for parenthesis in ["", ")"] {
        if close.is_empty() && parenthesis.is_empty() {
            continue;
        }

        let repaired = format!(
            "{}{close}{parenthesis}{}",
            &document.text[..span.end],
            &document.text[span.end..]
        );

        let tree = vermis::parse(repaired.as_bytes());
        let index = crate::bindings::Index::new(&tree);

        if index
            .import_at(offset)
            .is_some_and(|site| site.span.start == span.start)
        {
            return Some(span);
        }
    }

    None
}

fn resolve(project: &mut Project, document: &Document, request: Request) -> io::Result<Value> {
    match request {
        Request::Links => {
            Ok(
                json!(project.links(&document.path)?.into_iter().map(|(span, target)| {
            Ok(json!({"range": document.range(span[0], span[1]), "target": uri(&target)?}))
        }).collect::<io::Result<Vec<_>>>()?),
            )
        }

        Request::Definition(offset) => Ok(json!(
            project
                .links(&document.path)?
                .into_iter()
                .filter(|(span, _)| span[0] <= offset && offset <= span[1])
                .map(|(_, target)| { Ok(json!({"uri": uri(&target)?, "range": Range::default()})) })
                .collect::<io::Result<Vec<_>>>()?
        )),

        Request::Completion {
            prefix,
            range,
            delimiter,
        } => {
            let mut items = Vec::new();

            for candidate in project.complete_import(&document.path, &prefix)? {
                let text = if delimiter.len() == 1 && delimiter != "]" {
                    let encoded = serde_json::to_string(&candidate)?;
                    let content = &encoded[1..encoded.len() - 1];

                    if delimiter == "'" {
                        content.replace('\'', "\\'")
                    } else {
                        content.to_owned()
                    }
                } else {
                    if candidate.contains(&delimiter) {
                        continue;
                    }

                    candidate.clone()
                };

                items.push(json!({"label": candidate, "kind": if candidate.ends_with('/') { CompletionItemKind::FOLDER } else { CompletionItemKind::FILE },
                    "textEdit": {"range": range, "newText": text}}));
            }

            Ok(json!({"isIncomplete": false, "items": items}))
        }
    }
}
