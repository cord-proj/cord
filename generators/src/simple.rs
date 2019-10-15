use super::{Client, Factory, MessageStream, CONSUMERS_PER_CLIENT};
use client::errors::{Error, ErrorKind};
use futures::{Future, Stream};
use message::Message;
use pattern_matcher::Pattern;
use rand::{self, rngs::ThreadRng, Rng};
use std::{net::SocketAddr, result};
use tokio::prelude::Async;

const MESSAGE: &str = "message";
const NAMESPACE_LENGTH: u8 = 5;

pub struct SimpleFactory {
    seed: Vec<char>,
    rng: ThreadRng,
    consumers_per_client: u32,
    provider_ns: Vec<Pattern>,
    subscriber_ns: Vec<Pattern>,
}

struct SimpleProducer {
    namespace: Pattern,
    message: String,
}

impl SimpleFactory {
    fn gen_namespace(&mut self) -> Pattern {
        let seed_len = self.seed.len();
        let mut nsbuf = vec!['/'];
        for _ in 0..NAMESPACE_LENGTH {
            nsbuf.push(self.seed[self.rng.gen_range(0, seed_len)]);
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
            rng: rand::thread_rng(),
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
        let provider_ns = self
            .provider_ns
            .pop()
            .unwrap_or_else(|| self.gen_namespace());
        let subscriber_ns = Vec::new();

        // The subscriber buffer should only ever contain as many namespaces as
        // there are consumers.
        if self.subscriber_ns.len() == self.consumers_per_client as usize {
            self.subscriber_ns.remove(0);
        }

        // Now that we are providing provider_ns, the next client can subscribe
        // to it.
        self.subscriber_ns.push(provider_ns.clone());
        Client::new(
            addr,
            provider_ns.clone(),
            decorator(Box::new(SimpleProducer::new(provider_ns, MESSAGE))),
            subscriber_ns,
            |subscriber| {
                Box::new(
                    subscriber
                        .fold(0, |acc, _| Ok::<_, Error>(acc + 1))
                        .and_then(|acc| {
                            if acc == 1 {
                                Ok(())
                            } else {
                                Err(Error::from_kind(ErrorKind::Msg(format!(
                                    "Expected to receive {} messages, got {}",
                                    1, acc
                                ))))
                            }
                        }),
                )
            },
        )
    }

    fn set_consumers_per_client(&mut self, consumers_per_client: u32) {
        self.consumers_per_client = consumers_per_client;
    }
}

impl SimpleProducer {
    fn new<S: Into<String>>(namespace: Pattern, message: S) -> SimpleProducer {
        SimpleProducer {
            namespace,
            message: message.into(),
        }
    }
}

impl Stream for SimpleProducer {
    type Item = Message;
    type Error = Error;

    fn poll(&mut self) -> result::Result<Async<Option<Self::Item>>, Self::Error> {
        Ok(Async::Ready(Some(Message::Event(
            self.namespace.clone(),
            self.message.clone(),
        ))))
    }
}
