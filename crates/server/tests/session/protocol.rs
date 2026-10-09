use std::{
    fs,
    sync::{Arc, Mutex, atomic::AtomicBool, mpsc},
    time::Duration,
};

use instar_analysis::Options;
use instar_core::project::Project;
use instar_format::Formatter;
use serde_json::{Value, json};
use tokio::{sync::oneshot, time};

use tower_lsp_server::{
    ClientSocket, LspService,
    gen_lsp_types::Uri,
    jsonrpc::{Request, Response},
};

use tower_service::Service;

use super::{Job, Workers};
use crate::{diagnostics::Diagnostics, dispatch::Dispatch, session::Session};

fn queue() -> (Arc<Workers>, mpsc::Receiver<Job>) {
    let (sender, receiver) = mpsc::sync_channel(1);

    let workers = Arc::new(Workers {
        sender: Mutex::new(Some(sender)),
        handles: Mutex::new(Vec::new()),
        active: Mutex::new(Vec::new()),
    });

    (workers, receiver)
}

fn fixture(
    options: Options,
) -> (
    Dispatch,
    mpsc::Receiver<Job>,
    mpsc::Receiver<Job>,
    ClientSocket,
) {
    let (workers, receiver) = queue();
    let (queries, queued) = queue();
    let diagnostics = Arc::new(Diagnostics::new(&options).expect("diagnostic worker"));

    let (service, socket) = LspService::build_with_lifecycle_methods(|client| {
        Session::new(
            client,
            workers,
            queries,
            options,
            Arc::new(AtomicBool::new(false)),
            diagnostics,
        )
    })
    .custom_method("textDocument/didOpen", Session::open)
    .custom_method("textDocument/didChange", Session::change)
    .custom_method("textDocument/diagnostic", Session::document_diagnostics)
    .custom_method("workspace/diagnostic", Session::workspace_diagnostics)
    .custom_method("textDocument/formatting", Session::format)
    .custom_method("textDocument/rangeFormatting", Session::format_range)
    .custom_method("textDocument/onTypeFormatting", Session::format_character)
    .custom_method("textDocument/hover", Session::hover)
    .custom_method("textDocument/completion", Session::complete)
    .custom_method("completionItem/resolve", Session::resolve_completion)
    .custom_method("textDocument/signatureHelp", Session::signatures)
    .custom_method("textDocument/definition", Session::definition)
    .custom_method("textDocument/typeDefinition", Session::type_definition)
    .custom_method("textDocument/implementation", Session::implementations)
    .custom_method("textDocument/prepareCallHierarchy", Session::prepare_calls)
    .custom_method("callHierarchy/incomingCalls", Session::incoming_calls)
    .custom_method("callHierarchy/outgoingCalls", Session::outgoing_calls)
    .custom_method("textDocument/prepareTypeHierarchy", Session::prepare_types)
    .custom_method("typeHierarchy/supertypes", Session::supertypes)
    .custom_method("typeHierarchy/subtypes", Session::subtypes)
    .custom_method("textDocument/documentSymbol", Session::symbols)
    .custom_method("workspace/symbol", Session::search_symbols)
    .custom_method("textDocument/references", Session::references)
    .custom_method("textDocument/documentHighlight", Session::highlights)
    .custom_method("textDocument/prepareRename", Session::prepare_rename)
    .custom_method("textDocument/rename", Session::rename)
    .custom_method("textDocument/codeAction", Session::actions)
    .custom_method("workspace/willRenameFiles", Session::move_imports)
    .custom_method("textDocument/semanticTokens/full", Session::tokens)
    .custom_method(
        "textDocument/semanticTokens/full/delta",
        Session::token_changes,
    )
    .custom_method("textDocument/semanticTokens/range", Session::token_range)
    .custom_method("textDocument/inlayHint", Session::hints)
    .custom_method("inlayHint/resolve", Session::resolve_hint)
    .custom_method("textDocument/foldingRange", Session::folds)
    .custom_method("textDocument/selectionRange", Session::selections)
    .custom_method("textDocument/documentLink", Session::links)
    .finish();

    (
        Dispatch {
            service,
            exited: Arc::new(AtomicBool::new(false)),
        },
        receiver,
        queued,
        socket,
    )
}

