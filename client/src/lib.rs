pub mod errors;

use errors::{Error, ErrorKind, Result};
use futures::{future, Future, Stream};
use futures_locks::Mutex;
use message::{Codec, Message};
use pattern_matcher::Pattern;
use retain_mut::RetainMut;
use tokio::{codec::Framed, net::TcpStream, sync::mpsc};

use std::{collections::HashMap, net::SocketAddr};

/// A `Conn` is used to connect to and communicate with a server.
///
/// # Examples
///
/// ```
/// use client::Conn;
/// use futures::{future, Future};
/// use tokio;
///
/// let fut = Conn::new("127.0.0.1:7101".parse().unwrap()).and_then(|mut conn| {
///     // Tell the server we're going to provide the namespace /users
///     conn.provide("/users".into()).unwrap();
///
///     // Start publishing events...
///     conn.event("/users/mark".into(), "Mark has joined").unwrap();
///
///     future::ok(())
/// }).map_err(|_| ());
///
/// tokio::run(fut);
/// ```
pub struct Conn {
    sender: mpsc::UnboundedSender<Message>,
    receivers: Mutex<HashMap<Pattern, Vec<mpsc::Sender<Message>>>>,
}

impl Conn {
    /// Connect to a server
    pub fn new(addr: SocketAddr) -> impl Future<Item = Conn, Error = Error> {
        // Because Sink::send takes ownership of the sink and returns a future, we need a
        // different type to send data that doesn't require ownership. Channels are great
        // for this.
        let (tx, rx) = mpsc::unbounded_channel();

        TcpStream::connect(&addr)
            .map(|sock| {
                // Wrap socket in message codec
                let framed = Framed::new(sock, Codec::default());
                let (sink, stream) = framed.split();

                // Drain the channel's receiver into the codec's sink
                tokio::spawn(
                    rx.map_err(|e| Error::from_kind(ErrorKind::ConnRecv(e)))
                        .forward(sink)
                        .map(|_| ())
                        .map_err(|_| ()),
                );

                // Setup the receivers map
                let receivers = Mutex::new(HashMap::new());
                let receivers_c = receivers.clone();

                // Route the codec's stream to receivers
                tokio::spawn(
                    stream
                        .map_err(|e| Error::from_kind(ErrorKind::Message(e)))
                        .for_each(move |message| route(&receivers_c, message))
                        .map(|_| ())
                        .map_err(|_| ()),
                );

                Conn {
                    sender: tx,
                    receivers,
                }
            })
            .map_err(|e| ErrorKind::Io(e).into())
    }

    /// Inform the server that you will be providing a new namespace
    pub fn provide(&mut self, namespace: Pattern) -> Result<()> {
        Ok(self.sender.try_send(Message::Provide(namespace))?)
    }

    /// Inform the server that you will no longer be providing a namespace
    pub fn revoke(&mut self, namespace: Pattern) -> Result<()> {
        Ok(self.sender.try_send(Message::Revoke(namespace))?)
    }

    /// Subscribe to another provider's namespace
    ///
    /// # Examples
    ///
    /// ```
    ///# use client::Conn;
    /// use client::errors::ErrorKind;
    ///# use futures::{future, Future, Stream};
    ///# use tokio;
    ///
    ///# let fut = Conn::new("127.0.0.1:7101".parse().unwrap()).and_then(|mut conn| {
    /// conn.subscribe("/users/".into()).unwrap().for_each(|msg| {
    ///     // Handle the message...
    ///     dbg!("The following user just joined: {}", msg);
    ///
    ///     future::ok(())
    /// }).map_err(|e| ErrorKind::SubscriberError(e).into())
    ///# });
    /// ```
    pub fn subscribe(&mut self, namespace: Pattern) -> Result<mpsc::Receiver<Message>> {
        let (tx, rx) = mpsc::channel(10);
        let namespace_c = namespace.clone();
        tokio::spawn(
            self.receivers
                .with(move |mut guard| {
                    (*guard)
                        .entry(namespace_c)
                        .or_insert_with(Vec::new)
                        .push(tx);
                    future::ok(())
                })
                .expect("The default executor has shut down")
                .map(|_| ()),
        );
        self.sender.try_send(Message::Subscribe(namespace))?;
        Ok(rx)
    }

    /// Unsubscribe from another provider's namespace
    pub fn unsubscribe(&mut self, namespace: Pattern) -> Result<()> {
        let namespace_c = namespace.clone();
        self.sender.try_send(Message::Unsubscribe(namespace))?;
        tokio::spawn(
            self.receivers
                .with(move |mut guard| {
                    (*guard).remove(&namespace_c);
                    future::ok(())
                })
                .expect("The default executor has shut down")
                .map(|_| ()),
        );
        Ok(())
    }

    /// Publish an event to your subscribers
    pub fn event<S: Into<String>>(&mut self, namespace: Pattern, data: S) -> Result<()> {
        Ok(self
            .sender
            .try_send(Message::Event(namespace, data.into()))?)
    }
}

