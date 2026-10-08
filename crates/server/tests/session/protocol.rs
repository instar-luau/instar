use std::{
    collections::BTreeSet,
    sync::{Arc, Mutex, atomic::AtomicBool, mpsc},
    time::Duration,
};

use instar_analysis::Options;
use serde_json::{Value, json};

use tower_lsp_server::{
    ClientSocket, LspService,
    gen_lsp_types::Uri,
    jsonrpc::{Request, Response},
};

use tower_service::Service;

use super::{Job, Output, Workers};
use crate::{dispatch::Dispatch, session::Session};

fn fixture(options: Options) -> (Dispatch, mpsc::Receiver<Job>, ClientSocket) {
    let (sender, receiver) = mpsc::sync_channel(1);

    let workers = Arc::new(Workers {
        sender: Mutex::new(Some(sender)),
        handles: Mutex::new(Vec::new()),
        active: Mutex::new(Vec::new()),
    });

    let (service, socket) = LspService::build_with_lifecycle_methods(|client| {
        Session::new(client, workers, options, Arc::new(AtomicBool::new(false)))
    })
    .custom_method("textDocument/didOpen", Session::open)
    .custom_method("textDocument/didChange", Session::change)
    .custom_method("textDocument/formatting", Session::format)
    .finish();

    (
        Dispatch {
            service,
            exited: Arc::new(AtomicBool::new(false)),
        },
        receiver,
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
        .call(request("initialize", 0, json!({"capabilities":{}})))
        .await
        .expect("initialize transport")
        .expect("initialize response");

    assert!(response.is_ok(), "{response:?}");
}

fn code(response: &Response) -> i64 {
    response.error().expect("error response").code.code()
}

#[tokio::test]
async fn dispatch_captures_in_receipt_order_and_rejects_obsolete_results() {
    let directory = tempfile::tempdir().expect("workspace directory");
    let uri = Uri::from_file_path(directory.path().join("entry.luau")).expect("file URI");
    let (mut dispatch, jobs, _socket) = fixture(Options::new(Duration::from_secs(5)));
    initialize(&mut dispatch).await;

    let opened = dispatch.call(notification(
        "textDocument/didOpen",
        json!({"textDocument":{"uri":uri,"languageId":"luau","version":1,"text":"before"}}),
    ));

    let response = dispatch.call(request(
        "textDocument/formatting",
        1,
        json!({"textDocument":{"uri":uri},"options":{"tabSize":4,"insertSpaces":false}}),
    ));

    let changed = dispatch.call(notification(
        "textDocument/didChange",
        json!({"textDocument":{"uri":uri,"version":2},"contentChanges":[{"text":"after"}]}),
    ));

    let job = jobs.try_recv().expect("queued request");
    let source = Arc::clone(job.snapshot.source(&job.path).expect("captured source"));
    assert_eq!(source.as_ref(), "before");

    assert!(
        job.response
            .send(Output {
                text: Ok(Some((source, "formatted".to_owned()))),
                inputs: BTreeSet::from([job.path])
            })
            .is_ok()
    );

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
        job.snapshot
            .source(&job.path)
            .expect("current source")
            .as_ref(),
        "after"
    );

    let source = Arc::clone(job.snapshot.source(&job.path).expect("current source"));

    assert!(
        job.response
            .send(Output {
                text: Ok(Some((source, "formatted".to_owned()))),
                inputs: BTreeSet::from([job.path])
            })
            .is_ok()
    );

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
    let (mut dispatch, jobs, _socket) = fixture(Options::new(Duration::from_secs(5)));
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
    let (mut dispatch, jobs, _socket) = fixture(options);
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