fn notification(method: &'static str, parameters: Value) -> Request {
    Request::build(method).params(parameters).finish()
}

fn request(method: &'static str, identifier: i64, parameters: Value) -> Request {
    Request::build(method)
        .id(identifier)
        .params(parameters)
        .finish()
}

async fn initialize(dispatch: &mut Dispatch) {
    let response = dispatch
        .call(request("initialize", 0, json!({"capabilities":{"textDocument":{"semanticTokens":{"requests":{"full":{"delta":true},"range":true},"tokenTypes":["variable"],"tokenModifiers":[],"formats":["relative"]}}}})))
        .await
        .expect("initialize transport")
        .expect("initialize response");

    assert!(response.is_ok(), "{response:?}");
}

fn query_parameters(method: &str, uri: &Uri, line: u32, character: usize) -> Value {
    match method {
        "workspace/symbol" => json!({"query":""}),

        "textDocument/references" => {
            json!({"textDocument":{"uri":uri},"position":{"line":line,"character":character},"context":{"includeDeclaration":true}})
        }

        "textDocument/rename" => {
            json!({"textDocument":{"uri":uri},"position":{"line":line,"character":character},"newName":"renamed"})
        }

        "textDocument/codeAction" => {
            json!({"textDocument":{"uri":uri},"range":{"start":{"line":line,"character":character},"end":{"line":line,"character":character}},"context":{"diagnostics":[]}})
        }

        "workspace/willRenameFiles" => json!({"files":[]}),

        "workspace/diagnostic" => json!({"previousResultIds":[]}),

        "textDocument/diagnostic" => json!({"textDocument":{"uri":uri}}),

        "textDocument/rangeFormatting" => {
            json!({"textDocument":{"uri":uri},"range":{"start":{"line":line,"character":character},"end":{"line":line,"character":character}},"options":{"tabSize":4,"insertSpaces":false}})
        }

        "textDocument/onTypeFormatting" => {
            json!({"textDocument":{"uri":uri},"position":{"line":line,"character":character},"ch":";","options":{"tabSize":4,"insertSpaces":false}})
        }

        "textDocument/semanticTokens/full/delta" => {
            json!({"textDocument":{"uri":uri},"previousResultId":"unknown"})
        }

        "textDocument/semanticTokens/range" | "textDocument/inlayHint" => {
            json!({"textDocument":{"uri":uri},"range":{"start":{"line":line,"character":character},"end":{"line":line,"character":character}}})
        }

        "textDocument/selectionRange" => {
            json!({"textDocument":{"uri":uri},"positions":[{"line":line,"character":character}]})
        }

        _ => json!({"textDocument":{"uri":uri},"position":{"line":line,"character":character}}),
    }
}

fn code(response: &Response) -> i64 {
    response.error().expect("error response").code.code()
}

fn execute(job: Job) -> bool {
    let mut project = Project::new(job.options.timeout);
    project.apply(&job.snapshot);

    (job.operation)(
        &mut project,
        &mut Formatter::default(),
        &job.options,
        job.started,
    )
}

