use super::Client;
use super::{DecoratedStream, Factory};
use client::{errors::Error, Conn};
use futures::{
    future::{self},
    Future, Stream,
};
use message::Message;
use pattern_matcher::Pattern;
use rand::{self, rngs::ThreadRng, Rng};
use std::{net::SocketAddr, result};
use tokio::prelude::Async;

const MESSAGE: &str = "message";
const NAMESPACE_LENGTH: u8 = 5;

pub struct SimpleFactory;

struct SimpleProducer {
    namespace: Pattern,
    message: String,
}

impl Factory for SimpleFactory {
    fn generate<F>(
        self,
        server_addr: SocketAddr,
        num_clients: u32,
        consumers_per_client: u32,
        num_messages: Option<u64>,
        decorator: F,
    ) -> Vec<Box<dyn Future<Item = Client, Error = Error>>>
    where
        F: Fn(DecoratedStream) -> DecoratedStream + 'static,
    {
        // Create seed data for namespace generation
        let seed = (b'A'..=b'z')
            .map(|c| c as char)
            .filter(|c| c.is_alphabetic())
            .collect::<Vec<_>>();

        let mut rng = rand::thread_rng();
        let mut namespaces = Vec::new();

        // Generate namespaces
        for _ in 0..num_clients {
            namespaces.push(gen_namespace(&mut rng, &seed));
        }

        let mut clients: Vec<Box<dyn Future<Item = Client, Error = Error>>> = Vec::new();
        for namespace in namespaces.iter() {
            let namespaces_c = namespaces.clone();
            let namespace_c = namespace.clone();
            clients.push(Box::new(Conn::new(server_addr).map(move |conn| {
                let subs = namespaces_c
                    .iter()
                    .filter(|n| *n != &namespace_c)
                    .cloned()
                    .collect();

                Client::new(
                    conn,
                    namespace_c.clone(),
                    decorator(Box::new(SimpleProducer::new(namespace_c, MESSAGE))),
                    subs,
                    |sub| {
                        Box::new(
                            sub.fold(0, |acc, _| Ok::<_, Error>(acc + 1))
                                .and_then(|acc| {
                                    if acc == 1 {
                                        future::ok(())
                                    } else {
                                        future::err("ABC".into())
                                    }
                                }),
                        )
                    },
                )
                .unwrap()
            })))
        }

        clients
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

fn gen_namespace(rng: &mut ThreadRng, seed: &[char]) -> Pattern {
    let seed_len = seed.len();
    let mut nsbuf = vec!['/'];
    for _ in 0..NAMESPACE_LENGTH {
        nsbuf.push(seed[rng.gen_range(0, seed_len)]);
    }

    let ns: String = nsbuf.into_iter().collect();
    ns.into()
}
