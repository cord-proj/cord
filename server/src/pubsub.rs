use crate::{
    errors::{Error, ErrorKind},
    message::{Message, MessageCodec},
};
use futures::{self, future, Future, Stream};
use futures_locks::Mutex;
use pattern_matcher::Pattern;
use tokio::{
    codec::Framed,
    net::TcpStream,
    sync::mpsc::{self, UnboundedSender},
};
use uuid::Uuid;

use std::collections::{hash_map::Entry, HashMap};

type SomeFuture = Box<dyn Future<Item = (), Error = Error> + Send>;

#[derive(Clone)]
pub struct PublisherHandle {
    // This is a channel to the write half of our TcpStream
    consumer: UnboundedSender<Message>,
    subscribers: Mutex<HashMap<Message, HashMap<Uuid, Subscriber>>>,
}

enum Subscriber {
    Consumer(UnboundedSender<Message>),
    // @todo Replace trait object with impl Trait once stabilised:
    // https://github.com/rust-lang/rust/issues/34511
    // Task(Box<FnMut(Message, PublisherHandle) -> impl Future<Item=(), Error=()>>)
    Task(Box<FnMut(Message) -> SomeFuture + Send>),
    OnetimeTask(Option<Box<FnOnce(Message) -> SomeFuture + Send>>),
}

impl PublisherHandle {
    fn new(consumer: UnboundedSender<Message>) -> PublisherHandle {
        PublisherHandle {
            consumer,
            subscribers: Mutex::new(HashMap::new()),
        }
    }

    // Link two publishers together so that they can subscribe to each other's streams
    pub fn link(&self, other: &PublisherHandle) -> impl Future<Item = (), Error = Error> {
        let me = self.clone();
        let you = other.clone();

        self.on_link(you).join(other.on_link(me)).map(|_| ())
    }

    // Helper function for subscribing another publisher to our PROVIDE and REVOKE
    // messages
    fn on_link(&self, other: PublisherHandle) -> impl Future<Item = (), Error = Error> {
        let me = self.clone();
        let me_two = self.clone();

        // Subscribe to PROVIDE messages
        let (_, futp) = self.subscribe(
            Message::Provide("*".into()),
            Subscriber::Task(Box::new(move |message| {
                Box::new(other.on_provide(message.unwrap_provide(), me.clone()))
            })),
        );

        // Subscribe to REVOKE messages
        // We use this task to bulk-remove all subscribers that have subscribed to the
        // revoked namespace.
        let (_, futr) =
            self.subscribe(
                Message::Revoke("*".into()),
                Subscriber::Task(Box::new(move |message| {
                    Box::new(me_two.unsubscribe_children(Message::Event(
                        message.unwrap_revoke(),
                        String::new(),
                    )))
                })),
            );

        futp.join(futr).map(|_| ())
    }

    // Helper function for subscribing another publisher to our SUBSCRIBE messages
    fn on_provide(
        &self,
        namespace: Pattern,
        other: PublisherHandle,
    ) -> impl Future<Item = (), Error = Error> {
        let me = self.clone();
        let me_two = self.clone();
        let other_clone = other.clone();
        let namespace_clone = namespace.clone();

        // Add a subscription for any SUBSCRIBE messages from our client that match the
        // other publisher's namespace. I.e. if our client wants to subscribe to /a and
        // the other publisher provides /a, forward our client's request to the other
        // publisher. The other publisher will then start sending /a messages to our
        // client.
        let (uuid, futs) = self.subscribe(
            Message::Subscribe(namespace),
            Subscriber::Task(Box::new(move |message| {
                Box::new(other.on_subscribe(message.unwrap_subscribe(), me_two.clone()))
            })),
        );

        // Add a subscription for any REVOKE messages from the other publisher. If the
        // other provider revokes the namespace, we don't want to keep listening for
        // SUBSCRIBE messages to it.
        let (_, futr) = other_clone.subscribe(
            Message::Revoke(namespace_clone),
            Subscriber::OnetimeTask(Some(Box::new(move |message| {
                Box::new(me.unsubscribe(Message::Subscribe(message.unwrap_revoke()), uuid))
            }))),
        );

        futs.join(futr).map(|_| ())
    }

