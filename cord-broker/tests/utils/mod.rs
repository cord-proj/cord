use std::{
    env,
    io::Read,
    panic,
    path::PathBuf,
    process::{Child, Command, Stdio},
    str,
};

pub fn run_client<F: FnOnce(u16) + panic::UnwindSafe>(f: F) {
    // Start a new broker process
    let (mut broker, port) = start_broker();

    // Catch panics to ensure that we have the opportunity to terminate the broker
    let result = panic::catch_unwind(|| f(port));

    // Terminate the broker
    broker.kill().expect("Broker was not running");

    // Now we can resume panicking if needed
    if let Err(e) = result {
        panic::resume_unwind(e);
    }
}

fn start_broker() -> (Child, u16) {
    let mut child = command().spawn().unwrap();

    // Get port number
    let mut bytes = [0; 5];
    child
        .stdout
        .as_mut()
        .unwrap()
        .read_exact(&mut bytes)
        .unwrap();
    (child, str::from_utf8(&bytes).unwrap().parse().unwrap())
}

fn command() -> Command {
    let mut c = Command::new(&bin());
    c.args(&["-a 127.0.0.1", "-p 0"]).stdout(Stdio::piped());
    c
}

// Returns the path to the broker executable
fn bin() -> PathBuf {
    if cfg!(windows) {
        root_dir().join("../cord-broker.exe")
    } else {
        root_dir().join("../cord-broker")
    }
}

// Returns the path to the executable root directory
fn root_dir() -> PathBuf {
    env::current_exe()
        .unwrap()
        .parent()
        .expect("executable's directory")
        .to_path_buf()
}
