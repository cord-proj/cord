#[allow(deprecated)]
mod errors;

use clap::{App, Arg};
use env_logger;
use errors::*;
use futures::{future, future::Future, stream::Stream};
use log::error;
use message::{Codec, Message};
use pubsub::{self, Publisher};
use tokio::{
    codec::Framed,
    net::{TcpListener, TcpStream},
    prelude::stream::SplitSink,
    sync::mpsc::{self, UnboundedSender},
};

use std::net::SocketAddr;

fn main() -> Result<()> {
    env_logger::init();

    let matches = App::new("Server")
        .version("1.0")
        .author("Pete Hayes <pete@hayes.id.au>")
        .about("Data server node")
        .arg(
            Arg::with_name("bind")
                .short("a")
                .long("bind-address")
                .value_name("ADDRESS")
                .help("The IP address to bind this service to (e.g. 0.0.0.0 for all addresses) - defaults to 127.0.0.1")
                .takes_value(true)
                .default_value("127.0.0.1")
        )
        .arg(
            Arg::with_name("port")
                .short("p")
                .long("port")
                .value_name("PORT")
                .help("The port number to bind this service to - defaults to 7101")
                .takes_value(true)
                .default_value("7101")
        ).get_matches();

    // Bind the server's socket
    // `value_of().unwrap()` is safe as a default value will always be available
    let port = matches.value_of("port").unwrap().trim();
    // `value_of().unwrap()` is safe as a default value will always be available
    let addr = format!("{}:{}", matches.value_of("bind").unwrap().trim(), port)
        .parse()
        .chain_err(|| "Invalid bind address")?;
    let listener = TcpListener::bind(&addr).expect("unable to bind TCP listener");

    // If port is set to 0, the user wants us to bind to a random port. It would be
    // neighbourly to tell them what we've bound to!
    if port == "0" {
        if let Ok(SocketAddr::V4(s)) = listener.local_addr() {
            println!("{}", s.port());
        }
    }

    // Create a new vector to store handles for all of the existing publishers
    let mut publishers: Vec<Publisher> = vec![];

    // Poll the socket for incoming connections
    tokio::run(
        listener
            .incoming()
            .map_err(|e| eprintln!("accept failed = {:?}", e))
            .for_each(move |sock| {
                match sock.peer_addr() {
                    Ok(name) => {
                        // Wrap socket in message codec
                        let framed = Framed::new(sock, Codec::default());
                        let (sink, stream) = framed.split();

                        // Convert sink to channel so that we can clone it and distribute to
                        // multiple publishers
                        let subscriber = sink_to_channel(sink);

                        // Create a clonable publisher handle so we can control the publisher from afar
                        let handle = Publisher::new(name, subscriber);
                        let newbie = handle.clone();

                        // Introduce the newbie to all the other publishers. This allows each
                        // publisher to subscribe to the other publishers' SUBSCRIBE events. This
                        // is important for facilitating subscriptions between consumers of
                        // different publishers.
                        let mut futs = Vec::new();
                        for publisher in publishers.iter() {
                            futs.push(publisher.link(&newbie).map_err(|e| {
                                error!("{}", e);
                            }));
                        }

                        // This is the main routing task. For each message we receive, find all the
                        // subscribers that match it, then pass each a copy via `recv()`.
                        tokio::spawn(future::join_all(futs).and_then(|_| {
                            stream
                                .map_err(|e| pubsub::errors::ErrorKind::Message(e).into())
                                .for_each(move |message| handle.route(message))
                                .map_err(|_| ())
                        }));

                        // Finally, add the newbie to the list of existing publishers
                        publishers.push(newbie);
                    }
                    Err(e) => error!("{}", e),
                }

                Ok(())
            }),
    );

    Ok(())
}

// Create a channel for the consumer (sink) half of the socket. This allows us to
// pass clones of the channel to multiple producers to facilitate a consumer's
// subscriptions.
fn sink_to_channel(sink: SplitSink<Framed<TcpStream, Codec>>) -> UnboundedSender<Message> {
    let (tx, rx) = mpsc::unbounded_channel();

    // Spawn task to drain channel receiver into socket sink
    tokio::spawn(
        rx.map_err(|e| Error::from_kind(ErrorKind::ChanRecv(e)))
            .forward(sink)
            .map(|_| ())
            .map_err(|_| ()),
    );

    tx
}