async fn query_lifecycle(method: &'static str) {
    let directory = tempfile::tempdir().expect("workspace directory");

    fs::write(
        directory.path().join("instar.toml"),
        include_str!("../../../cli/tests/fixtures/diagnostics/instar.toml"),
    )
    .expect("configuration");

    let path = directory.path().join("entry.luau");
    let uri = Uri::from_file_path(&path).expect("source URI");
    let source = include_str!("../../../cli/tests/fixtures/hover/input.luau");

    let character = source[..source.rfind("count").expect("reference")]
        .encode_utf16()
        .count();

    let (mut dispatch, formatting, queries, _socket) =
        fixture(Options::new(Duration::from_secs(5)));

    let queries = if matches!(
        method,
        "textDocument/documentSymbol"
            | "workspace/symbol"
            | "workspace/willRenameFiles"
            | "textDocument/foldingRange"
            | "textDocument/selectionRange"
            | "textDocument/documentLink"
            | "textDocument/rangeFormatting"
            | "textDocument/onTypeFormatting"
    ) {
        formatting
    } else {
        queries
    };

    initialize(&mut dispatch).await;

    dispatch
        .call(notification(
            "textDocument/didOpen",
            json!({"textDocument":{"uri":uri,"languageId":"luau","version":1,"text":source}}),
        ))
        .await
        .expect("open transport");

    let pending = dispatch.call(request(
        method,
        1,
        query_parameters(method, &uri, 0, character),
    ));

    let job = queries.try_recv().expect("captured query");
    assert_eq!(job.snapshot.source(&path).expect("source").as_ref(), source);
    dispatch.call(notification("textDocument/didChange", json!({"textDocument":{"uri":uri,"version":2},"contentChanges":[{"text":include_str!("../../../cli/tests/fixtures/hover/updated.luau")}]}))).await.expect("edit transport");
    assert!(execute(job));

    assert_eq!(
        code(
            &pending
                .await
                .expect("query transport")
                .expect("query response")
        ),
        -32801
    );

    let pending = dispatch.call(request(method, 2, query_parameters(method, &uri, 1, 7)));

    let job = queries.try_recv().expect("queued query");

    dispatch
        .call(notification("$/cancelRequest", json!({"id":2})))
        .await
        .expect("cancel transport");

    assert_eq!(
        code(
            &pending
                .await
                .expect("query transport")
                .expect("query response")
        ),
        -32800
    );

    assert!(job.options.cancellation.requested());

    let pending = dispatch.call(request(method, 3, query_parameters(method, &uri, 1, 7)));

    let job = queries.try_recv().expect("shutdown query");

    let stopped = dispatch
        .call(request("shutdown", 4, Value::Null))
        .await
        .expect("shutdown transport")
        .expect("shutdown response");

    assert!(stopped.is_ok());
    assert!(job.options.cancellation.requested());
    drop(pending);
}

#[tokio::test]
async fn dispatch_captures_in_receipt_order_and_rejects_obsolete_results() {
    let directory = tempfile::tempdir().expect("workspace directory");
    let uri = Uri::from_file_path(directory.path().join("entry.luau")).expect("file URI");
    let (mut dispatch, jobs, _queries, _socket) = fixture(Options::new(Duration::from_secs(5)));
    initialize(&mut dispatch).await;

    let opened = dispatch.call(notification(
        "textDocument/didOpen",
        json!({"textDocument":{"uri":uri,"languageId":"luau","version":1,"text":"return 1"}}),
    ));

    let response = dispatch.call(request(
        "textDocument/formatting",
        1,
        json!({"textDocument":{"uri":uri},"options":{"tabSize":4,"insertSpaces":false}}),
    ));

    let changed = dispatch.call(notification(
        "textDocument/didChange",
        json!({"textDocument":{"uri":uri,"version":2},"contentChanges":[{"text":"return 2"}]}),
    ));

    let job = jobs.try_recv().expect("queued request");
    let path = uri.to_file_path().expect("source path");
    let source = Arc::clone(job.snapshot.source(&path).expect("captured source"));
    assert_eq!(source.as_ref(), "return 1");
    assert!(execute(job));

    assert_eq!(
        code(&response.await.expect("transport").expect("response")),
        -32801
    );

    assert!(changed.await.expect("change transport").is_none());
    assert!(opened.await.expect("open transport").is_none());

    let response = dispatch.call(request(
        "textDocument/formatting",
        2,
        json!({"textDocument":{"uri":uri},"options":{"tabSize":4,"insertSpaces":false}}),
    ));

    let job = jobs.try_recv().expect("next request");

    assert_eq!(
        job.snapshot.source(&path).expect("current source").as_ref(),
        "return 2"
    );

    assert!(execute(job));

    assert!(
        response
            .await
            .expect("transport")
            .expect("response")
            .is_ok()
    );
}

