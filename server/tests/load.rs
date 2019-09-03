mod executors;
// mod utils;
//
// use atomic_counter::{AtomicCounter, RelaxedCounter};
// use client::{errors::Error, Conn};
// use futures::{
//     future::{self, Loop},
//     Future, Stream,
// };
// use num_cpus;
// use pattern_matcher::Pattern;
// use rand::{self, rngs::ThreadRng, Rng};
// use std::error::Error as StdError;
// use std::{
//     net::SocketAddr,
//     panic,
//     sync::Arc,
//     time::{Duration, Instant},
// };
// use tokio::{
//     sync::oneshot::{self, Receiver, Sender},
//     timer::Delay,
// };
//
// const NUM_MESSAGES: u32 = 100;
// const FREQUENCY: u64 = 1; // in milliseconds
//
// struct User {
//     conn: Conn,
//     namespace: Pattern,
//     send_count: u32,
//     msg_count: Vec<(Arc<RelaxedCounter>, Option<Receiver<()>>)>,
//     detonator: Option<Sender<()>>,
// }
//
// struct Subscription {
//     namespace: Pattern,
//     counter: Arc<RelaxedCounter>,
//     detonator: Option<Sender<()>>,
// }
//
// impl User {
//     fn new(
//         broker_addr: SocketAddr,
//         namespace: Pattern,
//         subscriptions: Vec<Pattern>,
//     ) -> impl Future<Item = User, Error = Error> {
//         let namespace_c = namespace.clone();
//         Conn::new(broker_addr).and_then(move |mut conn| {
//             // Publish provided namespace
//             conn.provide(namespace_c).unwrap();
//
//             // Setup detonator channel
//             let (det_tx, det_rx) = oneshot::channel();
//
//             // Keep record of received messages
//             let mut msg_count = Vec::new();
//
//             // Subscribe to other namespaces
//             for sub in subscriptions {
//                 let (tx, rx) = oneshot::channel();
//                 let counter = Arc::new(RelaxedCounter::new(0));
//                 msg_count.push((counter.clone(), Some(rx)));
//
//                 tokio::spawn(
//                     conn.subscribe(sub)
//                         .unwrap()
//                         .for_each(move |_| {
//                             counter.inc();
//                             Ok(())
//                         })
//                         .select(
//                             det_rx
//                                 .map_err(|e| panic!(e))
//                                 .and_then(|_| Delay::new(Instant::now() + Duration::from_secs(2)))
//                                 .map_err(|e| panic!(e)),
//                         )
//                         .and_then(|_| {
//                             tx.send(()).unwrap();
//                             Ok(())
//                         })
//                         .map_err(|_| ()),
//                 );
//             }
//
//             Ok(User {
//                 conn,
//                 namespace,
//                 send_count: 0,
//                 msg_count,
//                 detonator: Some(det_tx),
//             })
//         })
//     }
//
//     fn send(mut self) -> impl Future<Item = Self, Error = Error> {
//         self.conn.event(self.namespace.clone(), "message").unwrap();
//         self.send_count += 1;
//         Delay::new(Instant::now() + Duration::from_millis(FREQUENCY))
//             .map(|_| self)
//             .map_err(|e| e.description().into())
//     }
//
//     fn run(self) -> impl Future<Item = Self, Error = Error> {
//         future::loop_fn(self, |me| {
//             me.send().and_then(|me| {
//                 if me.send_count < NUM_MESSAGES {
//                     Ok(Loop::Continue(me))
//                 } else {
//                     Ok(Loop::Break(me))
//                 }
//             })
//         })
//     }
//
//     fn assert_count(&mut self) -> impl Future<Item = (), Error = ()> + '_ {
//         let mut futs = Vec::new();
//         for (counter, rx) in self.msg_count.iter_mut() {
//             futs.push(rx.take().unwrap().and_then(move |_| {
//                 assert_eq!(counter.get(), NUM_MESSAGES as usize);
//                 Ok(())
//             }));
//         }
//         future::join_all(futs).map(|_| ()).map_err(|_| ())
//     }
// }
//
// fn gen_namespace(rng: &mut ThreadRng, seed: &[char], length: u8) -> Pattern {
//     let seed_len = seed.len();
//     let mut nsbuf = vec!['/'];
//     for _ in 0..length {
//         nsbuf.push(seed[rng.gen_range(0, seed_len)]);
//     }
//
//     let ns: String = nsbuf.into_iter().collect();
//     ns.into()
// }
//
// #[test]
// #[ignore]
// fn test_load() {
//     let num_users = num_cpus::get();
//
//     // Generate namespaces
//     let seed = (b'A'..=b'z')
//         .map(|c| c as char)
//         .filter(|c| c.is_alphabetic())
//         .collect::<Vec<_>>();
//     let mut rng = rand::thread_rng();
//     let mut namespaces = Vec::new();
//     for _ in 0..num_users {
//         namespaces.push(gen_namespace(&mut rng, &seed, 5));
//     }
//
//     let f = |port| {
//         let mut futs = Vec::new();
//
//         // This loop caps the number of users at the number of items in `alphabet`
//         for namespace in namespaces.iter() {
//             let addr = format!("127.0.0.1:{}", port);
//             let fut = User::new(
//                 addr.parse().unwrap(),
//                 namespace.clone(),
//                 namespaces
//                     .iter()
//                     .filter(|n| *n != namespace)
//                     .cloned()
//                     .collect(),
//             )
//             .and_then(|user| user.run())
//             .and_then(|mut user| {
//                 user.assert_count();
//                 Ok(())
//             })
//             .map_err(|e| panic!(e));
//
//             futs.push(fut);
//         }
//
//         tokio::run(future::join_all(futs).map(|_| ()));
//     };
//     utils::run_client(f);
// }
