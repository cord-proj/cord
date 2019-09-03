use super::{DecoratedStream, Factory};
use client::{errors::Error, Conn};
use futures::{Future, Stream};
use message::Message;
use pattern_matcher::Pattern;
use rand::{self, rngs::ThreadRng, Rng};
use tokio::prelude::Async;

const MESSAGE: &str = "message";
const NAMESPACE_LENGTH: u8 = 5;

pub struct SimpleFactory {
    namespaces: Option<Vec<Pattern>>,
}

struct SimpleProducer {
    namespace: Pattern,
    message: String,
}

impl Factory for SimpleFactory {
    fn generate(&mut self, num_clients: u32, consumers_per_client: u32) {
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

        self.namespaces = Some(namespaces);
    }

    fn prepare<F>(
        &mut self,
        mut conn: Conn,
        decorator: F,
    ) -> Box<dyn Future<Item = Conn, Error = Error> + Send>
    where
        F: Fn(DecoratedStream) -> DecoratedStream,
    {
        // If self.namespaces = None, then the implementor has neglected to call
        // generate()
        assert!(self.namespaces.is_some());

        let namespace = self.namespaces.as_mut().unwrap().pop().unwrap();
        conn.provide(namespace.clone()).unwrap();
        let producer = decorator(Box::new(SimpleProducer {
            namespace,
            message: MESSAGE.into(),
        }));

        Box::new(conn.forward(producer))

        // conn.subscribe() each
        // Subscriber decorate
        // Subscriber tokio::exec()
        // return conn.forward(stream)
    }
}

impl Stream for SimpleProducer {
    type Item = Message;
    type Error = Error;

    fn poll(&mut self) -> Result<Async<Option<Self::Item>>, Self::Error> {
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
