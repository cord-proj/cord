use futures::{Sink, Stream};
use std::time::{Duration, Instant};
use tokio::prelude::{Async, AsyncSink};

/// Stream combinator that delays each Stream::Item by a given duration.
pub struct Delay<S>
where
    S: Stream,
{
    stream: S,
    now: Instant,
    delay: Duration,
}

pub fn new<S>(stream: S, delay: Duration) -> Delay<S>
where
    S: Stream,
{
    Delay {
        stream,
        now: Instant::now(),
        delay,
    }
}

impl<S> Sink for Delay<S>
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

impl<S> Stream for Delay<S>
where
    S: Stream,
{
    type Item = S::Item;
    type Error = S::Error;

    fn poll(&mut self) -> Result<Async<Option<Self::Item>>, Self::Error> {
        // If we have not waited long enough, don't return a message
        if self.now.elapsed() < self.delay {
            Ok(Async::NotReady)
        } else {
            self.now = Instant::now();
            self.stream.poll()
        }
    }
}
