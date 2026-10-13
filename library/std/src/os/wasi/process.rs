//! Wasi-specific extensions to primitives in the [`std::process`] module.
//!
//! [`std::process`]: crate::process

#![stable(feature = "rust1", since = "1.0.0")]
#![allow(dead_code, unused)]

use crate::os::wasi::io::{AsFd, AsRawFd, BorrowedFd, FromRawFd, IntoRawFd, OwnedFd, RawFd};
use crate::sys::{AsInner, FromInner, IntoInner};
use crate::{process, sys};

#[stable(feature = "process_extensions", since = "1.2.0")]
impl FromRawFd for process::Stdio {
    #[inline]
    unsafe fn from_raw_fd(fd: RawFd) -> process::Stdio {
        let fd = unsafe { sys::fd::FileDesc::from_raw_fd(fd) };
        let io = sys::process::Stdio::Fd(fd);
        process::Stdio::from_inner(io)
    }
}

#[stable(feature = "io_safety", since = "1.63.0")]
impl From<OwnedFd> for process::Stdio {
    #[inline]
    fn from(fd: OwnedFd) -> process::Stdio {
        let fd = sys::fd::FileDesc::from_inner(fd);
        let io = sys::process::Stdio::Fd(fd);
        process::Stdio::from_inner(io)
    }
}

#[stable(feature = "process_extensions", since = "1.2.0")]
impl AsRawFd for process::ChildStdin {
    #[inline]
    fn as_raw_fd(&self) -> RawFd {
        self.as_inner().as_raw_fd()
    }
}

#[stable(feature = "process_extensions", since = "1.2.0")]
impl AsRawFd for process::ChildStdout {
    #[inline]
    fn as_raw_fd(&self) -> RawFd {
        self.as_inner().as_raw_fd()
    }
}

#[stable(feature = "process_extensions", since = "1.2.0")]
impl AsRawFd for process::ChildStderr {
    #[inline]
    fn as_raw_fd(&self) -> RawFd {
        self.as_inner().as_raw_fd()
    }
}

#[stable(feature = "into_raw_os", since = "1.4.0")]
impl IntoRawFd for process::ChildStdin {
    #[inline]
    fn into_raw_fd(self) -> RawFd {
        self.into_inner().into_raw_fd()
    }
}

#[stable(feature = "into_raw_os", since = "1.4.0")]
impl IntoRawFd for process::ChildStdout {
    #[inline]
    fn into_raw_fd(self) -> RawFd {
        self.into_inner().into_raw_fd()
    }
}

#[stable(feature = "into_raw_os", since = "1.4.0")]
impl IntoRawFd for process::ChildStderr {
    #[inline]
    fn into_raw_fd(self) -> RawFd {
        self.into_inner().into_raw_fd()
    }
}

#[stable(feature = "io_safety", since = "1.63.0")]
impl AsFd for crate::process::ChildStdin {
    #[inline]
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.as_inner().as_fd()
    }
}

#[stable(feature = "io_safety", since = "1.63.0")]
impl From<crate::process::ChildStdin> for OwnedFd {
    #[inline]
    fn from(child_stdin: crate::process::ChildStdin) -> OwnedFd {
        child_stdin.into_inner().into_inner()
    }
}

#[stable(feature = "io_safety", since = "1.63.0")]
impl AsFd for crate::process::ChildStdout {
    #[inline]
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.as_inner().as_fd()
    }
}

#[stable(feature = "io_safety", since = "1.63.0")]
impl From<crate::process::ChildStdout> for OwnedFd {
    #[inline]
    fn from(child_stdout: crate::process::ChildStdout) -> OwnedFd {
        child_stdout.into_inner().into_inner()
    }
}

#[stable(feature = "io_safety", since = "1.63.0")]
impl AsFd for crate::process::ChildStderr {
    #[inline]
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.as_inner().as_fd()
    }
}

#[stable(feature = "io_safety", since = "1.63.0")]
impl From<crate::process::ChildStderr> for OwnedFd {
    #[inline]
    fn from(child_stderr: crate::process::ChildStderr) -> OwnedFd {
        child_stderr.into_inner().into_inner()
    }
}
