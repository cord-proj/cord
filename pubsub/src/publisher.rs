use crate::{errors::*, subscriber::Subscriber};
use futures::{self, future, Future};
use futures_locks::Mutex;
use log::debug;
use message::Message;
use pattern_matcher::Pattern;
use tokio::sync::mpsc::UnboundedSender;
use uuid::Uuid;

use std::{
    collections::{hash_map::Entry, HashMap},
    fmt,
    net::SocketAddr,
};

#[derive(Clone)]
pub struct Publisher {
    name: SocketAddr,
    // This is a channel to the write half of our TcpStream
    consumer: UnboundedSender<Message>,
    inner: Mutex<PublishInner>,
}

pub struct PublishInner {
    provides: Vec<Pattern>,
    subscribes: Vec<Pattern>,
    subscribers: HashMap<Message, HashMap<Uuid, Subscriber>>,
}

impl fmt::Display for Publisher {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Publisher<{}>", self.name)
    }
}

impl Publisher {
    pub fn new(name: SocketAddr, consumer: UnboundedSender<Message>) -> Publisher {
        Publisher {
            name,
            consumer,
            inner: Mutex::new(PublishInner {
                provides: Vec::new(),
                subscribes: Vec::new(),
                subscribers: HashMap::new(),
            }),
        }
    }

    // Link two publishers together so that they can subscribe to each other's streams
    pub fn link(&self, other: &Publisher) -> impl Future<Item = (), Error = Error> {
        debug!(target: "publisher", "Link {} with {}", self, other);

        let me = self.clone();
        let me_two = self.clone();
        let you = other.clone();
        let you_two = other.clone();

        // Link publishers
        let join = self.on_link(you).join(other.on_link(me)).map(|_| ());

        // Replay provide messages for new joiner
        join.and_then(move |_| {
            let me_three = me_two.clone();

            me_two
                .inner
                .with(move |guard| {
                    let mut futs = Vec::new();

                    (*guard).provides.iter().for_each(|p| {
                        debug!(target: "publisher", "Replay {:?} for new joiner", p);
                        futs.push(you_two.on_provide(p.clone(), me_three.clone()))
                    });

                    future::join_all(futs).map(|_| ())
                })
                .expect("The default executor has shut down")
        })
    }

