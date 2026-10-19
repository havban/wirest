//! `wirest-helper`: Wirest's root daemon. Installed and started by the
//! frontends (LaunchDaemon on macOS, systemd on Linux); not meant to be run
//! by hand.

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.as_slice() {
        [flag, gid] if flag == "--socket-gid" => match gid.parse() {
            Ok(gid) => wirest_core::daemon::run(gid),
            Err(_) => fail("--socket-gid needs a numeric group id"),
        },
        [flag] if flag == "--version" => println!("wirest-helper {}", env!("CARGO_PKG_VERSION")),
        _ => fail("usage: wirest-helper --socket-gid <gid>   (started by Wirest; see `wirest-tui helper`)"),
    }
}

fn fail(msg: &str) -> ! {
    eprintln!("{msg}");
    std::process::exit(2);
}
