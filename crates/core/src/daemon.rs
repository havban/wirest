//! The root helper's server loop (the `wirest-helper` binary). Like
//! Tailscale's system extension it is installed once; afterwards frontends
//! connect and disconnect through it without password prompts.

use crate::install;
use crate::ops;
use crate::protocol::{file_build_id, Request, Response, SOCKET};
use crate::wg::Tools;
use std::fs::Permissions;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::{Arc, Mutex};

/// Serves requests forever. The socket is `root:<socket_gid>` `0660`: only
/// that group's users (macOS: staff; Linux: the installing user's group) can
/// ask the helper to bring tunnels up or down.
pub fn run(socket_gid: u32) -> ! {
    let tools = Arc::new(Tools::in_dir(install::bin_dir()));
    let build = Arc::new(
        std::env::current_exe().map(|p| file_build_id(&p)).unwrap_or_default(),
    );
    ops::reap_orphans(&tools);
    std::fs::remove_file(SOCKET).ok();
    let listener = match UnixListener::bind(SOCKET) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("wirest-helper: cannot bind {SOCKET}: {e}");
            std::process::exit(1);
        }
    };
    std::os::unix::fs::chown(SOCKET, Some(0), Some(socket_gid)).ok();
    std::fs::set_permissions(SOCKET, Permissions::from_mode(0o660)).ok();
    let lock = Arc::new(Mutex::new(()));
    for stream in listener.incoming().flatten() {
        let (tools, lock, build) = (tools.clone(), lock.clone(), build.clone());
        std::thread::spawn(move || handle(stream, &tools, &lock, &build));
    }
    std::process::exit(1);
}

fn handle(stream: UnixStream, tools: &Tools, lock: &Mutex<()>, build: &str) {
    let mut line = String::new();
    if BufReader::new(&stream).read_line(&mut line).is_err() {
        return;
    }
    let result = match serde_json::from_str::<Request>(&line) {
        Ok(Request::Version) => Ok(String::new()),
        Ok(Request::Up { name, config }) => {
            let _g = lock.lock().unwrap_or_else(|e| e.into_inner());
            ops::up(tools, &name, &config)
        }
        Ok(Request::Down { name }) => {
            let _g = lock.lock().unwrap_or_else(|e| e.into_inner());
            ops::down(tools, &name)
        }
        Err(e) => Err(format!("bad request: {e}")),
    };
    let (ok, output) = match result {
        Ok(o) => (true, o),
        Err(e) => (false, e),
    };
    let resp = Response { ok, output, build: build.to_owned(), active: ops::active(tools) };
    let mut out = serde_json::to_string(&resp).expect("response serializes");
    out.push('\n');
    (&stream).write_all(out.as_bytes()).ok();
}
