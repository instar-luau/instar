use std::{
    fs,
    num::NonZeroUsize,
    sync::{Arc, mpsc},
    time::{Duration, Instant},
};

use instar_project::Project;
use instar_shared::{EventPayload, Options};
use serde_json::{Value, json};
use tokio::{sync::mpsc as asynchronous_channel, time};

use tower_lsp_server::{
    ClientSocket, LspService,
    gen_lsp_types::Uri,
    jsonrpc::{Request, Response},
};

use tower_service::Service;

use crate::{Server, transport::Dispatch};

struct Job {
    release: mpsc::Sender<()>,
}

fn fixture(
    mut options: Options,
) -> (
    Dispatch,
    asynchronous_channel::UnboundedReceiver<Job>,
    ClientSocket,
    Arc<Project>,
) {
    let (sender, receiver) = asynchronous_channel::unbounded_channel();
    let observer = options.observer.clone();

    options.observer = Some(Arc::new(move |event| {
        if let Some(observer) = &observer {
            observer(event.clone());
        }

        if matches!(&event.payload, EventPayload::Progress(progress) if progress.phase == "Executing project operation")
        {
            let (release, waiting) = mpsc::channel();

            if sender.send(Job { release }).is_ok() {
                let _ = waiting.recv_timeout(Duration::from_secs(5));
            }
        }
    }));

    let project = Arc::new(Project::new(Vec::new()));

    let (service, socket) = LspService::build_with_lifecycle_methods(|client| {
        Server::new(client, Arc::clone(&project), options)
    })
    .custom_method("window/workDoneProgress/cancel", Server::cancel_progress)
    .finish();

    (Dispatch { service }, receiver, socket, project)
}

