use crate::io;
use crate::os::fd::{FromRawFd, RawFd};
use crate::sys::err2io;
use crate::sys::fd::FileDesc;

pub type Pipe = FileDesc;

pub fn pipe() -> io::Result<(Pipe, Pipe)> {
    let (fd1, fd2) = unsafe { wasi::fd_pipe().map_err(err2io)? };
    unsafe { Ok((Pipe::from_raw_fd(fd1 as RawFd), Pipe::from_raw_fd(fd2 as RawFd))) }
}