    // Helper function for subscribing another publisher to our PROVIDE and REVOKE
    // messages
    fn on_link(&self, other: Publisher) -> impl Future<Item = (), Error = Error> {
        let me = self.clone();
        let me_two = self.clone();

        // Subscribe to PROVIDE messages
        let (_, futp) = self.subscribe(
            Message::Provide("/".into()),
            Subscriber::Task(Box::new(move |message| {
                Box::new(other.on_provide(message.unwrap_provide(), me.clone()))
            })),
        );

        // Subscribe to REVOKE messages
        // We use this task to bulk-remove all subscribers that have subscribed to the
        // revoked namespace.
        let (_, futr) =
            self.subscribe(
                Message::Revoke("/".into()),
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
        other: Publisher,
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
        other: Publisher,
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

        debug!(target: "publisher", "Subscribe {} to {:?} for {}", uuid, message, self);

        (
            uuid,
            self.inner
                .with(move |mut guard| {
                    (*guard)
                        .subscribers
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
        debug!(target: "publisher", "Unsubscribe {} from {:?} for {}", sub_id, message, self);

        self.inner
            .with(move |mut guard| {
                if let Entry::Occupied(mut o) = (*guard).subscribers.entry(message) {
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
        debug!(target: "publisher", "Unsubscribe everyone from {:?} for {}", message, self);

        self.inner
            .with(move |mut guard| {
                (*guard).subscribers.retain(|k, _| !message.contains(&k));
                future::ok(())
            })
            .expect("The default executor has shut down")
    }

    // Route a message to all interested subscribers, making sure we clean up any
    // subscribers that are only executed once (i.e. OnetimeTask).
    pub fn route(&mut self, message: Message) -> impl Future<Item = (), Error = Error> {
        let name = self.name;

        self.inner
            .with(move |mut guard| {
                // Cache important messages so we can replay them to new joiners
                match message {
                    Message::Provide(ref pattern) => (*guard).provides.push(pattern.clone()),
                    Message::Revoke(ref pattern) => {
                        (*guard).provides.retain(|provide| provide != pattern)
                    }
                    Message::Subscribe(ref pattern) => (*guard).subscribes.push(pattern.clone()),
                    Message::Unsubscribe(ref pattern) => {
                        (*guard).subscribes.retain(|subscribe| subscribe != pattern)
                    }
                    _ => (),
                }

                debug!("Received message {:?} from {}", message, name);

                let mut futs = vec![];

                // In the inner loop we may cleanup all of the subscribers in the
                // `subs` map. This leaves us with an empty map that should also
                // be tidied up, hence the use of retain() here.
                (*guard).subscribers.retain(|sub_msg, subs| {
                    if sub_msg.contains(&message) {
                        debug!("Subscriber {:?} matches {:?}", sub_msg, message);
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
                        debug!("Subscriber {:?} does not match {:?}", sub_msg, message);
                        true // Retain as we haven't modified the map
                    }
                });

                future::join_all(futs).map(|_| ())
            })
            .expect("The default executor has shut down")
    }
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

    use std::{
        net::{IpAddr, Ipv4Addr, SocketAddr},
        sync::Mutex as StdMutex,
    };

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
        subs: &mut PublishInner,
    ) -> Message {
        let message_c = message.clone();
        subs.subscribers.insert(message, message_subs);
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

        let s1 = Mutex::new(PublishInner {
            provides: Vec::new(),
            subscribes: Vec::new(),
            subscribers: HashMap::new(),
        });
        let (tx, _) = mpsc::unbounded_channel();
        let h1 = Publisher {
            name: SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 8080),
            consumer: tx,
            inner: s1.clone(),
        };

        let (tx, _) = mpsc::unbounded_channel();
        let h2 = Publisher {
            name: SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 8080),

            consumer: tx,
            inner: Mutex::new(PublishInner {
                provides: Vec::new(),
                subscribes: Vec::new(),
                subscribers: HashMap::new(),
            }),
        };

        // All this extra fluff around future::lazy() is necessary to ensure that there
        // is an active executor when the fn calls Mutex::with().
        RUNTIME.lock().unwrap().block_on(future::lazy(move || {
            h1.on_link(h2).map_err(|e| {
                dbg!(e);
            })
        }));

        let subs = s1.try_lock().ok().unwrap();

        let m1 = Message::Provide("/".into());
        assert_eq!(subs.subscribers[&m1].len(), 1);

        let m2 = Message::Revoke("/".into());
        assert_eq!(subs.subscribers[&m2].len(), 1);

        RUNTIME.lock().unwrap().stop();
    }

    // XXX This function should mock the subscribe and unsubscribe functions. However as
    // we can't yet return `impl Trait` from a Trait, we can't create a mock impl; it
    // would require that every function returned a trait object instead.
    // https://github.com/rust-lang/rfcs/pull/2515
    #[test]
    fn test_on_provide() {
        RUNTIME.lock().unwrap().start();

        let s1 = Mutex::new(PublishInner {
            provides: Vec::new(),
            subscribes: Vec::new(),
            subscribers: HashMap::new(),
        });
        let (tx, _) = mpsc::unbounded_channel();
        let h1 = Publisher {
            name: SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 8080),
            consumer: tx,
            inner: s1.clone(),
        };

        let s2 = Mutex::new(PublishInner {
            provides: Vec::new(),
            subscribes: Vec::new(),
            subscribers: HashMap::new(),
        });
        let (tx, _) = mpsc::unbounded_channel();
        let h2 = Publisher {
            name: SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 8080),
            consumer: tx,
            inner: s2.clone(),
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
        assert_eq!(subs.subscribers[&m1].len(), 1);

        let m2 = Message::Revoke("/a".into());
        let subs = s2.try_lock().ok().unwrap();
        assert_eq!(subs.subscribers[&m2].len(), 1);

        RUNTIME.lock().unwrap().stop();
    }

    // XXX This function should mock the subscribe and unsubscribe functions. However as
    // we can't yet return `impl Trait` from a Trait, we can't create a mock impl; it
    // would require that every function returned a trait object instead.
    // https://github.com/rust-lang/rfcs/pull/2515
    #[test]
    fn test_on_subscribe() {
        RUNTIME.lock().unwrap().start();

        let s1 = Mutex::new(PublishInner {
            provides: Vec::new(),
            subscribes: Vec::new(),
            subscribers: HashMap::new(),
        });
        let (tx, _) = mpsc::unbounded_channel();
        let h1 = Publisher {
            name: SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 8080),
            consumer: tx,
            inner: s1.clone(),
        };

        let s2 = Mutex::new(PublishInner {
            provides: Vec::new(),
            subscribes: Vec::new(),
            subscribers: HashMap::new(),
        });
        let (tx, _) = mpsc::unbounded_channel();
        let h2 = Publisher {
            name: SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 8080),
            consumer: tx,
            inner: s2.clone(),
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
        assert_eq!(subs.subscribers[&m2].len(), 1);

        let m1 = Message::Event("/a".into(), String::new());
        let subs = s1.try_lock().ok().unwrap();
        assert_eq!(subs.subscribers[&m1].len(), 1);

        RUNTIME.lock().unwrap().stop();
    }

    #[test]
    fn test_subscribe() {
        RUNTIME.lock().unwrap().start();

        let (tx, _) = mpsc::unbounded_channel();
        let inner = Mutex::new(PublishInner {
            provides: Vec::new(),
            subscribes: Vec::new(),
            subscribers: HashMap::new(),
        });
        let message = Message::Provide("/a".into());
        let message_c = message.clone();
        let handle = Publisher {
            name: SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 8080),
            consumer: tx,
            inner: inner.clone(),
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

        let inn = inner.try_unwrap().ok().unwrap();
        assert!(inn.subscribers[&message].contains_key(&rx.try_recv().unwrap()));

        RUNTIME.lock().unwrap().stop();
    }

    #[test]
    fn test_unsubscribe_single() {
        RUNTIME.lock().unwrap().start();

        let (tx, _) = mpsc::unbounded_channel();

        let mut pi = PublishInner {
            provides: Vec::new(),
            subscribes: Vec::new(),
            subscribers: HashMap::new(),
        };

        let mut message_subs = HashMap::new();
        let uuid = add_message_sub(&mut message_subs);
        let message = add_message(Message::Provide("/a".into()), message_subs, &mut pi);
        let message_c = message.clone();

        let inner = Mutex::new(pi);

        let handle = Publisher {
            name: SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 8080),
            consumer: tx,
            inner: inner.clone(),
        };

        // All this extra fluff around future::lazy() is necessary to ensure that there
        // is an active executor when the fn calls Mutex::with().
        RUNTIME.lock().unwrap().block_on(future::lazy(move || {
            handle.unsubscribe(message, uuid).map_err(|e| {
                dbg!(e);
            })
        }));

        let inn = inner.try_unwrap().ok().unwrap();
        assert!(!inn.subscribers.contains_key(&message_c));

        RUNTIME.lock().unwrap().stop();
    }

    #[test]
    fn test_unsubscribe_multiple() {
        RUNTIME.lock().unwrap().start();

        let (tx, _) = mpsc::unbounded_channel();

        let mut pi = PublishInner {
            provides: Vec::new(),
            subscribes: Vec::new(),
            subscribers: HashMap::new(),
        };

        let mut message_subs = HashMap::new();
        let uuid1 = add_message_sub(&mut message_subs);
        let uuid2 = add_message_sub(&mut message_subs);
        let message = add_message(Message::Provide("/a".into()), message_subs, &mut pi);
        let message_c = message.clone();

        let inner = Mutex::new(pi);

        let handle = Publisher {
            name: SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 8080),
            consumer: tx,
            inner: inner.clone(),
        };

        // All this extra fluff around future::lazy() is necessary to ensure that there
        // is an active executor when the fn calls Mutex::with().
        RUNTIME.lock().unwrap().block_on(future::lazy(move || {
            handle.unsubscribe(message, uuid1).map_err(|e| {
                dbg!(e);
            })
        }));

        let inn = inner.try_unwrap().ok().unwrap();
        assert!(inn.subscribers.contains_key(&message_c));
        assert!(!inn.subscribers[&message_c].contains_key(&uuid1));
        assert!(inn.subscribers[&message_c].contains_key(&uuid2));

        RUNTIME.lock().unwrap().stop();
    }

