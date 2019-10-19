use crate::errors::Error;
use futures::{self, future, Future};
use log::debug;
use message::Message;
use tokio::sync::mpsc::UnboundedSender;

type SomeFuture = Box<dyn Future<Item = (), Error = Error> + Send>;

pub enum Subscriber {
    Consumer(UnboundedSender<Message>),
    // @todo Replace trait object with impl Trait once stabilised:
    // https://github.com/rust-lang/rust/issues/34511
    // Task(Box<FnMut(Message, PublisherHandle) -> impl Future<Item=(), Error=()>>)
    Task(Box<dyn FnMut(Message) -> SomeFuture + Send>),
    OnetimeTask(Option<Box<dyn FnOnce(Message) -> SomeFuture + Send>>),
}

impl Subscriber {
    pub fn recv(&mut self, message: Message) -> (bool, SomeFuture) {
        match self {
            // Retain subscriber in map if the channel is ok
            Subscriber::Consumer(ref mut chan) => {
                debug!(target: "subscriber", "Receive message for consumer: {:?}", message);
                (chan.try_send(message).is_ok(), Box::new(future::ok(())))
            }
            Subscriber::Task(f) => {
                debug!(target: "subscriber", "Receive message for task: {:?}", message);
                (true, f(message)) // Retain subscriber in map
            }
            Subscriber::OnetimeTask(opt) => {
                debug!(target: "subscriber", "Receive message for one-time task: {:?}", message);
                (
                    false, // Don't retain subscriber in map
                    opt.take().expect("OnetimeTask already executed")(message),
                )
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::{future, stream::Stream};
    use lazy_static::lazy_static;
    use tokio::{
        runtime::Runtime,
        sync::{mpsc, oneshot},
    };

    use std::sync::Mutex as StdMutex;

    lazy_static! {
        static ref RUNTIME: StdMutex<TokioRuntime> = StdMutex::new(TokioRuntime::new());
    }

    // Because each test requires a Tokio runtime, the test pack can exhaust the kernel's
    // file limit. To mitigate this issue, we use a shared runtime to limit the number of
    // open file descriptors.
    struct TokioRuntime {
        runtime: Option<Runtime>,
        users: u16,
    }

    impl TokioRuntime {
        pub fn new() -> TokioRuntime {
            TokioRuntime {
                runtime: None,
                users: 0,
            }
        }

        pub fn start(&mut self) {
            if self.runtime.is_none() {
                self.runtime = Some(Runtime::new().unwrap());
            }

            self.users += 1;
        }

        pub fn stop(&mut self) {
            self.users -= 1;

            if self.users == 0 && self.runtime.is_some() {
                self.runtime
                    .take()
                    .unwrap()
                    .shutdown_on_idle()
                    .wait()
                    .unwrap();
            }
        }

        pub fn block_on<F>(&mut self, future: F)
        where
            F: Send + 'static + Future<Item = (), Error = ()>,
        {
            self.runtime.as_mut().unwrap().block_on(future).unwrap();
        }
    }

    #[test]
    fn test_subscriber_recv_consumer() {
        RUNTIME.lock().unwrap().start();

        let (tx, rx) = mpsc::unbounded_channel();
        let (txo, mut rxo) = oneshot::channel();
        let message = Message::Event("/a".into(), "abc".into());
        let message_c = message.clone();

        let mut consumer = Subscriber::Consumer(tx);
        let (retain, _) = consumer.recv(message);
        assert!(retain);

        RUNTIME.lock().unwrap().block_on(
            rx.into_future()
                .and_then(|(msg, _)| {
                    txo.send(msg.unwrap()).unwrap();
                    future::ok(())
                })
                .map_err(|_| ()),
        );
        assert_eq!(rxo.try_recv().unwrap(), message_c);

        RUNTIME.lock().unwrap().stop();
    }

    #[test]
    fn test_subscriber_recv_task() {
        RUNTIME.lock().unwrap().start();

        let (tx, mut rx) = oneshot::channel();
        let mut tx = Some(tx);
        let message = Message::Event("/a".into(), "abc".into());
        let message_c = message.clone();

        let mut consumer = Subscriber::Task(Box::new(move |msg| {
            tx.take().unwrap().send(msg).unwrap();
            Box::new(future::ok(()))
        }));
        RUNTIME.lock().unwrap().block_on(future::lazy(move || {
            consumer.recv(message);
            future::ok(())
        }));
        assert_eq!(rx.try_recv().unwrap(), message_c);

        RUNTIME.lock().unwrap().stop();
    }

    #[test]
    fn test_subscriber_recv_onetime_task() {
        RUNTIME.lock().unwrap().start();

        let (tx, mut rx) = oneshot::channel();
        let message = Message::Event("/a".into(), "abc".into());
        let message_c = message.clone();

        let mut consumer = Subscriber::OnetimeTask(Some(Box::new(move |msg| {
            tx.send(msg).unwrap();
            Box::new(future::ok(()))
        })));
        RUNTIME.lock().unwrap().block_on(future::lazy(move || {
            consumer.recv(message);
            future::ok(())
        }));
        assert_eq!(rx.try_recv().unwrap(), message_c);

        RUNTIME.lock().unwrap().stop();
    }
}
