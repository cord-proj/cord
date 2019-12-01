pub use simple::SimpleFactory;

use client::{
    errors::{Error, Result},
    Conn,
};
use futures::{
    compat::Future01CompatExt,
    future,
    task::{Context, Poll},
    Future, FutureExt, Stream, StreamExt, TryFuture, TryFutureExt, TryStream, TryStreamExt,
};
use log::{debug, error, info};
use message::Message;
use pattern_matcher::Pattern;
use std::{
    fmt,
    marker::Unpin,
    net::SocketAddr,
    pin::Pin,
    result,
    sync::{atomic::AtomicU64, Arc},
    time::Duration,
};
use stream::{delay, interruptable::Interrupt, take_for};
use tokio;

mod simple;
mod stream;

const NUM_CLIENTS: u32 = 5;
const CONSUMERS_PER_CLIENT: u32 = 2;
const NUM_MESSAGES: u64 = 100;
const TERMINATION_GRACE: u64 = 1000; // in milliseconds

type MessageStream = Box<dyn TryStream<Ok = Message, Error = Error> + Send + Unpin>;

pub trait Factory {
    fn new_client<F>(&mut self, addr: SocketAddr, decorator: F) -> Client
    where
        F: Fn(MessageStream) -> MessageStream + 'static;
    fn get_consumers_per_client(&self) -> u32;
    fn set_consumers_per_client(&mut self, consumers_per_client: u32);
}

pub struct Executor<F>
where
    F: Factory + Send,
{
    factory: F,
    server_addr: SocketAddr,
    num_clients: u32,
    frequency: Option<Duration>,
    duration: Option<Duration>,
    num_messages: Option<u64>,
}

pub struct Client {
    inner: Box<dyn TryFuture<Ok = (), Error = Error> + Send + Unpin>,
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
        self.factory.set_consumers_per_client(consumers);
        self
    }

    /// Set how frequently the data stream will produce messages
    pub fn set_frequency(&mut self, delay: Option<Duration>) -> &mut Self {
        self.frequency = delay;
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
    pub fn exec(mut self) -> impl TryFuture<Ok = (), Error = Error> {
        // Destructure self so members can be copied across threads
        let frequency = self.frequency;
        let duration = self.duration;
        let num_messages = self.num_messages;

        let mut futs = Vec::new();
        for _ in 0..self.num_clients {
            futs.push(
                self.factory
                    .new_client(self.server_addr, move |mut producer| {
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
                            producer = Box::new(producer.take(n as usize));
                        }

                        producer
                    }),
            );
        }

        future::join_all(futs).map(|_| ())
    }
}

impl<F> fmt::Display for Executor<F>
where
    F: Factory + Send,
{
    fn fmt(&self, f: &mut fmt::Formatter) -> result::Result<(), fmt::Error> {
        let time = if let Some(duration) = self.duration {
            format!("for {:?}", duration)
        } else if let Some(messages) = self.num_messages {
            format!("{} times", messages)
        } else {
            unreachable!();
        };

        let duration = if let Some(frequency) = self.frequency {
            format!(" every {:?}", frequency)
        } else {
            "".into()
        };
        write!(
            f,
            "Testing {} clients with {} consumers each, sending messages{} {}",
            self.num_clients,
            self.factory.get_consumers_per_client(),
            duration,
            time
        )
    }
}

impl Client {
    fn new<F>(
        addr: SocketAddr,
        provider_namespace: Pattern,
        producer: MessageStream,
        subscribe_namespaces: Vec<(Pattern, Arc<AtomicU64>)>,
        accumulator: F,
    ) -> Client
    where
        F: Fn(
                Box<dyn Stream<Item = Result<(Pattern, String)>> + Send + Unpin>,
                Pattern,
                Arc<AtomicU64>,
            ) -> Box<dyn Future<Output = Result<()>> + Send + Unpin>
            + Send
            + 'static,
    {
        // Setup interrupt to terminate subscribers after producer is finished.
        let interrupt = Interrupt::new();

        // Connect to the server and start sending/receiving
        let fut = Conn::new(addr).compat().and_then(move |mut conn| {
            // Register the namespace we provide
            info!("{} providing namespace {:?}", conn, provider_namespace);
            conn.provide(provider_namespace)
                .expect("Cannot send PROVIDE message to server");

            // Subscribe to foreign namespaces and run streams to exhaustion.
            let mut futs = vec![];
            for (n, c) in subscribe_namespaces {
                // Subscribe to foreign namespace
                info!("{} subscribing to namespace {:?}", conn, n);
                let sub = conn
                    .subscribe(n.clone())
                    .expect("Cannot send SUBSCRIBE message to server");

                // Attach this subscriber stream to the Interrupt
                let interruptable = interrupt.attach(sub);

                // Accumulate the stream to test results
                let fut = accumulator(Box::new(interruptable), n, c);

                // Push the accumulator future onto the heap
                futs.push(fut);
            }

            tokio::spawn(
                conn.forward(producer)
                    .and_then(move |conn| {
                        debug!("Terminating consumers for {}", conn);
                        interrupt
                            .after(Duration::from_millis(TERMINATION_GRACE))
                            .map_err(|e| e.to_string().into())
                    })
                    .map_err(|e| error!("{}", e)),
            );

            future::join_all(futs).map(|_| ())
        });

        Client {
            inner: Box::new(fut),
        }
    }
}

impl TryFuture for Client {
    type Ok = ();
    type Error = Error;

    fn try_poll(
        self: Pin<&mut Self>,
        cx: &mut Context,
    ) -> Poll<result::Result<Self::Ok, Self::Error>> {
        unsafe { Pin::map_unchecked_mut(self.as_mut(), |x| &mut x.inner).try_poll(cx) }
    }
}