#[tokio::test]
async fn bounded_queue_keeps_cancellation_and_shutdown_responsive() {
    let directory = tempfile::tempdir().expect("workspace directory");
    let uri = Uri::from_file_path(directory.path().join("entry.luau")).expect("file URI");
    let (mut dispatch, jobs, _queries, _socket) = fixture(Options::new(Duration::from_secs(5)));
    initialize(&mut dispatch).await;

    let parameters =
        json!({"textDocument":{"uri":uri},"options":{"tabSize":4,"insertSpaces":false}});

    let pending = dispatch.call(request("textDocument/formatting", 1, parameters.clone()));

    let rejected = dispatch
        .call(request("textDocument/formatting", 2, parameters.clone()))
        .await
        .expect("busy transport")
        .expect("busy response");

    assert_eq!(code(&rejected), -32802);

    dispatch
        .call(notification("$/cancelRequest", json!({"id":1})))
        .await
        .expect("cancellation transport");

    assert_eq!(
        code(&pending.await.expect("transport").expect("response")),
        -32800
    );

    let cancelled = jobs.try_recv().expect("cancelled queued job");
    assert!(cancelled.options.cancellation.requested());
    let pending = dispatch.call(request("textDocument/formatting", 3, parameters.clone()));
    let running = jobs.try_recv().expect("running job");

    dispatch
        .call(notification("$/cancelRequest", json!({"id":3})))
        .await
        .expect("running cancellation");

    assert_eq!(
        code(&pending.await.expect("transport").expect("response")),
        -32800
    );

    assert!(running.options.cancellation.requested());
    let pending = dispatch.call(request("textDocument/formatting", 4, parameters));

    let shutdown = dispatch
        .call(request("shutdown", 5, Value::Null))
        .await
        .expect("shutdown transport")
        .expect("shutdown response");

    assert!(shutdown.is_ok());
    let stopped = jobs.try_recv().expect("shutdown job");
    assert!(stopped.options.cancellation.requested());
    drop(pending);

    dispatch
        .call(notification("exit", Value::Null))
        .await
        .expect("exit transport");
}

#[tokio::test]
async fn requests_observe_caller_cancellation_without_cancelling_siblings() {
    let directory = tempfile::tempdir().expect("workspace directory");
    let uri = Uri::from_file_path(directory.path().join("entry.luau")).expect("file URI");
    let options = Options::new(Duration::from_secs(5));
    let cancellation = options.cancellation.clone();
    let (mut dispatch, jobs, _queries, _socket) = fixture(options);
    initialize(&mut dispatch).await;

    let parameters =
        json!({"textDocument":{"uri":uri},"options":{"tabSize":4,"insertSpaces":false}});

    let first = dispatch.call(request("textDocument/formatting", 1, parameters.clone()));
    let first_job = jobs.try_recv().expect("first job");
    let second = dispatch.call(request("textDocument/formatting", 2, parameters));
    let second_job = jobs.try_recv().expect("second job");
    drop(first);
    assert!(first_job.options.cancellation.requested());
    assert!(!cancellation.requested());
    assert!(!second_job.options.cancellation.requested());
    cancellation.cancel();
    assert!(second_job.options.cancellation.requested());
    drop(second);
}

