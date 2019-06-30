#[allow(deprecated)]
mod errors;
mod message;
mod pubsub;

use clap::{App, Arg};
use errors::*;
use pubsub::PublisherHandle;
use tokio::net::TcpListener;
use tokio::prelude::*;

fn main() -> Result<()> {
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
    let addr = format!(
        "{}:{}",
        matches.value_of("bind").unwrap(),
        matches.value_of("port").unwrap()
    )
    .parse()
    .chain_err(|| "Invalid bind address")?;
    let listener = TcpListener::bind(&addr).expect("unable to bind TCP listener");

    // Create a new vector to store handles for all of the existing publishers
    let mut publishers: Vec<PublisherHandle> = vec![];

    // Pull out a stream of sockets for incoming connections
    tokio::run(
        listener
            .incoming()
            .map_err(|e| eprintln!("accept failed = {:?}", e))
            .for_each(move |sock| {
                // Create a new publisher/subscriber, and kick off their async tasks
                let newbie = pubsub::new(sock);

                // Introduce the newbie to all the other publishers. This allows each
                // publisher to subscribe to the other publishers' SUBSCRIBE events. This
                // is important for facilitating subscriptions between consumers of
                // different publishers.
                for publisher in publishers.iter() {
                    tokio::spawn(publisher.link(&newbie));
                }

                // Finally, add the newbie to the list of existing publishers
                publishers.push(newbie);

                Ok(())
            }),
    );

    Ok(())
}
