mod utils;

use client::Conn;
use futures::{Future, Stream};
use message::Message;
use pattern_matcher::Pattern;
use tokio::sync::{mpsc, oneshot};

use std::panic;

#[test]
fn test_reciprocal() {
    let (mut child, port) = utils::start_server();

    let result = panic::catch_unwind(|| {
        let (tx, mut rx1) = oneshot::channel();
        let client1 = Conn::new(format!("127.0.0.1:{}", port).parse().unwrap())
            .map_err(|e| panic!("{}", e))
            .and_then(move |mut conn| {
                conn.provide("/users".into()).unwrap();
                let group_rx = conn.subscribe("/groups".into()).unwrap();

                conn.event("/users/add".into(), "Mark has joined").unwrap();

                send_event(group_rx, tx, conn)
            });

        let (tx, mut rx2) = oneshot::channel();
        let client2 = Conn::new(format!("127.0.0.1:{}", port).parse().unwrap())
            .map_err(|e| panic!("{}", e))
            .and_then(move |mut conn| {
                conn.provide("/groups".into()).unwrap();
                let group_rx = conn.subscribe("/users".into()).unwrap();

                conn.event("/groups/add".into(), "Admin group created")
                    .unwrap();

                send_event(group_rx, tx, conn)
            });

        tokio::run(client1.join(client2).map(|_| ()));

        assert_eq!(
            rx1.try_recv().unwrap(),
            ("/groups/add".into(), "Admin group created".into())
        );

        assert_eq!(
            rx2.try_recv().unwrap(),
            ("/users/add".into(), "Mark has joined".into())
        );
    });

    child.kill().expect("Server was not running");
    result.unwrap();
}

fn send_event(
    group_rx: mpsc::Receiver<Message>,
    tx: oneshot::Sender<(Pattern, String)>,
    conn: Conn,
) -> impl Future<Item = (), Error = ()> {
    group_rx
        .into_future()
        .and_then(move |(msg, _)| {
            let _c = conn;
            match msg.unwrap() {
                Message::Event(n, d) => tx.send((n, d)).unwrap(),
                _ => unreachable!(),
            }
            Ok(())
        })
        .map_err(|_| ())
}
