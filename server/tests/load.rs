use futures::TryFutureExt;
use generators::{Executor, SimpleFactory};
use log::{error, info};
use std::panic;
use tokio;

mod utils;

#[tokio::test]
#[ignore]
async fn test_load() {
    // Start logger
    utils::init_log();

    // Start a new server process
    let (mut server, socket_addr) = utils::start_server();

    // Catch panics to ensure that we have the opportunity to terminate the server
    let result = panic::catch_unwind(|| async {
        let mut executor = Executor::new(SimpleFactory::default(), socket_addr);
        executor.set_frequency(None);
        info!("{}", executor);
        executor.exec().map_err(|e| error!("{}", e)).await
    });

    // Terminate the server
    server.kill().expect("Server was not running");

    // Now we can resume panicking if needed
    if let Err(e) = result {
        panic::resume_unwind(e);
    }
}
