use super::{Response, Result, internal_error, protocol};
use std::{pin::Pin, task::Poll};
use tokio::sync::{mpsc, oneshot};
use tower_lsp_server::{Bounded, Client, NotCancellable, OngoingProgress};

#[derive(Default)]
struct Progress(Option<OngoingProgress<Bounded, NotCancellable>>);

impl Drop for Progress {
    fn drop(&mut self) {
        if let Some(progress) = self.0.take() {
            tokio::spawn(progress.finish());
        }
    }
}

enum Event {
    Progress(usize, usize),
    Response(std::result::Result<Result<Response>, oneshot::error::RecvError>),
}

pub(super) async fn wait(
    mut receiver: oneshot::Receiver<Result<Response>>,
    mut events: mpsc::UnboundedReceiver<(usize, usize)>,
    client: &Client,
    mut token: Option<protocol::ProgressToken>,
) -> Result<Response> {
    let mut progress = Progress::default();

    loop {
        let event = std::future::poll_fn(|context| {
            if let Poll::Ready(Some((completed, total))) = events.poll_recv(context) {
                return Poll::Ready(Event::Progress(completed, total));
            }

            Pin::new(&mut receiver).poll(context).map(Event::Response)
        })
        .await;

        match event {
            Event::Response(response) => {
                if let Some(progress) = progress.0.take() {
                    progress.finish().await;
                }

                return response.map_err(internal_error)?;
            }

            Event::Progress(mut completed, mut total) => {
                while let Ok(latest) = events.try_recv() {
                    (completed, total) = latest;
                }

                if progress.0.is_none()
                    && let Some(token) = token.take()
                    && client
                        .create_work_done_progress(token.clone())
                        .await
                        .is_ok()
                {
                    progress.0 = Some(
                        client
                            .progress(token, "Index workspace")
                            .with_percentage(0)
                            .begin()
                            .await,
                    );
                }

                if let Some(progress) = &progress.0 {
                    let percentage = u32::try_from(completed.saturating_mul(100) / total.max(1))
                        .map_err(internal_error)?;

                    progress
                        .report_with_message(format!("{completed}/{total} files"), percentage)
                        .await;
                }
            }
        }
    }
}
