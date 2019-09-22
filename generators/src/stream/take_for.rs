use futures::{Sink, Stream};
use std::time::{Duration, Instant};
use tokio::prelude::{Async, AsyncSink};

/// Stream combinator that terminates a stream after a given duration.
pub struct TakeFor<S>
where
    S: Stream,
{
    stream: S,
    now: Instant,
    duration: Duration,
}

pub fn new<S>(stream: S, duration: Duration) -> TakeFor<S>
where
    S: Stream,
{
    TakeFor {
        stream,
        now: Instant::now(),
        duration,
    }
}

impl<S> Sink for TakeFor<S>
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

impl<S> Stream for TakeFor<S>
where
    S: Stream,
{
    type Item = S::Item;
    type Error = S::Error;

    fn poll(&mut self) -> Result<Async<Option<Self::Item>>, Self::Error> {
        // While our function has not been running for long enough, continue polling the
        // underlying stream. Otherwise, terminate the stream.
        if self.now.elapsed() < self.duration {
            self.stream.poll()
        } else {
            Ok(Async::Ready(None))
        }
    }
}
