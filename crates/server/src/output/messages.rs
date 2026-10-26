use std::sync::Arc;

use instar_core::workspace::GeneratorReporter;
use tokio::runtime::Handle;

use tower_lsp_server::{
    Client,
    gen_lsp_types::{MessageActionItem, MessageType, Uri},
    jsonrpc::{Error, ErrorCode, Result},
};

pub(crate) fn generator(client: &Client) -> GeneratorReporter {
    let client = client.clone();
    let runtime = Handle::current();

    Arc::new(move |level, executable, message| {
        tracing::debug!(generator = executable, "Received generator output");

        let kind = match level {
            tracing::Level::ERROR => MessageType::Error,
            tracing::Level::WARN => MessageType::Warning,
            _ => MessageType::Info,
        };

        let client = client.clone();
        let name = executable.rsplit(['/', '\\']).next().unwrap_or(executable);

        let lines: Vec<_> = message
            .split('\n')
            .map(|line| format!("[{name}] {line}"))
            .collect();

        let message = lines.join("\n");

        drop(runtime.spawn(async move {
            client.log_message(kind, message.clone()).await;

            if matches!(kind, MessageType::Warning | MessageType::Error) {
                client.show_message(kind, message).await;
            }
        }));
    })
}

pub(crate) async fn report(client: &Client, error: Error) {
    if matches!(
        error.code,
        ErrorCode::RequestCancelled | ErrorCode::ContentModified
    ) {
        return;
    }

    client
        .log_message(MessageType::Error, error.to_string())
        .await;

    client
        .show_message(MessageType::Error, error.message.into_owned())
        .await;
}

pub(crate) async fn warning(client: &Client, error: &Error) {
    client
        .log_message(MessageType::Warning, error.to_string())
        .await;
}

pub(crate) async fn initialized(client: &Client) {
    client
        .log_message(MessageType::Info, "Instar is ready.")
        .await;
}

pub(crate) async fn configuration(
    client: &Client,
    roots: &[Uri],
) -> Result<Option<MessageActionItem>> {
    let actions = roots
        .iter()
        .map(|root| MessageActionItem {
            title: root.to_string(),
            ..MessageActionItem::default()
        })
        .collect();

    client
        .show_message_request(
            MessageType::Info,
            "Select a workspace to open its instar.toml",
            Some(actions),
        )
        .await
}
