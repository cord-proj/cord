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

use std::{
    collections::{hash_map::Entry, HashMap},
    mem,
};

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
    Task(Box<FnMut(Message) -> Box<dyn Future<Item = (), Error = ()> + Send> + Send>),
    // Task(Box<FnMut(Message, PublisherHandle) -> impl Future<Item=(), Error=()>>)
    OnetimeTask(Box<FnMut(Message) -> Box<dyn Future<Item = (), Error = ()> + Send> + Send>),
}

impl PublisherHandle {
    fn new(consumer: UnboundedSender<Message>) -> PublisherHandle {
        PublisherHandle {
            consumer,
            subscribers: Mutex::new(HashMap::new()),
        }
    }

    // Link two publishers together so that they can subscribe to each other's streams
    pub fn link(&self, other: &PublisherHandle) -> impl Future<Item = (), Error = ()> {
        let me = self.clone();
        let you = other.clone();

        self.on_link(you).join(other.on_link(me)).map(|_| ())
    }

    // Helper function for subscribing another publisher to our PROVIDE and REVOKE
    // messages
    fn on_link(&self, other: PublisherHandle) -> impl Future<Item = (), Error = ()> {
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
    ) -> impl Future<Item = (), Error = ()> {
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
            Subscriber::OnetimeTask(Box::new(move |message| {
                Box::new(me.unsubscribe(Message::Subscribe(message.unwrap_revoke()), uuid))
            })),
        );

        futs.join(futr).map(|_| ())
    }

    // Helper function for subscribing another publisher's consumer to our EVENT messages
    fn on_subscribe(
        &self,
        namespace: Pattern,
        other: PublisherHandle,
    ) -> impl Future<Item = (), Error = ()> {
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
            Subscriber::OnetimeTask(Box::new(move |message| {
                Box::new(me.unsubscribe(
                    Message::Event(message.unwrap_unsubscribe(), String::new()),
                    uuid,
                ))
            })),
        );

        fute.join(futu).map(|_| ())
    }

    // Add a Subscriber for a specific Message to our subscriber list
    fn subscribe(
        &self,
        message: Message,
        subscriber: Subscriber,
    ) -> (Uuid, impl Future<Item = (), Error = ()>) {
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
    fn unsubscribe(
        &self,
        message: Message,
        subscriber: Uuid,
    ) -> impl Future<Item = (), Error = ()> {
        self.subscribers
            .with(move |mut guard| {
                if let Entry::Occupied(mut o) = (*guard).entry(message) {
                    let e = o.get_mut();
                    e.remove(&subscriber);

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
    fn unsubscribe_children(&self, message: Message) -> impl Future<Item = (), Error = ()> {
        self.subscribers
            .with(move |mut guard| {
                (*guard).retain(|k, _| {
                    mem::discriminant(k) != mem::discriminant(&message)
                        || k.namespace() > message.namespace()
                });
                future::ok(())
            })
            .expect("The default executor has shut down")
    }
}

impl Subscriber {
    fn recv(&mut self, message: Message) -> bool {
        match self {
            // Retain subscriber in map if the channel is ok
            Subscriber::Consumer(ref mut chan) => chan.try_send(message).is_ok(),
            Subscriber::Task(f) => {
                tokio::spawn(f(message));
                true // Retain subscriber in map
            }
            Subscriber::OnetimeTask(f) => {
                tokio::spawn(f(message));
                false // Don't retain subscriber in map
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
            .for_each(move |message| {
                handle
                    .subscribers
                    .with(move |mut guard| {
                        // In the inner loop we may cleanup all of the subscribers in the
                        // `subs` map. This leaves us with an empty map that should also
                        // be tidied up, hence the use of retain() here.
                        (*guard).retain(|sub_msg, subs| {
                            if sub_msg.namespace() >= message.namespace() {
                                // We use retain() here, which allows us to cleanup any
                                // subscribers that have OnetimeTask's or whose channels
                                // have been closed.
                                subs.retain(|_, sub| sub.recv(message.clone()));
                                !subs.is_empty() // Retain if map contains subscribers
                            } else {
                                true // Retain as we haven't modified the map
                            }
                        });
                        future::ok(())
                    })
                    .expect("The default executor has shut down")
            })
            .map_err(|_| ()),
    );

    handle_c
}
