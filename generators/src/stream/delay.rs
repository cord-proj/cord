use futures::{Future, Sink, Stream};
use std::time::{Duration, Instant};
use tokio::{
    prelude::{Async, AsyncSink},
    timer,
};

/// Stream combinator that delays each Stream::Item by a given duration.
pub struct Delay<S>
where
    S: Stream,
{
    stream: S,
    duration: Duration,
    delay: Option<timer::Delay>,
}

pub fn new<S>(stream: S, duration: Duration) -> Delay<S>
where
    S: Stream,
{
    Delay {
        stream,
        duration,
        delay: None,
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
    S::Error: From<String>,
{
    type Item = S::Item;
    type Error = S::Error;

    fn poll(&mut self) -> Result<Async<Option<Self::Item>>, Self::Error> {
        let duration = self.duration;
        let delay = self
            .delay
            .get_or_insert_with(|| timer::Delay::new(Instant::now() + duration));

        match delay.poll() {
            Ok(Async::Ready(_)) => {
                self.delay = None;
                self.stream.poll()
            }
            Ok(Async::NotReady) => Ok(Async::NotReady),
            Err(e) => Err(format!("{}", e).into()),
        }
    }
}
