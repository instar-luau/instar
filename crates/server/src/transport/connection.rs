use std::{
    io,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};

use tokio::{
    io::{AsyncRead, AsyncWrite, ReadBuf},
    sync::Notify,
};

pub(super) struct Connection<Stream> {
    pub(super) stream: Stream,
    pub(super) disconnected: Arc<Notify>,
}

impl<Stream: AsyncRead + Unpin> AsyncRead for Connection<Stream> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let previous = buffer.filled().len();
        let requested = buffer.remaining();
        let result = Pin::new(&mut self.stream).poll_read(context, buffer);

        if matches!(result, Poll::Ready(Err(_)))
            || (requested != 0
                && matches!(result, Poll::Ready(Ok(())))
                && buffer.filled().len() == previous)
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
