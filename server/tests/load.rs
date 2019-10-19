use futures::future::Future;
use generators::{Executor, SimpleFactory};
use tokio;

mod utils;

#[test]
#[ignore]
fn test_load() {
    let f = |socket_addr| {
        let executor = Executor::new(SimpleFactory::default(), socket_addr);
        tokio::run(executor.exec().map_err(|e| panic!(e)));
    };
    utils::run_client(f);
}
