use std::{
    env,
    io::Read,
    path::PathBuf,
    process::{Child, Command, Stdio},
    str,
};

pub fn start_server() -> (Child, u16) {
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
