use std::{
    fs, io,
    io::{BufRead, BufReader, Write},
    os::unix::net::{UnixListener, UnixStream},
};

use hz_core::{HzError, HzResult};

use crate::{
    client::{is_not_running_error, request},
    paths::{pid_path, prepare_runtime_dir, remove_stale_socket, socket_path},
    protocol::{CONNECT_TIMEOUT, DaemonReply},
    state::DaemonState,
};

pub fn run_foreground() -> HzResult<()> {
    prepare_runtime_dir()?;
    match request("PING") {
        Ok(_) => return Err(HzError::Usage("hz daemon already running".to_owned())),
        Err(error) if is_not_running_error(&error) => {}
        Err(error) => return Err(error),
    }
    remove_stale_socket()?;

    let socket_path = socket_path()?;
    let pid_path = pid_path()?;
    let listener = UnixListener::bind(&socket_path)?;
    fs::write(&pid_path, std::process::id().to_string())?;

    let result = serve(listener);

    let _ = fs::remove_file(socket_path);
    let _ = fs::remove_file(pid_path);

    result
}

fn serve(listener: UnixListener) -> HzResult<()> {
    let mut state = DaemonState::new()?;

    for stream in listener.incoming() {
        match stream {
            Ok(stream) => match handle_client(stream, &mut state) {
                Ok(true) => break,
                Ok(false) => {}
                Err(_) => continue,
            },
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error.into()),
        }
    }

    Ok(())
}

fn handle_client(mut stream: UnixStream, state: &mut DaemonState) -> io::Result<bool> {
    let _ = stream.set_read_timeout(Some(CONNECT_TIMEOUT));
    let _ = stream.set_write_timeout(Some(CONNECT_TIMEOUT));

    let mut request = String::new();
    let mut reader = BufReader::new(stream.try_clone()?);
    reader.read_line(&mut request)?;

    let reply = state.handle(request.trim_end_matches(['\r', '\n']));
    let stop = matches!(reply, DaemonReply::Stop(_));
    stream.write_all(reply.line().as_bytes())?;
    stream.write_all(b"\n")?;
    Ok(stop)
}
