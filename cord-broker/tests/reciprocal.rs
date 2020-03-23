mod utils;

use cord_client::{Conn, Subscriber};
use cord_pattern::Pattern;
use futures::{Future, Stream};
use tokio::{sync::oneshot, timer::Delay};

use std::{
    panic,
    time::{Duration, Instant},
};

#[test]
fn test_reciprocal() {
    let f = |port| {
        let (tx, mut rx1) = oneshot::channel();
        let client1 = Conn::new(format!("127.0.0.1:{}", port).parse().unwrap())
            .map_err(|e| panic!("{}", e))
            .and_then(|mut conn| {
                conn.provide("/users".into()).unwrap();

                Delay::new(Instant::now() + Duration::from_millis(100))
                    .map_err(|e| panic!(e))
                    .and_then(|_| {
                        let group_rx = conn.subscribe("/groups".into()).unwrap();

                        Delay::new(Instant::now() + Duration::from_millis(100))
                            .map_err(|e| panic!(e))
                            .and_then(move |_| {
                                conn.event("/users/add".into(), "Mark has joined").unwrap();
                                Ok(())
                            })
                            .and_then(|_| send_event(group_rx, tx))
                    })
            });

        let (tx, mut rx2) = oneshot::channel();
        let client2 = Conn::new(format!("127.0.0.1:{}", port).parse().unwrap())
            .map_err(|e| panic!("{}", e))
            .and_then(|mut conn| {
                conn.provide("/groups".into()).unwrap();

                Delay::new(Instant::now() + Duration::from_millis(100))
                    .map_err(|e| panic!(e))
                    .and_then(|_| {
                        let user_rx = conn.subscribe("/users".into()).unwrap();

                        Delay::new(Instant::now() + Duration::from_millis(100))
                            .map_err(|e| panic!(e))
                            .and_then(move |_| {
                                conn.event("/groups/add".into(), "Admin group created")
                                    .unwrap();
                                Ok(())
                            })
                            .and_then(|_| send_event(user_rx, tx))
                    })
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
    };

    utils::run_client(f);
}

fn send_event(
    rx: Subscriber,
    tx: oneshot::Sender<(Pattern, String)>,
) -> impl Future<Item = (), Error = ()> {
    rx.into_future()
        .and_then(move |(opt, _)| {
            tx.send(opt.unwrap()).unwrap();
            Ok(())
        })
        .map_err(|_| ())
}