    #[test]
    fn test_unsubscribe_children() {
        RUNTIME.lock().unwrap().start();

        let (tx, _) = mpsc::unbounded_channel();

        let mut pi = PublishInner {
            provides: Vec::new(),
            subscribes: Vec::new(),
            subscribers: HashMap::new(),
        };

        let mut message_subs = HashMap::new();
        add_message_sub(&mut message_subs);
        let provide_a = add_message(Message::Provide("/a".into()), message_subs, &mut pi);

        let mut message_subs = HashMap::new();
        add_message_sub(&mut message_subs);
        let provide_ab = add_message(Message::Provide("/a/b".into()), message_subs, &mut pi);

        let mut message_subs = HashMap::new();
        add_message_sub(&mut message_subs);
        let provide_c = add_message(Message::Provide("/c".into()), message_subs, &mut pi);

        let mut message_subs = HashMap::new();
        add_message_sub(&mut message_subs);
        let revoke_a = add_message(Message::Revoke("/a".into()), message_subs, &mut pi);

        let inner = Mutex::new(pi);

        let handle = Publisher {
            name: SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 8080),
            consumer: tx,
            inner: inner.clone(),
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

        let inn = inner.try_unwrap().ok().unwrap();
        assert!(!inn.subscribers.contains_key(&provide_a));
        assert!(!inn.subscribers.contains_key(&provide_ab));
        assert!(inn.subscribers.contains_key(&provide_c));
        assert!(inn.subscribers.contains_key(&revoke_a));

        RUNTIME.lock().unwrap().stop();
    }

