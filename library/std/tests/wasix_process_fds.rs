#![cfg(all(target_os = "wasi", target_vendor = "wasmer"))]

use std::env;
use std::os::wasi::io::{AsFd, AsRawFd, FromRawFd, IntoRawFd, OwnedFd};
use std::process::{Command, Stdio};

fn assert_borrowed_fd<T: AsFd + AsRawFd>(stream: &T) {
    assert_eq!(stream.as_raw_fd(), stream.as_fd().as_raw_fd());
}

fn stdio_through_raw_fd<T: IntoRawFd>(stream: T) -> Stdio {
    let fd = stream.into_raw_fd();
    unsafe { Stdio::from_raw_fd(fd) }
}

fn stdio_through_owned_fd<T: Into<OwnedFd>>(stream: T) -> Stdio {
    Stdio::from(stream.into())
}

#[test]
fn child_stream_fd_conversions_transfer_ownership() {
    const CHILD: &str = "RUST_STD_WASIX_PROCESS_FD_CHILD";
    if env::var_os(CHILD).is_some() {
        return;
    }

    let mut child = Command::new(env::current_exe().unwrap())
        .arg("--exact")
        .arg("child_stream_fd_conversions_transfer_ownership")
        .env(CHILD, "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();

    let stdin = child.stdin.take().unwrap();
    assert_borrowed_fd(&stdin);
    drop(stdio_through_raw_fd(stdin));

    let stdout = child.stdout.take().unwrap();
    assert_borrowed_fd(&stdout);
    drop(stdio_through_owned_fd(stdout));

    let stderr = child.stderr.take().unwrap();
    assert_borrowed_fd(&stderr);
    drop(stdio_through_owned_fd(stderr));

    assert!(child.wait().unwrap().success());
}
