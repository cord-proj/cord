use client::{Conn, Subscriber};
use futures::{
    compat::Future01CompatExt, compat::Stream01CompatExt, join, Future, FutureExt, StreamExt,
    TryFutureExt, TryStreamExt,
};
use pattern_matcher::Pattern;
use std::{panic, time::Duration};
use tokio::{sync::oneshot, time};

mod utils;

#[tokio::test]
async fn test_reciprocal() {
    // Start logger
    utils::init_log();

    // Start a new server process
    let (mut server, socket_addr) = utils::start_server();

    let result = panic::catch_unwind(|| {
        async {
            let (tx, mut rx1) = oneshot::channel();
            let client1 = Conn::new(socket_addr)
                .compat()
                .map_err(|e| panic!("{}", e))
                .and_then(|mut conn| {
                    conn.provide("/users".into()).unwrap();

                    time::delay_for(Duration::from_millis(100)).then(|_| {
                        let group_rx = conn.subscribe("/groups".into()).unwrap();

                        time::delay_for(Duration::from_millis(100))
                            .then(move |_| {
                                async {
                                    conn.event("/users/add".into(), "Mark has joined").unwrap();
                                }
                            })
                            .then(|_| {
                                send_event(group_rx, tx);
                                Ok(())
                            })
                    })
                });

            let (tx, mut rx2) = oneshot::channel();
            let client2 = Conn::new(socket_addr)
                .compat()
                .map_err(|e| panic!("{}", e))
                .and_then(|mut conn| {
                    conn.provide("/groups".into()).unwrap();

                    time::delay_for(Duration::from_millis(100)).then(|_| {
                        let user_rx = conn.subscribe("/users".into()).unwrap();

                        time::delay_for(Duration::from_millis(100))
                            .then(move |_| {
                                async {
                                    conn.event("/groups/add".into(), "Admin group created")
                                        .unwrap();
                                }
                            })
                            .then(|_| send_event(user_rx, tx))
                    })
                });

            join!(client1, client2);

            assert_eq!(
                rx1.try_recv().unwrap(),
                ("/groups/add".into(), "Admin group created".into())
            );

            assert_eq!(
                rx2.try_recv().unwrap(),
                ("/users/add".into(), "Mark has joined".into())
            );
        }
    });

    // Terminate the server
    server.kill().expect("Server was not running");

    // Now we can resume panicking if needed
    if let Err(e) = result {
        panic::resume_unwind(e);
    }
}

fn send_event(rx: Subscriber, tx: oneshot::Sender<(Pattern, String)>) -> impl Future<Output = ()> {
    rx.compat()
        .into_future()
        .and_then(move |(opt, _)| {
            tx.send(opt.unwrap()).unwrap();
            Ok(())
        })
        .map_err(|_| ())
}