    #[test]
    fn test_route_none() {
        RUNTIME.lock().unwrap().start();

        let (tx, _) = mpsc::unbounded_channel();

        let mut pi = PublishInner {
            provides: Vec::new(),
            subscribes: Vec::new(),
            subscribers: HashMap::new(),
        };

        let mut retain_subs = HashMap::new();
        let (uuid, trigger) = add_message_sub_once_checked(&mut retain_subs);
        let message = add_message(Message::Subscribe("/a".into()), retain_subs, &mut pi);

        let inner = Mutex::new(pi);

        let mut handle = Publisher {
            name: SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 8080),
            consumer: tx,
            inner: inner.clone(),
        };

        // All this extra fluff around future::lazy() is necessary to ensure that there
        // is an active executor when the fn calls Mutex::with().
        RUNTIME.lock().unwrap().block_on(future::lazy(move || {
            handle.route(Message::Provide("/b".into())).map_err(|_| ())
        }));

        // Check that the subscriber was not triggered
        assert!(!*trigger.try_lock().ok().unwrap());

        // Check that subscribers contains the message we expect to have retained
        let inn = inner.try_lock().ok().unwrap();
        assert_eq!(inn.subscribers[&message].len(), 1);
        assert!(inn.subscribers[&message].contains_key(&uuid));

        RUNTIME.lock().unwrap().stop();
    }

    #[test]
    fn test_route_retain() {
        RUNTIME.lock().unwrap().start();

        let (tx, _) = mpsc::unbounded_channel();

        let mut pi = PublishInner {
            provides: Vec::new(),
            subscribes: Vec::new(),
            subscribers: HashMap::new(),
        };

        // Susbcription to test retention for non-empty maps
        let mut retain_subs = HashMap::new();
        // This message should remain post `route()`
        let (uuid_a, trigger_a) = add_message_sub_checked(&mut retain_subs);
        // This message should be deleted during `route()`
        let (_, trigger_b) = add_message_sub_once_checked(&mut retain_subs);
        let message = add_message(Message::Subscribe("/a".into()), retain_subs, &mut pi);
        let message_c = message.clone();

        let inner = Mutex::new(pi);

        let mut handle = Publisher {
            name: SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 8080),
            consumer: tx,
            inner: inner.clone(),
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
        let inn = inner.try_lock().ok().unwrap();
        assert_eq!(inn.subscribers[&message_c].len(), 1);
        assert!(inn.subscribers[&message_c].contains_key(&uuid_a));

        RUNTIME.lock().unwrap().stop();
    }

    #[test]
    fn test_route_delete() {
        RUNTIME.lock().unwrap().start();

        let (tx, _) = mpsc::unbounded_channel();

        let mut pi = PublishInner {
            provides: Vec::new(),
            subscribes: Vec::new(),
            subscribers: HashMap::new(),
        };

        // Subscription to test deletion of empty maps
        let mut delete_subs = HashMap::new();
        let (_, trigger) = add_message_sub_once_checked(&mut delete_subs);
        let message = add_message(Message::Unsubscribe("/b".into()), delete_subs, &mut pi);

        let inner = Mutex::new(pi);

        let mut handle = Publisher {
            name: SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 8080),
            consumer: tx,
            inner: inner.clone(),
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
        assert!(inner.try_lock().ok().unwrap().subscribers.is_empty());

        RUNTIME.lock().unwrap().stop();
    }
}
