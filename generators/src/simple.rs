use super::{Client, Factory, MessageStream, CONSUMERS_PER_CLIENT};
use client::errors::Error;
use futures::{
    task::{Context, Poll},
    Stream, TryStreamExt,
};
use log::{debug, error, info};
use message::Message;
use pattern_matcher::Pattern;
use rand::{self, Rng};
use std::{
    net::SocketAddr,
    pin::Pin,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
};

const MESSAGE: &str = "message";
const NAMESPACE_LENGTH: u8 = 5;

pub struct SimpleFactory {
    seed: Vec<char>,
    consumers_per_client: u32,
    provider_ns: Vec<(Pattern, Arc<AtomicU64>)>,
    subscriber_ns: Vec<(Pattern, Arc<AtomicU64>)>,
}

struct SimpleProducer {
    namespace: Pattern,
    message: String,
    msg_count: Arc<AtomicU64>,
}

impl SimpleFactory {
    fn gen_namespace(&mut self) -> Pattern {
        let seed_len = self.seed.len();
        let mut nsbuf = vec!['/'];
        for _ in 0..NAMESPACE_LENGTH {
            // XXX `rand::thread_rng()` should be cached somehow, but isn't Sendable so
            // we can't include it in the SimpleFactory struct.
            nsbuf.push(self.seed[rand::thread_rng().gen_range(0, seed_len)]);
        }

        let ns: String = nsbuf.into_iter().collect();
        ns.into()
    }
}

impl Default for SimpleFactory {
    fn default() -> Self {
        // Create seed data for namespace generation
        let seed = (b'A'..=b'z')
            .map(|c| c as char)
            .filter(|c| c.is_alphabetic())
            .collect::<Vec<_>>();

        Self {
            seed,
            consumers_per_client: CONSUMERS_PER_CLIENT,
            provider_ns: Vec::new(),
            subscriber_ns: Vec::new(),
        }
    }
}

impl Factory for SimpleFactory {
    fn new_client<F>(&mut self, addr: SocketAddr, decorator: F) -> Client
    where
        F: Fn(MessageStream) -> MessageStream + 'static,
    {
        // Get or generate the provider namespace and counter for this client.
        // The counter is used to determine how many messages were sent vs. how
        // many messages were received.
        let (provider_ns, counter) = self
            .provider_ns
            .pop()
            .unwrap_or_else(|| (self.gen_namespace(), Arc::new(AtomicU64::new(0))));

        // Get or generate the subscriber namespaces for this client
        let mut subscriber_ns = self.subscriber_ns.clone();

        // We need a namespace per consumer. If `subscriber_ns` is not full,
        // top it up with new namespaces.
        for _ in subscriber_ns.len()..self.consumers_per_client as usize {
            let ns = self.gen_namespace();
            let c = Arc::new(AtomicU64::new(0));
            // For any new namespace we subscribe to, we also need a provider.
            // Thus we add every new namespace to the "needs provider" list.
            self.provider_ns.push((ns.clone(), c.clone()));
            subscriber_ns.push((ns, c));
        }

        info!(
            "Create new Client that provides {:?} and subscribes to {:?}",
            provider_ns,
            subscriber_ns
                .iter()
                .map(|s| &s.0)
                .collect::<Vec<&Pattern>>()
        );

        // The subscriber buffer should only ever contain as many namespaces as
        // there are consumers.
        if self.subscriber_ns.len() == self.consumers_per_client as usize {
            self.subscriber_ns.remove(0);
        }

        // Now that we are providing provider_ns, the next client can subscribe
        // to it.
        self.subscriber_ns
            .push((provider_ns.clone(), counter.clone()));

        Client::new(
            addr,
            provider_ns.clone(),
            decorator(Box::new(SimpleProducer::new(
                provider_ns,
                MESSAGE,
                counter.clone(),
            ))),
            subscriber_ns,
            move |subscriber, namespace, counter| {
                let namespace_c = namespace.clone();
                Box::new(
                    subscriber
                        .fold(counter.load(Ordering::Relaxed), move |acc, _| {
                            debug!("Subscriber {:?} received message {}", namespace_c, acc + 1);
                            Ok::<_, Error>(acc + 1)
                        })
                        .and_then(move |acc| {
                            let sent = counter.load(Ordering::Relaxed);
                            if acc == sent {
                                info!("Subscriber {:?} received all {} messages", namespace, acc);
                            } else {
                                error!(
                                    "Subscriber {:?} only received {} of {} messages",
                                    namespace, acc, sent
                                );
                            }
                            Ok(())
                        }),
                )
            },
        )
    }

    fn get_consumers_per_client(&self) -> u32 {
        self.consumers_per_client
    }

    fn set_consumers_per_client(&mut self, consumers_per_client: u32) {
        self.consumers_per_client = consumers_per_client;
    }
}

impl SimpleProducer {
    fn new<S: Into<String>>(
        namespace: Pattern,
        message: S,
        counter: Arc<AtomicU64>,
    ) -> SimpleProducer {
        SimpleProducer {
            namespace,
            message: message.into(),
            msg_count: counter,
        }
    }
}

impl Stream for SimpleProducer {
    type Item = Message;

    fn poll_next(self: Pin<&mut Self>, _: &mut Context) -> Poll<Option<Self::Item>> {
        // Increment the message count for later comparison
        let prev = self.msg_count.fetch_add(1, Ordering::Relaxed);
        debug!("Producer {:?} sending message {}", self.namespace, prev + 1);
        Poll::Ready(Some(Message::Event(
            self.namespace.clone(),
            self.message.clone(),
        )))
    }
}
