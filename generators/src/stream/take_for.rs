use futures::{
    task::{Context, Poll},
    Sink, Stream,
};
use pin_utils::unsafe_pinned;
use std::{
    pin::Pin,
    time::{Duration, Instant},
};

/// Stream combinator that terminates a stream after a given duration.
pub struct TakeFor<S> {
    stream: S,
    now: Instant,
    duration: Duration,
}

impl<S> TakeFor<S> {
    unsafe_pinned!(stream: S);

    pub fn new(stream: S, duration: Duration) -> TakeFor<S> {
        TakeFor {
            stream,
            now: Instant::now(),
            duration,
        }
    }
}

impl<S, T> Sink<T> for TakeFor<S>
where
    S: Sink<T>,
{
    type Error = S::Error;

    fn poll_ready(self: Pin<&mut Self>, cx: &mut Context) -> Poll<Result<(), Self::Error>> {
        self.stream().poll_ready(cx)
    }

    fn start_send(self: Pin<&mut Self>, item: T) -> Result<(), Self::Error> {
        self.stream().start_send(item)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context) -> Poll<Result<(), Self::Error>> {
        self.stream().poll_flush(cx)
    }

    fn poll_close(self: Pin<&mut Self>, cx: &mut Context) -> Poll<Result<(), Self::Error>> {
        self.stream().poll_close(cx)
    }
}

impl<S> Stream for TakeFor<S>
where
    S: Stream,
{
    type Item = S::Item;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context) -> Poll<Option<Self::Item>> {
        // While our function has not been running for long enough, continue polling the
        // underlying stream. Otherwise, terminate the stream.
        if self.now.elapsed() < self.duration {
            self.stream().poll_next(cx)
        } else {
            Poll::Ready(None)
        }
    }
}
