use futures::{
    task::{Context, Poll},
    Future, FutureExt, Sink, Stream,
};
use log::debug;
use pin_utils::unsafe_pinned;
use std::{
    pin::Pin,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};
use tokio::time;

type Trigger = Arc<AtomicBool>;

/// A trigger to terminate any Interruptable streams attached to it.
pub struct Interrupt(Trigger);

/// Stream combinator that terminates a stream when the trigger is pulled.
pub struct Interruptable<S> {
    stream: S,
    trigger: Trigger,
}

impl Interrupt {
    pub fn new() -> Interrupt {
        Interrupt(Arc::new(AtomicBool::new(false)))
    }

    /// Link a stream to this `Interrupt`
    pub fn attach<S>(&self, stream: S) -> Interruptable<S>
    where
        S: Stream,
    {
        Interruptable {
            stream,
            trigger: self.0.clone(),
        }
    }

    /// Interrupt the attached streams now
    pub fn now(self) {
        debug!("Terminating attached streams now");
        self.0.store(false, Ordering::Relaxed);
    }

    /// Interrupt the attached streams after the given duration
    pub fn after(self, duration: Duration) -> impl Future<Output = ()> {
        debug!("Terminating attached streams in {:?}", duration);
        time::delay_for(duration).then(|_| {
            self.now();
            async {}
        })
    }
}

impl<S> Interruptable<S> {
    unsafe_pinned!(stream: S);
}

impl<S, T> Sink<T> for Interruptable<S>
where
    S: Sink<T>,
{
    type Error = S::Error;

    fn poll_ready(self: Pin<&mut Self>, cx: &mut Context) -> Poll<Result<(), Self::Error>> {
        self.as_mut().stream().poll_ready(cx)
    }

    fn start_send(self: Pin<&mut Self>, item: T) -> Result<(), Self::Error> {
        self.as_mut().stream().start_send(item)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context) -> Poll<Result<(), Self::Error>> {
        self.as_mut().stream().poll_flush(cx)
    }

    fn poll_close(self: Pin<&mut Self>, cx: &mut Context) -> Poll<Result<(), Self::Error>> {
        self.as_mut().stream().poll_close(cx)
    }
}

impl<S> Stream for Interruptable<S>
where
    S: Stream,
{
    type Item = S::Item;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context) -> Poll<Option<Self::Item>> {
        // If the AtomicBool is set to true, this stream will be terminated
        if self.trigger.load(Ordering::Relaxed) {
            debug!("Terminating stream attached to Interrupt");
            Poll::Ready(None)
        } else {
            self.as_mut().stream().poll_next(cx)
        }
    }
}
