use futures::future::Future;
use generators::{Executor, SimpleFactory};
use log::{error, info};
use tokio;

mod utils;

#[test]
#[ignore]
fn test_load() {
    utils::init_log();
    let f = |socket_addr| {
        let executor = Executor::new(SimpleFactory::default(), socket_addr);
        // executor.set_frequency(None);
        info!("{}", executor);
        tokio::run(executor.exec().map_err(|e| error!("{}", e)));
    };
    utils::run_client(f);
}
