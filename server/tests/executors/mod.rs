use client::{errors::Error, Conn};
use futures::{future, Future, Stream};
use futures_locks::Mutex;
use message::Message;
use std::{net::SocketAddr, time::Duration};
use stream::{delay, take_for};

mod simple;
mod stream;

const NUM_CLIENTS: u32 = 5;
const CONSUMERS_PER_CLIENT: u32 = 2;
const NUM_MESSAGES: u64 = 100;

type DecoratedStream = Box<dyn Stream<Item = Message, Error = Error>>;

pub trait Factory {
    fn generate(&mut self, num_clients: u32, consumers_per_client: u32);
    fn prepare<F>(
        &mut self,
        conn: Conn,
        decorator: F,
    ) -> Box<dyn Future<Item = Conn, Error = Error> + Send>
    where
        F: Fn(DecoratedStream) -> DecoratedStream;
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

    pub fn exec(mut self) -> impl Future<Item = (), Error = ()> {
        self.factory
            .generate(self.num_clients, self.consumers_per_client);

        // Enclose factory in a Mutex so that it can be shared safely across threads
        let factory = Mutex::new(self.factory);
        let frequency = self.frequency;
        let duration = self.duration;
        let num_messages = self.num_messages;

        let mut futs = Vec::new();
        for _ in 0..self.num_clients {
            let factory_c = factory.clone();
            let f = Conn::new(self.server_addr).and_then(move |conn| {
                factory_c
                    .with(move |mut guard| {
                        (*guard).prepare(conn, |mut producer| {
                            // Decorate this stream to set message frequency
                            if let Some(d) = frequency {
                                producer = Box::new(delay::new(producer, d));
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
                        })
                    })
                    .expect("The default executor has shut down")
            });
            futs.push(f);
        }

        future::join_all(futs).map(|_| ()).map_err(|e| panic!(e))
    }
}