    // Helper function for subscribing another publisher's consumer to our EVENT messages
    fn on_subscribe(
        &self,
        namespace: Pattern,
        other: PublisherHandle,
    ) -> impl Future<Item = (), Error = Error> {
        let me = self.clone();
        let namespace_clone = namespace.clone();
        let consumer = other.consumer.clone();

        // Subscribe a consumer to our EVENT messages matching `namespace`
        let (uuid, fute) = self.subscribe(
            Message::Event(namespace, String::new()),
            Subscriber::Consumer(consumer),
        );

        // Add a subscription for any UNSUBSCRIBE messages from the other publisher. If
        // the other publisher receives a request to unsubscribe this namespace, the task
        // below will trigger it.
        let (_, futu) = other.subscribe(
            Message::Unsubscribe(namespace_clone),
            Subscriber::OnetimeTask(Some(Box::new(move |message| {
                Box::new(me.unsubscribe(
                    Message::Event(message.unwrap_unsubscribe(), String::new()),
                    uuid,
                ))
            }))),
        );

        fute.join(futu).map(|_| ())
    }

    // Add a Subscriber for a specific Message to our subscriber list
    fn subscribe(
        &self,
        message: Message,
        subscriber: Subscriber,
    ) -> (Uuid, impl Future<Item = (), Error = Error>) {
        let uuid = Uuid::new_v4();

        (
            uuid,
            self.subscribers
                .with(move |mut guard| {
                    (*guard)
                        .entry(message)
                        .or_insert_with(HashMap::new)
                        .insert(uuid, subscriber);
                    future::ok(())
                })
                .expect("The default executor has shut down"),
        )
    }

    // Remove a Subscriber for a specific Message from our subscriber list
    fn unsubscribe(&self, message: Message, sub_id: Uuid) -> impl Future<Item = (), Error = Error> {
        self.subscribers
            .with(move |mut guard| {
                if let Entry::Occupied(mut o) = (*guard).entry(message) {
                    let e = o.get_mut();
                    e.remove(&sub_id);

                    // If that was the last subscriber for a particular message, delete
                    // the message so it doesn't add more overhead to the routing loop.
                    if e.is_empty() {
                        o.remove();
                    }
                }

                future::ok(())
            })
            .expect("The default executor has shut down")
    }

    // Remove all Subscribers that are are contained within a broader namespace.
    // For example: Subscribe(/a) contains Subscribe(/a/b), so Subscribe(/a/b) would be
    // removed.
    fn unsubscribe_children(&self, message: Message) -> impl Future<Item = (), Error = Error> {
        self.subscribers
            .with(move |mut guard| {
                (*guard).retain(|k, _| !message.contains(&k));
                future::ok(())
            })
            .expect("The default executor has shut down")
    }

    fn route(&self, message: Message) -> impl Future<Item = (), Error = Error> {
        self.subscribers
            .with(move |mut guard| {
                let mut futs = vec![];

                // In the inner loop we may cleanup all of the subscribers in the
                // `subs` map. This leaves us with an empty map that should also
                // be tidied up, hence the use of retain() here.
                (*guard).retain(|sub_msg, subs| {
                    if sub_msg.contains(&message) {
                        // We use retain() here, which allows us to cleanup any
                        // subscribers that have OnetimeTask's or whose channels
                        // have been closed.
                        subs.retain(|_, sub| {
                            let (retain, f) = sub.recv(message.clone());
                            futs.push(f);
                            retain
                        });
                        !subs.is_empty() // Retain if map contains subscribers
                    } else {
                        true // Retain as we haven't modified the map
                    }
                });

                future::join_all(futs).map(|_| ())
            })
            .expect("The default executor has shut down")
    }
}

impl Subscriber {
    fn recv(&mut self, message: Message) -> (bool, SomeFuture) {
        match self {
            // Retain subscriber in map if the channel is ok
            Subscriber::Consumer(ref mut chan) => {
                (chan.try_send(message).is_ok(), Box::new(future::ok(())))
            }
            Subscriber::Task(f) => {
                (true, f(message)) // Retain subscriber in map
            }
            Subscriber::OnetimeTask(opt) => {
                (
                    false, // Don't retain subscriber in map
                    opt.take().expect("OnetimeTask already executed")(message),
                )
            }
        }
    }
}