fn route(
    receivers: &Mutex<HashMap<Pattern, Vec<mpsc::Sender<Message>>>>,
    message: Message,
) -> impl Future<Item = (), Error = Error> {
    receivers
        .with(move |mut guard| {
            // Remove any subscribers that have no senders left
            (*guard).retain(|namespace, senders| {
                // We assume that all messages will be Events. If this changes, we will
                // need to store a Message, not a pattern.
                if namespace.contains(message.namespace()) {
                    // Remove any senders that give errors when attempting to send
                    senders.retain_mut(|tx| tx.try_send(message.clone()).is_ok());

                    // XXX Awaiting stabilisation
                    // https://github.com/rust-lang/rust/issues/43244
                    // senders.drain_filter(|tx| {
                    //     tx.try_send(message.clone()).is_ok()
                    // });
                }

                // So long as we have senders, keep the subscriber
                !senders.is_empty()
            });

            future::ok(())
        })
        .expect("The default executor has shut down")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_provide() {
        let (tx, rx) = mpsc::unbounded_channel();

        let mut conn = Conn {
            sender: tx,
            receivers: Mutex::new(HashMap::new()),
        };

        conn.provide("/a/b".into()).unwrap();
        assert_eq!(
            rx.into_future().wait().unwrap().0.unwrap(),
            Message::Provide("/a/b".into())
        );
    }

    #[test]
    fn test_revoke() {
        let (tx, rx) = mpsc::unbounded_channel();

        let mut conn = Conn {
            sender: tx,
            receivers: Mutex::new(HashMap::new()),
        };

        conn.revoke("/a/b".into()).unwrap();
        assert_eq!(
            rx.into_future().wait().unwrap().0.unwrap(),
            Message::Revoke("/a/b".into())
        );
    }

    #[test]
    fn test_subscribe() {
        let (tx, rx) = mpsc::unbounded_channel();

        let receivers = Mutex::new(HashMap::new());

        let mut conn = Conn {
            sender: tx,
            receivers: receivers.clone(),
        };

        // All this extra fluff around future::lazy() is necessary to ensure that there
        // is an active executor when the fn calls Mutex::with().
        tokio::run(future::lazy(move || {
            conn.subscribe("/a/b".into()).unwrap();
            future::ok(())
        }));
        assert_eq!(
            rx.into_future().wait().unwrap().0.unwrap(),
            Message::Subscribe("/a/b".into())
        );
        assert!(receivers.try_unwrap().unwrap().contains_key(&"/a/b".into()));
    }

    #[test]
    fn test_unsubscribe() {
        let (tx, rx) = mpsc::unbounded_channel();

        let mut receivers = HashMap::new();
        receivers.insert("/a/b".into(), Vec::new());
        let receivers = Mutex::new(receivers);
        let receivers_c = receivers.clone();

        let mut conn = Conn {
            sender: tx,
            receivers,
        };

        // All this extra fluff around future::lazy() is necessary to ensure that there
        // is an active executor when the fn calls Mutex::with().
        tokio::run(future::lazy(move || {
            conn.unsubscribe("/a/b".into()).unwrap();
            future::ok(())
        }));
        assert_eq!(
            rx.into_future().wait().unwrap().0.unwrap(),
            Message::Unsubscribe("/a/b".into())
        );
        assert!(receivers_c.try_unwrap().unwrap().is_empty());
    }

    #[test]
    fn test_event() {
        let (tx, rx) = mpsc::unbounded_channel();

        let mut conn = Conn {
            sender: tx,
            receivers: Mutex::new(HashMap::new()),
        };

        conn.event("/a/b".into(), "moo").unwrap();
        assert_eq!(
            rx.into_future().wait().unwrap().0.unwrap(),
            Message::Event("/a/b".into(), "moo".into())
        );
    }

    #[test]
    fn test_route() {
        let (tx, rx) = mpsc::channel(10);

        let mut receivers = HashMap::new();
        receivers.insert("/a/b".into(), vec![tx]);
        let receivers = Mutex::new(receivers);
        let receivers_c = receivers.clone();

        let event_msg = Message::Event("/a/b".into(), "Moo!".into());
        let event_msg_c = event_msg.clone();

        // All this extra fluff around future::lazy() is necessary to ensure that there
        // is an active executor when the fn calls Mutex::with().
        tokio::run(future::lazy(move || {
            route(&receivers, event_msg).map_err(|_| ())
        }));

        assert_eq!(rx.into_future().wait().unwrap().0.unwrap(), event_msg_c);
        assert!(receivers_c
            .try_unwrap()
            .unwrap()
            .contains_key(&"/a/b".into()));
    }

    #[test]
    fn test_route_norecv() {
        let (tx, _) = mpsc::channel(10);

        let mut receivers = HashMap::new();
        receivers.insert("/a/b".into(), vec![tx]);
        let receivers = Mutex::new(receivers);
        let receivers_c = receivers.clone();

        // All this extra fluff around future::lazy() is necessary to ensure that there
        // is an active executor when the fn calls Mutex::with().
        tokio::run(future::lazy(move || {
            route(&receivers, Message::Event("/a/b".into(), "Moo!".into())).map_err(|_| ())
        }));

        assert!(receivers_c.try_unwrap().unwrap().is_empty());
    }
}
