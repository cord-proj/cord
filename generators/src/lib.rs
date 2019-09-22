pub use simple::SimpleFactory;

use client::{
    errors::{Error, Result},
    Conn, Subscriber,
};
use futures::{
    future::{self, FutureResult},
    Future, Poll, Stream,
};
use message::Message;
use pattern_matcher::Pattern;
use std::{net::SocketAddr, time::Duration};
use stream::{delay, take_for};
use tokio;

mod simple;
mod stream;

const NUM_CLIENTS: u32 = 5;
const CONSUMERS_PER_CLIENT: u32 = 2;
const NUM_MESSAGES: u64 = 100;

type DecoratedStream = Box<dyn Stream<Item = Message, Error = Error> + Send>;

pub trait Factory {
    // XXX A more ergonomic approach is to return a Generator, however generators have
    // not been stabilised.
    // See: https://github.com/rust-lang/rust/issues/43122
    fn generate<F>(
        self,
        server_addr: SocketAddr,
        num_clients: u32,
        consumers_per_client: u32,
        num_messages: Option<u64>,
        decorator: F,
    ) -> Vec<Box<dyn Future<Item = Client, Error = Error>>>
    where
        F: Fn(DecoratedStream) -> DecoratedStream + 'static;
}

pub struct Executor<F>
where
    F: Factory + Send,
{
    factory: F,
    server_addr: SocketAddr,
    num_clients: u32,
    consumers_per_client: u32,
    frequency: Option<Duration>,
    duration: Option<Duration>,
    num_messages: Option<u64>,
}

pub struct Client {
    inner: Box<dyn Future<Item = (), Error = Error>>,
}

impl<F> Executor<F>
where
    F: Factory + Send + 'static,
{
    pub fn new(factory: F, server_addr: SocketAddr) -> Executor<F> {
        Executor {
            factory,
            server_addr,
            num_clients: NUM_CLIENTS,
            consumers_per_client: CONSUMERS_PER_CLIENT,
            frequency: Some(Duration::from_millis(200)),
            duration: None,
            num_messages: Some(NUM_MESSAGES),
        }
    }

    /// Set the number of clients we execute
    pub fn set_num_clients(&mut self, clients: u32) -> &mut Self {
        self.num_clients = clients;
        self
    }

    /// Set the number of consumers attached to each client
    pub fn set_consumers_per_client(&mut self, consumers: u32) -> &mut Self {
        self.consumers_per_client = consumers;
        self
    }

    /// Set how frequently the data stream will produce messages
    pub fn set_frequency(&mut self, delay: Duration) -> &mut Self {
        self.frequency = Some(delay);
        self
    }

    /// Set how long the data stream will produce messages for
    pub fn set_duration(&mut self, duration: Duration) -> &mut Self {
        self.duration = Some(duration);
        self.num_messages = None;
        self
    }

    /// Set how many messages the data stream will produce
    pub fn set_num_messages(&mut self, count: u64) -> &mut Self {
        self.num_messages = Some(count);
        self.duration = None;
        self
    }

    /// Run test executor
    pub fn exec(self) -> FutureResult<impl Future<Item = (), Error = Error>, Error> {
        // Destructure self so members can be sent safely across threads
        let frequency = self.frequency;
        let duration = self.duration;
        let num_messages = self.num_messages;

        let futs = self.factory.generate(
            self.server_addr,
            self.num_clients,
            self.consumers_per_client,
            num_messages,
            move |mut producer| {
                // Decorate this stream to set message frequency
                if let Some(f) = frequency {
                    producer = Box::new(delay::new(producer, f));
                }

                // Decorate this stream to limit the stream's duration
                if let Some(d) = duration {
                    producer = Box::new(take_for::new(producer, d));
                }

                // Decorate this stream to limit the number of messages in
                // the stream.
                if let Some(n) = num_messages {
                    producer = Box::new(producer.take(n));
                }

                producer
            },
        );

        future::ok(future::join_all(futs).map(|_| ()))
    }
}

impl Client {
    fn new<F, S>(
        mut conn: Conn,
        provider_namespace: Pattern,
        producer: S,
        subscribe_namespaces: Vec<Pattern>,
        acc: F,
    ) -> Result<Client>
    where
        F: Fn(Subscriber) -> Box<dyn Future<Item = (), Error = Error>>,
        S: Stream<Item = Message, Error = Error> + Send + 'static,
    {
        conn.provide(provider_namespace)?;

        let mut futs = vec![];
        for n in subscribe_namespaces {
            futs.push(acc(conn.subscribe(n)?));
        }

        tokio::spawn(conn.forward(producer).map(|_| ()).map_err(|_| ()));

        Ok(Client {
            inner: Box::new(future::join_all(futs).map(|_| ())),
        })
    }
}

impl Future for Client {
    type Item = ();
    type Error = Error;

    fn poll(&mut self) -> Poll<Self::Item, Self::Error> {
        self.inner.poll()
    }
}