#[tokio::test]
async fn semantic_work_does_not_block_edits_syntax_queries_or_shutdown() {
    let directory = tempfile::tempdir().expect("workspace directory");

    fs::write(
        directory.path().join("instar.toml"),
        include_str!("../../../cli/tests/fixtures/diagnostics/instar.toml"),
    )
    .expect("configuration");

    let uri = Uri::from_file_path(directory.path().join("entry.luau")).expect("file URI");
    let (entered, waiting) = oneshot::channel();
    let entered = Mutex::new(Some(entered));
    let (release, released) = mpsc::channel();
    let released = Mutex::new(released);
    let mut options = Options::new(Duration::from_secs(5));

    options.progress = Some(Arc::new(move |progress| {
        if progress.phase == "Discovering dependencies"
            && let Some(entered) = entered.lock().expect("analysis signal").take()
        {
            let _ = entered.send(());

            released
                .lock()
                .expect("analysis release")
                .recv_timeout(Duration::from_secs(5))
                .expect("release semantic work");
        }
    }));

    let (mut dispatch, jobs, _queries, _socket) = fixture(options);
    initialize(&mut dispatch).await;

    dispatch
        .call(notification(
            "textDocument/didOpen",
            json!({"textDocument":{"uri":uri,"languageId":"luau","version":1,"text":"return 1"}}),
        ))
        .await
        .expect("open transport");

    time::timeout(Duration::from_secs(5), waiting)
        .await
        .expect("analysis started")
        .expect("analysis signal");

    dispatch
        .call(notification(
            "textDocument/didChange",
            json!({"textDocument":{"uri":uri,"version":2},"contentChanges":[{"text":"return 2"}]}),
        ))
        .await
        .expect("edit transport");

    let formatting = dispatch.call(request(
        "textDocument/formatting",
        1,
        json!({"textDocument":{"uri":uri},"options":{"tabSize":4,"insertSpaces":false}}),
    ));

    let job = jobs
        .try_recv()
        .expect("formatting admitted during semantic work");

    let path = uri.to_file_path().expect("source path");
    let source = Arc::clone(job.snapshot.source(&path).expect("current source"));
    assert_eq!(source.as_ref(), "return 2");
    assert!(execute(job));

    assert!(
        formatting
            .await
            .expect("format transport")
            .expect("format response")
            .is_ok()
    );

    let symbols = dispatch.call(request(
        "textDocument/documentSymbol",
        2,
        json!({"textDocument":{"uri":uri}}),
    ));

    let job = jobs
        .try_recv()
        .expect("symbols admitted during semantic work");

    assert_eq!(
        job.snapshot.source(&path).expect("symbol source").as_ref(),
        "return 2"
    );

    assert!(execute(job));

    let response = symbols
        .await
        .expect("symbol transport")
        .expect("symbol response");

    assert_eq!(response.result(), Some(&json!([])));

    assert!(
        dispatch
            .call(request("shutdown", 3, Value::Null))
            .await
            .expect("shutdown transport")
            .expect("shutdown response")
            .is_ok()
    );

    release.send(()).expect("release semantic worker");
}

#[tokio::test]
async fn queries_reject_stale_work_and_cancel_queued_requests() {
    for method in [
        "textDocument/hover",
        "textDocument/completion",
        "textDocument/signatureHelp",
        "textDocument/definition",
        "textDocument/typeDefinition",
        "textDocument/implementation",
        "textDocument/prepareCallHierarchy",
        "textDocument/prepareTypeHierarchy",
        "textDocument/documentSymbol",
        "workspace/symbol",
        "textDocument/references",
        "textDocument/documentHighlight",
        "textDocument/prepareRename",
        "textDocument/rename",
        "textDocument/codeAction",
        "workspace/willRenameFiles",
        "textDocument/semanticTokens/full",
        "textDocument/semanticTokens/full/delta",
        "textDocument/semanticTokens/range",
        "textDocument/inlayHint",
        "textDocument/foldingRange",
        "textDocument/selectionRange",
        "textDocument/documentLink",
        "textDocument/rangeFormatting",
        "textDocument/onTypeFormatting",
        "textDocument/diagnostic",
        "workspace/diagnostic",
    ] {
        query_lifecycle(method).await;
    }
}

#[tokio::test]
async fn workspace_queries_reject_changes_outside_previous_inputs() {
    let directory = tempfile::tempdir().expect("workspace directory");
    let uri = Uri::from_file_path(directory.path().join("entry.luau")).expect("source URI");
    let (mut dispatch, jobs, _queries, _socket) = fixture(Options::new(Duration::from_secs(5)));
    initialize(&mut dispatch).await;
    let pending = dispatch.call(request("workspace/symbol", 1, json!({"query":""})));
    let job = jobs.try_recv().expect("workspace query");
    dispatch.call(notification("textDocument/didOpen", json!({"textDocument":{"uri":uri,"languageId":"luau","version":1,"text":include_str!("../../../cli/tests/fixtures/symbols/input.luau")}}))).await.expect("open transport");
    assert!(execute(job));

    assert_eq!(
        code(
            &pending
                .await
                .expect("query transport")
                .expect("query response")
        ),
        -32801
    );

    let stopped = dispatch
        .call(request("shutdown", 2, Value::Null))
        .await
        .expect("shutdown transport")
        .expect("shutdown response");

    assert!(stopped.is_ok());
}
