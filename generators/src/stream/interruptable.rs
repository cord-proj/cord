use futures::Future;
use futures::{Sink, Stream};
use log::debug;
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};
use tokio::{
    prelude::{Async, AsyncSink},
    timer::{Delay, Error as DelayError},
};

type Trigger = Arc<AtomicBool>;

/// A trigger to terminate any Interruptable streams attached to it.
pub struct Interrupt(Trigger);

/// Stream combinator that terminates a stream when the trigger is pulled.
pub struct Interruptable<S>
where
    S: Stream,
{
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
    pub fn after(self, duration: Duration) -> impl Future<Item = (), Error = DelayError> {
        debug!("Terminating attached streams in {:?}", duration);
        Delay::new(Instant::now() + duration).and_then(|_| {
            self.now();
            Ok(())
        })
    }
}

impl<S> Sink for Interruptable<S>
where
    S: Sink + Stream,
{
    type SinkItem = S::SinkItem;
    type SinkError = S::SinkError;

    fn start_send(
        &mut self,
        item: S::SinkItem,
    ) -> Result<AsyncSink<Self::SinkItem>, Self::SinkError> {
        self.stream.start_send(item)
    }

    fn poll_complete(&mut self) -> Result<Async<()>, Self::SinkError> {
        self.stream.poll_complete()
    }

    fn close(&mut self) -> Result<Async<()>, Self::SinkError> {
        self.stream.close()
    }
}

impl<S> Stream for Interruptable<S>
where
    S: Stream,
{
    type Item = S::Item;
    type Error = S::Error;

    fn poll(&mut self) -> Result<Async<Option<Self::Item>>, Self::Error> {
        // If the AtomicBool is set to true, this stream will be terminated
        if self.trigger.load(Ordering::Relaxed) {
            debug!("Terminating stream attached to Interrupt");
            Ok(Async::Ready(None))
        } else {
            self.stream.poll()
        }
    }
}
