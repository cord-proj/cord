use client::{
    errors::{Error, ErrorKind, Result},
    Conn, Subscriber,
};
use futures::{future, Future, IntoFuture, Stream};
use message::Message;
use pattern_matcher::Pattern;
use std::{
    borrow::Cow,
    iter::Iterator,
    net::SocketAddr,
    time::{Duration, Instant},
};
use stream::{delay, take_for};
use tokio::{self, timer::Delay};

// mod simple;
mod stream;

const NUM_CLIENTS: u32 = 5;
const CONSUMERS_PER_CLIENT: u32 = 2;
const NUM_MESSAGES: u64 = 100;

type DecoratedStream = Box<dyn Stream<Item = Message, Error = Error>>;

pub trait Factory {
    fn set_server_addr(&mut self, server_addr: SocketAddr) -> &mut Self;
    fn set_num_clients(&mut self, num_clients: u32) -> &mut Self;
    fn set_consumers_per_client(&mut self, consumers_per_client: u32) -> &mut Self;
    // XXX A more ergonomic approach is to return a Generator, however generators have
    // not been stabilised.
    // See: https://github.com/rust-lang/rust/issues/43122
    fn generate<F>(
        self,
        decorator: F,
    ) -> Box<dyn Iterator<Item = Box<dyn Future<Item = Conn, Error = Error>>>>
    where
        F: Fn(DecoratedStream) -> DecoratedStream;
}

pub struct Executor<F>
where
    F: Factory + Send,
{
    factory: F,
    frequency: Option<Duration>,
    duration: Option<Duration>,
    num_messages: Option<u64>,
}

struct Client {
    inner: Box<dyn Future<Item = (), Error = Error>>,
}

impl<F> Executor<F>
where
    F: Factory + Send + 'static,
{
    pub fn new(mut factory: F, server_addr: SocketAddr) -> Executor<F> {
        factory
            .set_server_addr(server_addr)
            .set_num_clients(NUM_CLIENTS)
            .set_consumers_per_client(CONSUMERS_PER_CLIENT);

        Executor {
            factory,
            frequency: Some(Duration::from_millis(200)),
            duration: None,
            num_messages: Some(NUM_MESSAGES),
        }
    }

    /// Set the number of clients we execute
    pub fn set_num_clients(&mut self, clients: u32) -> &mut Self {
        self.factory.set_num_clients(clients);
        self
    }

    /// Set the number of consumers attached to each client
    pub fn set_consumers_per_client(&mut self, consumers: u32) -> &mut Self {
        self.factory.set_consumers_per_client(consumers);
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

    pub fn exec(self) -> impl Future<Item = (), Error = Error> {
        // Destructure self so members can be sent safely across threads
        let frequency = self.frequency;
        let duration = self.duration;
        let num_messages = self.num_messages;

        let futs: Vec<Box<dyn Future<Item = Conn, Error = Error>>> = self
            .factory
            .generate(|mut producer| {
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
            })
            .collect();
        future::join_all(futs).map(|_| ())
    }
}

//
// XXX
// `Client::new` should take one acc argument, `acc: Fn(Stream) -> Future`
// `Client` must implement Future
// `Executor` should handle set_* fns
// `Factory::generate` should return Clients
// Move to own module
//

impl Client {
    fn new<F, N, S, T>(
        mut conn: Conn,
        provider_namespace: Pattern,
        producer: S,
        subscribe_namespaces: Vec<Pattern>,
        acc_init: T,
        acc_fn: N,
        acc_expected: T,
        acc_description: String,
    ) -> Result<Client>
    where
        F: IntoFuture<Item = T, Error = Error> + 'static,
        N: Clone + FnMut(T, (Pattern, String)) -> F + 'static,
        S: Stream<Item = Message, Error = Error> + Send + 'static,
        T: Clone + PartialEq + 'static,
    {
        conn.provide(provider_namespace)?;

        let mut futs = vec![];
        for n in subscribe_namespaces {
            let acc_expected_c = acc_expected.clone();
            let acc_description_c = acc_description.clone();
            futs.push(
                conn.subscribe(n)
                    .unwrap()
                    .map_err(|e| panic!(e))
                    .fold(acc_init.clone(), acc_fn.clone())
                    .and_then(move |acc| {
                        if acc == acc_expected_c {
                            Ok(())
                        } else {
                            Err(ErrorKind::Msg(acc_description_c).into())
                        }
                    }),
            );
        }

        tokio::spawn(conn.forward(producer).map(|_| ()).map_err(|_| ()));

        Ok(Client {
            inner: Box::new(future::join_all(futs).map(|_| ())),
        })
    }
}