pub fn new(socket: TcpStream) -> PublisherHandle {
    let framed = Framed::new(socket, MessageCodec::new());
    let (sink, stream) = framed.split();

    // Create a channel for the consumer (sink) half of the socket. This allows us to
    // pass clones of the channel to multiple producers to facilitate a consumer's
    // subscriptions.
    let (consumer_tx, consumer_rx) = mpsc::unbounded_channel();

    // Spawn task to drain channel receiver into socket sink
    tokio::spawn(
        consumer_rx
            .map_err(|e| Error::from_kind(ErrorKind::ChanRecv(e)))
            .forward(sink)
            .map(|_| ())
            .map_err(|_| ()),
    );

    // Create a clonable publisher handle so we can control the publisher from afar
    let handle = PublisherHandle::new(consumer_tx);
    let handle_c = handle.clone();

    // This is the main routing task. For each message we receive, find all the
    // subscribers that match it, then pass each a copy via `recv()`.
    tokio::spawn(
        stream
            .for_each(move |message| handle.route(message))
            .map_err(|_| ()),
    );

    handle_c
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::future;
    use futures_locks::Mutex;
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

    fn add_message(
        message: Message,
        message_subs: HashMap<Uuid, Subscriber>,
        subs: &mut HashMap<Message, HashMap<Uuid, Subscriber>>,
    ) -> Message {
        let message_c = message.clone();
        subs.insert(message, message_subs);
        message_c
    }

    fn add_message_sub(map: &mut HashMap<Uuid, Subscriber>) -> Uuid {
        let uuid = Uuid::new_v4();
        map.insert(
            uuid,
            Subscriber::Task(Box::new(|_| Box::new(future::ok(())))),
        );
        uuid
    }

    fn add_message_sub_checked(map: &mut HashMap<Uuid, Subscriber>) -> (Uuid, Mutex<bool>) {
        let uuid = Uuid::new_v4();
        let mutex = Mutex::new(false);
        let mutex_c = mutex.clone();

        map.insert(
            uuid,
            Subscriber::Task(Box::new(move |_| {
                Box::new(
                    mutex
                        .with(|mut guard| {
                            (*guard) = true;
                            future::ok(())
                        })
                        .unwrap(),
                )
            })),
        );
        (uuid, mutex_c)
    }

    fn add_message_sub_once_checked(map: &mut HashMap<Uuid, Subscriber>) -> (Uuid, Mutex<bool>) {
        let uuid = Uuid::new_v4();
        let mutex = Mutex::new(false);
        let mutex_c = mutex.clone();

        map.insert(
            uuid,
            Subscriber::OnetimeTask(Some(Box::new(move |_| {
                Box::new(
                    mutex
                        .with(|mut guard| {
                            (*guard) = true;
                            future::ok(())
                        })
                        .unwrap(),
                )
            }))),
        );
        (uuid, mutex_c)
    }

    // XXX This function should mock the subscribe and unsubscribe functions. However as
    // we can't yet return `impl Trait` from a Trait, we can't create a mock impl; it
    // would require that every function returned a trait object instead.
    // https://github.com/rust-lang/rfcs/pull/2515
    #[test]
    fn test_on_link() {
        RUNTIME.lock().unwrap().start();

        let s1 = Mutex::new(HashMap::new());
        let (tx, _) = mpsc::unbounded_channel();
        let h1 = PublisherHandle {
            consumer: tx,
            subscribers: s1.clone(),
        };

        let (tx, _) = mpsc::unbounded_channel();
        let h2 = PublisherHandle {
            consumer: tx,
            subscribers: Mutex::new(HashMap::new()),
        };

        // All this extra fluff around future::lazy() is necessary to ensure that there
        // is an active executor when the fn calls Mutex::with().
        RUNTIME.lock().unwrap().block_on(future::lazy(move || {
            h1.on_link(h2).map_err(|e| {
                dbg!(e);
            })
        }));

        let subs = s1.try_lock().ok().unwrap();

        let m1 = Message::Provide("*".into());
        assert_eq!(subs[&m1].len(), 1);

        let m2 = Message::Revoke("*".into());
        assert_eq!(subs[&m2].len(), 1);

        RUNTIME.lock().unwrap().stop();
    }

    // XXX This function should mock the subscribe and unsubscribe functions. However as
    // we can't yet return `impl Trait` from a Trait, we can't create a mock impl; it
    // would require that every function returned a trait object instead.
    // https://github.com/rust-lang/rfcs/pull/2515
    #[test]
    fn test_on_provide() {
        RUNTIME.lock().unwrap().start();

        let s1 = Mutex::new(HashMap::new());
        let (tx, _) = mpsc::unbounded_channel();
        let h1 = PublisherHandle {
            consumer: tx,
            subscribers: s1.clone(),
        };

        let s2 = Mutex::new(HashMap::new());
        let (tx, _) = mpsc::unbounded_channel();
        let h2 = PublisherHandle {
            consumer: tx,
            subscribers: s2.clone(),
        };

        // All this extra fluff around future::lazy() is necessary to ensure that there
        // is an active executor when the fn calls Mutex::with().
        RUNTIME.lock().unwrap().block_on(future::lazy(move || {
            h1.on_provide("/a".into(), h2).map_err(|e| {
                dbg!(e);
            })
        }));

        let m1 = Message::Subscribe("/a".into());
        let subs = s1.try_lock().ok().unwrap();
        assert_eq!(subs[&m1].len(), 1);

        let m2 = Message::Revoke("/a".into());
        let subs = s2.try_lock().ok().unwrap();
        assert_eq!(subs[&m2].len(), 1);

        RUNTIME.lock().unwrap().stop();
    }

    // XXX This function should mock the subscribe and unsubscribe functions. However as
    // we can't yet return `impl Trait` from a Trait, we can't create a mock impl; it
    // would require that every function returned a trait object instead.
    // https://github.com/rust-lang/rfcs/pull/2515
    #[test]
    fn test_on_subscribe() {
        RUNTIME.lock().unwrap().start();

        let s1 = Mutex::new(HashMap::new());
        let (tx, _) = mpsc::unbounded_channel();
        let h1 = PublisherHandle {
            consumer: tx,
            subscribers: s1.clone(),
        };

        let s2 = Mutex::new(HashMap::new());
        let (tx, _) = mpsc::unbounded_channel();
        let h2 = PublisherHandle {
            consumer: tx,
            subscribers: s2.clone(),
        };

        // All this extra fluff around future::lazy() is necessary to ensure that there
        // is an active executor when the fn calls Mutex::with().
        RUNTIME.lock().unwrap().block_on(future::lazy(move || {
            h1.on_subscribe("/a".into(), h2).map_err(|e| {
                dbg!(e);
            })
        }));

        let m2 = Message::Unsubscribe("/a".into());
        let subs = s2.try_lock().ok().unwrap();
        assert_eq!(subs[&m2].len(), 1);

        let m1 = Message::Event("/a".into(), String::new());
        let subs = s1.try_lock().ok().unwrap();
        assert_eq!(subs[&m1].len(), 1);

        RUNTIME.lock().unwrap().stop();
    }

    #[test]
    fn test_subscribe() {
        RUNTIME.lock().unwrap().start();

        let (tx, _) = mpsc::unbounded_channel();
        let subscribers = Mutex::new(HashMap::new());
        let message = Message::Provide("/a".into());
        let message_c = message.clone();
        let handle = PublisherHandle {
            consumer: tx,
            subscribers: subscribers.clone(),
        };

        // All this extra fluff around future::lazy() is necessary to ensure that there
        // is an active executor when the fn calls Mutex::with().
        let (tx, mut rx) = oneshot::channel();
        RUNTIME.lock().unwrap().block_on(future::lazy(move || {
            let (id, fut) = handle.subscribe(
                message_c,
                Subscriber::Task(Box::new(|_| Box::new(future::ok(())))),
            );
            tx.send(id).unwrap();
            fut.map_err(|e| {
                dbg!(e);
            })
        }));

        let subs = subscribers.try_unwrap().ok().unwrap();
        assert!(subs[&message].contains_key(&rx.try_recv().unwrap()));

        RUNTIME.lock().unwrap().stop();
    }

    #[test]
    fn test_unsubscribe_single() {
        RUNTIME.lock().unwrap().start();

        let (tx, _) = mpsc::unbounded_channel();

        let mut subs = HashMap::new();

        let mut message_subs = HashMap::new();
        let uuid = add_message_sub(&mut message_subs);
        let message = add_message(Message::Provide("/a".into()), message_subs, &mut subs);
        let message_c = message.clone();

        let subscribers = Mutex::new(subs);

        let handle = PublisherHandle {
            consumer: tx,
            subscribers: subscribers.clone(),
        };

        // All this extra fluff around future::lazy() is necessary to ensure that there
        // is an active executor when the fn calls Mutex::with().
        RUNTIME.lock().unwrap().block_on(future::lazy(move || {
            handle.unsubscribe(message, uuid).map_err(|e| {
                dbg!(e);
            })
        }));

        let subs = subscribers.try_unwrap().ok().unwrap();
        assert!(!subs.contains_key(&message_c));

        RUNTIME.lock().unwrap().stop();
    }

    #[test]
    fn test_unsubscribe_multiple() {
        RUNTIME.lock().unwrap().start();

        let (tx, _) = mpsc::unbounded_channel();

        let mut subs = HashMap::new();

        let mut message_subs = HashMap::new();
        let uuid1 = add_message_sub(&mut message_subs);
        let uuid2 = add_message_sub(&mut message_subs);
        let message = add_message(Message::Provide("/a".into()), message_subs, &mut subs);
        let message_c = message.clone();

        let subscribers = Mutex::new(subs);

        let handle = PublisherHandle {
            consumer: tx,
            subscribers: subscribers.clone(),
        };

        // All this extra fluff around future::lazy() is necessary to ensure that there
        // is an active executor when the fn calls Mutex::with().
        RUNTIME.lock().unwrap().block_on(future::lazy(move || {
            handle.unsubscribe(message, uuid1).map_err(|e| {
                dbg!(e);
            })
        }));

        let subs = subscribers.try_unwrap().ok().unwrap();
        assert!(subs.contains_key(&message_c));
        assert!(!subs[&message_c].contains_key(&uuid1));
        assert!(subs[&message_c].contains_key(&uuid2));

        RUNTIME.lock().unwrap().stop();
    }

    #[test]
    fn test_unsubscribe_children() {
        RUNTIME.lock().unwrap().start();

        let (tx, _) = mpsc::unbounded_channel();

        let mut subs = HashMap::new();

        let mut message_subs = HashMap::new();
        add_message_sub(&mut message_subs);
        let provide_a = add_message(Message::Provide("/a".into()), message_subs, &mut subs);

        let mut message_subs = HashMap::new();
        add_message_sub(&mut message_subs);
        let provide_ab = add_message(Message::Provide("/a/b".into()), message_subs, &mut subs);

        let mut message_subs = HashMap::new();
        add_message_sub(&mut message_subs);
        let provide_c = add_message(Message::Provide("/c".into()), message_subs, &mut subs);

        let mut message_subs = HashMap::new();
        add_message_sub(&mut message_subs);
        let revoke_a = add_message(Message::Revoke("/a".into()), message_subs, &mut subs);

        let subscribers = Mutex::new(subs);

        let handle = PublisherHandle {
            consumer: tx,
            subscribers: subscribers.clone(),
        };

        // All this extra fluff around future::lazy() is necessary to ensure that there
        // is an active executor when the fn calls Mutex::with().
        RUNTIME.lock().unwrap().block_on(future::lazy(move || {
            handle
                .unsubscribe_children(Message::Provide("/a".into()))
                .map_err(|e| {
                    dbg!(e);
                })
        }));

        let subs = subscribers.try_unwrap().ok().unwrap();
        assert!(!subs.contains_key(&provide_a));
        assert!(!subs.contains_key(&provide_ab));
        assert!(subs.contains_key(&provide_c));
        assert!(subs.contains_key(&revoke_a));

        RUNTIME.lock().unwrap().stop();
    }

    #[test]
    fn test_route_none() {
        RUNTIME.lock().unwrap().start();

        let (tx, _) = mpsc::unbounded_channel();

        let mut subs = HashMap::new();

        let mut retain_subs = HashMap::new();
        let (uuid, trigger) = add_message_sub_once_checked(&mut retain_subs);
        let message = add_message(Message::Subscribe("/a".into()), retain_subs, &mut subs);

        let subscribers = Mutex::new(subs);

        let handle = PublisherHandle {
            consumer: tx,
            subscribers: subscribers.clone(),
        };

        // All this extra fluff around future::lazy() is necessary to ensure that there
        // is an active executor when the fn calls Mutex::with().
        RUNTIME.lock().unwrap().block_on(future::lazy(move || {
            handle.route(Message::Provide("/b".into())).map_err(|_| ())
        }));

        // Check that the subscriber was not triggered
        assert!(!*trigger.try_lock().ok().unwrap());

        // Check that subscribers contains the message we expect to have retained
        let subs = subscribers.try_lock().ok().unwrap();
        assert_eq!(subs[&message].len(), 1);
        assert!(subs[&message].contains_key(&uuid));

        RUNTIME.lock().unwrap().stop();
    }

    #[test]
    fn test_route_retain() {
        RUNTIME.lock().unwrap().start();

        let (tx, _) = mpsc::unbounded_channel();

        let mut subs = HashMap::new();

        // Susbcription to test retention for non-empty maps
        let mut retain_subs = HashMap::new();
        // This message should remain post `route()`
        let (uuid_a, trigger_a) = add_message_sub_checked(&mut retain_subs);
        // This message should be deleted during `route()`
        let (_, trigger_b) = add_message_sub_once_checked(&mut retain_subs);
        let message = add_message(Message::Subscribe("/a".into()), retain_subs, &mut subs);
        let message_c = message.clone();

        let subscribers = Mutex::new(subs);

        let handle = PublisherHandle {
            consumer: tx,
            subscribers: subscribers.clone(),
        };

        // All this extra fluff around future::lazy() is necessary to ensure that there
        // is an active executor when the fn calls Mutex::with().
        RUNTIME
            .lock()
            .unwrap()
            .block_on(future::lazy(move || handle.route(message).map_err(|_| ())));

        // Check that the subscribers were triggered
        assert!(*trigger_a.try_lock().ok().unwrap());
        assert!(*trigger_b.try_lock().ok().unwrap());

        // Check that subscribers contains the one message we expect to have retained
        let subs = subscribers.try_lock().ok().unwrap();
        assert_eq!(subs[&message_c].len(), 1);
        assert!(subs[&message_c].contains_key(&uuid_a));

        RUNTIME.lock().unwrap().stop();
    }

    #[test]
    fn test_route_delete() {
        RUNTIME.lock().unwrap().start();

        let (tx, _) = mpsc::unbounded_channel();

        let mut subs = HashMap::new();

        // Subscription to test deletion of empty maps
        let mut delete_subs = HashMap::new();
        let (_, trigger) = add_message_sub_once_checked(&mut delete_subs);
        let message = add_message(Message::Unsubscribe("/b".into()), delete_subs, &mut subs);

        let subscribers = Mutex::new(subs);

        let handle = PublisherHandle {
            consumer: tx,
            subscribers: subscribers.clone(),
        };

        // All this extra fluff around future::lazy() is necessary to ensure that there
        // is an active executor when the fn calls Mutex::with().
        RUNTIME
            .lock()
            .unwrap()
            .block_on(future::lazy(move || handle.route(message).map_err(|_| ())));

        // Check that the subscriber was triggered
        assert!(*trigger.try_lock().ok().unwrap());

        // Check that subscribers is empty
        assert!(subscribers.try_lock().ok().unwrap().is_empty());

        RUNTIME.lock().unwrap().stop();
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
