use futures::{
    task::{Context, Poll},
    FutureExt, Sink, Stream,
};
use pin_utils::unsafe_pinned;
use std::{pin::Pin, time::Duration};
use tokio::time;

/// Stream combinator that delays each Stream::Item by a given duration.
pub struct Delay<S> {
    stream: S,
    duration: Duration,
    delay: Option<time::Delay>,
}

impl<S> Delay<S> {
    unsafe_pinned!(stream: S);

    pub fn new(stream: S, duration: Duration) -> Delay<S> {
        Delay {
            stream,
            duration,
            delay: None,
        }
    }
}

impl<T, S> Sink<T> for Delay<S>
where
    S: Sink<T>,
{
    type Error = S::Error;

    fn poll_ready(mut self: Pin<&mut Self>, cx: &mut Context) -> Poll<Result<(), Self::Error>> {
        self.as_mut().stream().poll_ready(cx)
    }

    fn start_send(mut self: Pin<&mut Self>, item: T) -> Result<(), Self::Error> {
        self.as_mut().stream().start_send(item)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context) -> Poll<Result<(), Self::Error>> {
        self.as_mut().stream().poll_flush(cx)
    }

    fn poll_close(mut self: Pin<&mut Self>, cx: &mut Context) -> Poll<Result<(), Self::Error>> {
        self.as_mut().stream().poll_close(cx)
    }
}

impl<S> Stream for Delay<S>
where
    S: Stream,
{
    type Item = S::Item;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context) -> Poll<Option<Self::Item>> {
        let duration = self.duration;
        let delay = self.delay.get_or_insert_with(|| time::delay_for(duration));

        match delay.poll_unpin(cx) {
            Poll::Ready(_) => {
                self.delay = None;
                self.as_mut().stream().poll_next(cx)
            }
            Poll::Pending => Poll::Pending,
        }
    }
}
