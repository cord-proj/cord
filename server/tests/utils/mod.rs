use env_logger;
use std::{
    env,
    io::Read,
    net::SocketAddr,
    path::PathBuf,
    process::{Child, Command, Stdio},
    str,
};

// Start the env_logger instance
#[allow(dead_code)]
pub fn init_log() {
    let _ = env_logger::builder().is_test(true).try_init();
}

pub fn start_server() -> (Child, SocketAddr) {
    let mut child = command().spawn().unwrap();

    // Get port number
    let mut bytes = [0; 5];
    child
        .stdout
        .as_mut()
        .unwrap()
        .read_exact(&mut bytes)
        .unwrap();
    let port: u16 = str::from_utf8(&bytes).unwrap().parse().unwrap();

    (child, format!("127.0.0.1:{}", port).parse().unwrap())
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
