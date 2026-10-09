use std::{
    io,
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    task::{Context, Poll},
};

use instar_analysis::Options;

use tokio::{
    io::{AsyncRead, AsyncWrite, ReadBuf, stdin, stdout},
    runtime::Builder,
    sync::Notify,
};

use tower_lsp_server::{LspService, Server};

use crate::{diagnostics::Diagnostics, dispatch::Dispatch, session::Session, workers::Workers};

struct Connection<Stream> {
    stream: Stream,
    disconnected: Arc<Notify>,
}

impl<Stream: AsyncRead + Unpin> AsyncRead for Connection<Stream> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let previous = buffer.filled().len();
        let result = Pin::new(&mut self.stream).poll_read(context, buffer);

        if matches!(result, Poll::Ready(Err(_)))
            || (matches!(result, Poll::Ready(Ok(()))) && buffer.filled().len() == previous)
        {
            self.disconnected.notify_one();
        }

        result
    }
}

impl<Stream: AsyncWrite + Unpin> AsyncWrite for Connection<Stream> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<io::Result<usize>> {
        let result = Pin::new(&mut self.stream).poll_write(context, bytes);

        if matches!(result, Poll::Ready(Err(_)))
            || (!bytes.is_empty() && matches!(result, Poll::Ready(Ok(0))))
        {
            self.disconnected.notify_one();
        }

        result
    }

    fn poll_flush(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<io::Result<()>> {
        let result = Pin::new(&mut self.stream).poll_flush(context);

        if matches!(result, Poll::Ready(Err(_))) {
            self.disconnected.notify_one();
        }

        result
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.stream).poll_shutdown(context)
    }
}

/// Serves a language-server connection, returning whether shutdown preceded exit.
///
/// # Errors
/// Returns worker setup or cleanup failures.
pub async fn serve<Input, Output>(
    input: Input,
    output: Output,
    options: Options,
) -> io::Result<bool>
where
    Input: AsyncRead + Unpin + Send + 'static,
    Output: AsyncWrite + Unpin + Send + 'static,
{
    let workers = Arc::new(Workers::new(&options)?);
    let queries = Arc::new(Workers::new(&options)?);
    let diagnostics = Arc::new(Diagnostics::new(&options)?);
    let shutdown = Arc::new(AtomicBool::new(false));
    let exited = Arc::new(AtomicBool::new(false));
    let disconnected = Arc::new(Notify::new());

    let (service, socket) = LspService::build_with_lifecycle_methods(|client| {
        Session::new(
            client,
            Arc::clone(&workers),
            Arc::clone(&queries),
            options,
            Arc::clone(&shutdown),
            Arc::clone(&diagnostics),
        )
    })
    .custom_method("textDocument/didOpen", Session::open)
    .custom_method("textDocument/didChange", Session::change)
    .custom_method("textDocument/didClose", Session::close)
    .custom_method("textDocument/didSave", Session::save)
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
    .custom_method("workspace/didChangeWatchedFiles", Session::watched)
    .custom_method("workspace/didChangeWorkspaceFolders", Session::folders)
    .custom_method("workspace/didChangeConfiguration", Session::configuration)
    .custom_method("workspace/didCreateFiles", Session::created)
    .custom_method("workspace/willRenameFiles", Session::move_imports)
    .custom_method("workspace/didRenameFiles", Session::renamed)
    .custom_method("workspace/didDeleteFiles", Session::deleted)
    .custom_method("workspace/textDocumentContent", Session::content)
    .finish();

    let dispatch = Dispatch {
        service,
        exited: Arc::clone(&exited),
    };

    let input = Connection {
        stream: input,
        disconnected: Arc::clone(&disconnected),
    };

    let output = Connection {
        stream: output,
        disconnected: Arc::clone(&disconnected),
    };

    tokio::select! {
        () = Server::new(input, output, socket).concurrency_level(usize::MAX).serve(dispatch) => {},
        () = disconnected.notified() => {},
    }

    diagnostics.finish()?;
    queries.finish()?;
    workers.finish()?;

    Ok(shutdown.load(Ordering::Relaxed) && exited.load(Ordering::Relaxed))
}

/// Runs the language server on process stdio.
///
/// # Errors
/// Returns runtime initialization or worker lifecycle failures.
pub fn stdio(options: Options) -> io::Result<bool> {
    let runtime = Builder::new_current_thread().enable_all().build()?;
    let result = runtime.block_on(serve(stdin(), stdout(), options));
    runtime.shutdown_background();

    result
}