fn options() -> Options {
    Options::new(Some(Instant::now() + Duration::from_secs(5)))
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

fn code(response: &Response) -> i64 {
    response.error().expect("error response").code.code()
}

async fn job(jobs: &mut asynchronous_channel::UnboundedReceiver<Job>) -> Job {
    time::timeout(Duration::from_secs(5), jobs.recv())
        .await
        .expect("project operation started")
        .expect("project task")
}

fn execute(job: Job) {
    job.release.send(()).expect("release project operation");
}

async fn initialize(dispatch: &mut Dispatch) {
    let response = dispatch.call(request("initialize",0,json!({"capabilities":{"textDocument":{"diagnostic":{},"semanticTokens":{"requests":{"full":{"delta":true},"range":true},"tokenTypes":["variable"],"tokenModifiers":[],"formats":["relative"]}}}}))).await.expect("initialize transport").expect("initialize response");
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
            json!({"textDocument":{"uri":uri},"range":{"start":{"line":line,"character":character},"end":{"line":line,"character":character}},"context":{"diagnostics":[],"only":["source.organizeImports"]}})
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

    let (mut dispatch, mut jobs, _socket, project) = fixture(options());
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

    let captured = job(&mut jobs).await;
    let original = project.snapshot();

    assert_eq!(
        original
            .source_for_address(uri.as_str())
            .expect("source")
            .text
            .as_ref(),
        source
    );

    dispatch.call(notification("textDocument/didChange",json!({"textDocument":{"uri":uri,"version":2},"contentChanges":[{"text":include_str!("../../../cli/tests/fixtures/hover/updated.luau")}]}))).await.expect("edit transport");
    execute(captured);

    assert_eq!(
        code(
            &pending
                .await
                .expect("query transport")
                .expect("query response")
        ),
        -32801
    );

    assert_eq!(
        original
            .source_for_address(uri.as_str())
            .expect("immutable source")
            .text
            .as_ref(),
        source
    );

    let pending = dispatch.call(request(method, 2, query_parameters(method, &uri, 1, 7)));
    let cancelled = job(&mut jobs).await;

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

    execute(cancelled);
    let pending = dispatch.call(request(method, 3, query_parameters(method, &uri, 1, 7)));
    let stopped = job(&mut jobs).await;
    let shutdown = dispatch.call(request("shutdown", 4, Value::Null));
    execute(stopped);

    assert!(
        shutdown
            .await
            .expect("shutdown transport")
            .expect("shutdown response")
            .is_ok()
    );

    assert_eq!(
        code(
            &pending
                .await
                .expect("cancelled transport")
                .expect("cancelled response")
        ),
        -32800
    );
}

#[tokio::test]
async fn dispatch_captures_in_receipt_order_and_rejects_obsolete_results() {
    let directory = tempfile::tempdir().expect("workspace directory");
    let uri = Uri::from_file_path(directory.path().join("entry.luau")).expect("file URI");
    let (mut dispatch, mut jobs, _socket, project) = fixture(options());
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

    let captured = job(&mut jobs).await;
    let original = project.snapshot();

    let changed = dispatch.call(notification(
        "textDocument/didChange",
        json!({"textDocument":{"uri":uri,"version":2},"contentChanges":[{"text":"return 2"}]}),
    ));

    assert_eq!(
        original
            .source_for_address(uri.as_str())
            .expect("captured source")
            .text
            .as_ref(),
        "return 1"
    );

    execute(captured);

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

    let next = job(&mut jobs).await;

    assert_eq!(
        project
            .snapshot()
            .source_for_address(uri.as_str())
            .expect("current source")
            .text
            .as_ref(),
        "return 2"
    );

    execute(next);

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
    let mut options = options();
    options.jobs = NonZeroUsize::MIN;
    let (mut dispatch, mut jobs, _socket, _project) = fixture(options);
    initialize(&mut dispatch).await;

    dispatch
        .call(notification(
            "textDocument/didOpen",
            json!({"textDocument":{"uri":uri,"languageId":"luau","version":1,"text":"return 1"}}),
        ))
        .await
        .expect("open transport");

    let parameters =
        json!({"textDocument":{"uri":uri},"options":{"tabSize":4,"insertSpaces":false}});

    let running = dispatch.call(request("textDocument/formatting", 1, parameters.clone()));
    let active = job(&mut jobs).await;
    let queued = dispatch.call(request("textDocument/formatting", 2, parameters.clone()));

    let rejected = dispatch
        .call(request("textDocument/formatting", 3, parameters.clone()))
        .await
        .expect("busy transport")
        .expect("busy response");

    assert_eq!(code(&rejected), -32802);

    dispatch
        .call(notification("$/cancelRequest", json!({"id":2})))
        .await
        .expect("queued cancellation transport");

    assert_eq!(
        code(&queued.await.expect("transport").expect("response")),
        -32800
    );

    dispatch
        .call(notification("$/cancelRequest", json!({"id":1})))
        .await
        .expect("running cancellation transport");

    assert_eq!(
        code(&running.await.expect("transport").expect("response")),
        -32800
    );

    execute(active);
    let pending = dispatch.call(request("textDocument/formatting", 4, parameters));
    let stopped = job(&mut jobs).await;
    let shutdown = dispatch.call(request("shutdown", 5, Value::Null));
    execute(stopped);

    assert!(
        shutdown
            .await
            .expect("shutdown transport")
            .expect("shutdown response")
            .is_ok()
    );

    assert_eq!(
        code(&pending.await.expect("transport").expect("response")),
        -32800
    );

    dispatch
        .call(notification("exit", Value::Null))
        .await
        .expect("exit transport");
}

#[tokio::test]
async fn requests_observe_caller_cancellation_without_cancelling_siblings() {
    let directory = tempfile::tempdir().expect("workspace directory");
    let uri = Uri::from_file_path(directory.path().join("entry.luau")).expect("file URI");
    let mut options = options();
    options.jobs = NonZeroUsize::new(2).expect("two jobs");
    let cancellation = options.cancellation.clone();
    let (mut dispatch, mut jobs, _socket, _project) = fixture(options);
    initialize(&mut dispatch).await;

    dispatch
        .call(notification(
            "textDocument/didOpen",
            json!({"textDocument":{"uri":uri,"languageId":"luau","version":1,"text":"return 1"}}),
        ))
        .await
        .expect("open transport");

    let parameters =
        json!({"textDocument":{"uri":uri},"options":{"tabSize":4,"insertSpaces":false}});

    let first = dispatch.call(request("textDocument/formatting", 1, parameters.clone()));
    let first_job = job(&mut jobs).await;
    let second = dispatch.call(request("textDocument/formatting", 2, parameters));
    let second_job = job(&mut jobs).await;
    drop(first);
    assert!(!cancellation.requested());
    execute(first_job);
    cancellation.cancel();
    execute(second_job);

    assert_eq!(
        code(&second.await.expect("transport").expect("response")),
        -32800
    );
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
    let mut options = options();
    options.jobs = NonZeroUsize::new(2).expect("two jobs");
    let (mut dispatch, mut jobs, _socket, project) = fixture(options);
    initialize(&mut dispatch).await;

    dispatch
        .call(notification(
            "textDocument/didOpen",
            json!({"textDocument":{"uri":uri,"languageId":"luau","version":1,"text":"return 1"}}),
        ))
        .await
        .expect("open transport");

    let semantic = dispatch.call(request(
        "textDocument/hover",
        1,
        json!({"textDocument":{"uri":uri},"position":{"line":0,"character":0}}),
    ));

    let blocked = job(&mut jobs).await;

    dispatch
        .call(notification(
            "textDocument/didChange",
            json!({"textDocument":{"uri":uri,"version":2},"contentChanges":[{"text":"return 2"}]}),
        ))
        .await
        .expect("edit transport");

    let formatting = dispatch.call(request(
        "textDocument/formatting",
        2,
        json!({"textDocument":{"uri":uri},"options":{"tabSize":4,"insertSpaces":false}}),
    ));

    let formatting_job = job(&mut jobs).await;

    assert_eq!(
        project
            .snapshot()
            .source_for_address(uri.as_str())
            .expect("current source")
            .text
            .as_ref(),
        "return 2"
    );

    execute(formatting_job);

    assert!(
        formatting
            .await
            .expect("format transport")
            .expect("format response")
            .is_ok()
    );

    let symbols = dispatch.call(request(
        "textDocument/documentSymbol",
        3,
        json!({"textDocument":{"uri":uri}}),
    ));

    execute(job(&mut jobs).await);

    let response = symbols
        .await
        .expect("symbol transport")
        .expect("symbol response");

    assert_eq!(response.result(), Some(&json!([])));
    let shutdown = dispatch.call(request("shutdown", 4, Value::Null));
    execute(blocked);

    assert!(
        shutdown
            .await
            .expect("shutdown transport")
            .expect("shutdown response")
            .is_ok()
    );

    assert_eq!(
        code(
            &semantic
                .await
                .expect("semantic transport")
                .expect("semantic response")
        ),
        -32800
    );
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
    let (mut dispatch, mut jobs, _socket, _project) = fixture(options());
    initialize(&mut dispatch).await;
    let pending = dispatch.call(request("workspace/symbol", 1, json!({"query":""})));
    let captured = job(&mut jobs).await;
    dispatch.call(notification("textDocument/didOpen",json!({"textDocument":{"uri":uri,"languageId":"luau","version":1,"text":include_str!("../../../cli/tests/fixtures/symbols/input.luau")}}))).await.expect("open transport");
    execute(captured);

    assert_eq!(
        code(
            &pending
                .await
                .expect("query transport")
                .expect("query response")
        ),
        -32801
    );

    assert!(
        dispatch
            .call(request("shutdown", 2, Value::Null))
            .await
            .expect("shutdown transport")
            .expect("shutdown response")
            .is_ok()
    );
}
