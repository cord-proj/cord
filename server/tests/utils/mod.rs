use std::{
    env,
    io::Read,
    net::SocketAddr,
    panic,
    path::PathBuf,
    process::{Child, Command, Stdio},
    str,
};

pub fn run_client<F: FnOnce(SocketAddr) + panic::UnwindSafe>(f: F) {
    // Start a new server process
    let (mut server, port) = start_server();

    // Convert the port into a SocketAddr to be friendly to the caller
    let socket_addr = format!("127.0.0.1:{}", port).parse().unwrap();

    // Catch panics to ensure that we have the opportunity to terminate the server
    let result = panic::catch_unwind(|| f(socket_addr));

    // Terminate the server
    server.kill().expect("Server was not running");

    // Now we can resume panicking if needed
    if let Err(e) = result {
        panic::resume_unwind(e);
    }
}

fn start_server() -> (Child, u16) {
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

// Returns the path to the server executable
fn bin() -> PathBuf {
    if cfg!(windows) {
        root_dir().join("../server.exe")
    } else {
        root_dir().join("../server")
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
