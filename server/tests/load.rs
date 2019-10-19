use futures::future::Future;
use generators::{Executor, SimpleFactory};
use log::info;
use tokio;

mod utils;

#[test]
#[ignore]
fn test_load() {
    utils::init_log();
    let f = |socket_addr| {
        let executor = Executor::new(SimpleFactory::default(), socket_addr);
        info!("{}", executor);
        tokio::run(executor.exec().map_err(|e| panic!(e)));
    };
    utils::run_client(f);
}
