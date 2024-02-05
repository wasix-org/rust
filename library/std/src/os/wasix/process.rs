//! Wasi-specific extensions to primitives in the [`std::process`] module.
//!
//! [`std::process`]: crate::process

#![stable(feature = "rust1", since = "1.0.0")]
#![allow(dead_code, unused)]

use crate::os::wasi::io::{AsFd, AsRawFd, BorrowedFd, FromRawFd, IntoRawFd, OwnedFd, RawFd};
use crate::process;
use crate::sealed::Sealed;
use crate::io;
use crate::ffi::OsStr;
use crate::sys_common::AsInnerMut;

type UserId = u32;
type GroupId = u32;

/// Unix-specific extensions to the [`process::Command`] builder.
///
/// This trait is sealed: it cannot be implemented outside the standard library.
/// This is so that future additional methods are not breaking changes.
#[stable(feature = "rust1", since = "1.0.0")]
pub trait CommandExt: Sealed {
    /// Sets the child process's user ID. This translates to a
    /// `setuid` call in the child process. Failure in the `setuid`
    /// call will cause the spawn to fail.
    #[stable(feature = "rust1", since = "1.0.0")]
    fn uid(&mut self, id: UserId) -> &mut process::Command;

    /// Similar to `uid`, but sets the group ID of the child process. This has
    /// the same semantics as the `uid` field.
    #[stable(feature = "rust1", since = "1.0.0")]
    fn gid(&mut self, id: GroupId) -> &mut process::Command;

    /// Sets the supplementary group IDs for the calling process. Translates to
    /// a `setgroups` call in the child process.
    #[unstable(feature = "setgroups", issue = "90747")]
    fn groups(&mut self, groups: &[GroupId]) -> &mut process::Command;

    /// Schedules a closure to be run just before the `exec` function is
    /// invoked.
    ///
    /// The closure is allowed to return an I/O error whose OS error code will
    /// be communicated back to the parent and returned as an error from when
    /// the spawn was requested.
    ///
    /// Multiple closures can be registered and they will be called in order of
    /// their registration. If a closure returns `Err` then no further closures
    /// will be called and the spawn operation will immediately return with a
    /// failure.
    ///
    /// # Notes and Safety
    ///
    /// This closure will be run in the context of the child process after a
    /// `fork`. This primarily means that any modifications made to memory on
    /// behalf of this closure will **not** be visible to the parent process.
    /// This is often a very constrained environment where normal operations
    /// like `malloc`, accessing environment variables through [`std::env`]
    /// or acquiring a mutex are not guaranteed to work (due to
    /// other threads perhaps still running when the `fork` was run).
    ///
    /// For further details refer to the [POSIX fork() specification]
    /// and the equivalent documentation for any targeted
    /// platform, especially the requirements around *async-signal-safety*.
    ///
    /// This also means that all resources such as file descriptors and
    /// memory-mapped regions got duplicated. It is your responsibility to make
    /// sure that the closure does not violate library invariants by making
    /// invalid use of these duplicates.
    ///
    /// Panicking in the closure is safe only if all the format arguments for the
    /// panic message can be safely formatted; this is because although
    /// `Command` calls [`std::panic::always_abort`](crate::panic::always_abort)
    /// before calling the pre_exec hook, panic will still try to format the
    /// panic message.
    ///
    /// When this closure is run, aspects such as the stdio file descriptors and
    /// working directory have successfully been changed, so output to these
    /// locations might not appear where intended.
    ///
    /// [POSIX fork() specification]:
    ///     https://pubs.opengroup.org/onlinepubs/9699919799/functions/fork.html
    /// [`std::env`]: mod@crate::env
    #[stable(feature = "process_pre_exec", since = "1.34.0")]
    unsafe fn pre_exec<F>(&mut self, f: F) -> &mut process::Command
    where
        F: FnMut() -> io::Result<()> + Send + Sync + 'static;

    /// Schedules a closure to be run just before the `exec` function is
    /// invoked.
    ///
    /// This method is stable and usable, but it should be unsafe. To fix
    /// that, it got deprecated in favor of the unsafe [`pre_exec`].
    ///
    /// [`pre_exec`]: CommandExt::pre_exec
    #[stable(feature = "process_exec", since = "1.15.0")]
    #[deprecated(since = "1.37.0", note = "should be unsafe, use `pre_exec` instead")]
    fn before_exec<F>(&mut self, f: F) -> &mut process::Command
    where
        F: FnMut() -> io::Result<()> + Send + Sync + 'static,
    {
        unsafe { self.pre_exec(f) }
    }

    /// Performs all the required setup by this `Command`, followed by calling
    /// the `execvp` syscall.
    ///
    /// On success this function will not return, and otherwise it will return
    /// an error indicating why the exec (or another part of the setup of the
    /// `Command`) failed.
    ///
    /// `exec` not returning has the same implications as calling
    /// [`process::exit`] – no destructors on the current stack or any other
    /// thread’s stack will be run. Therefore, it is recommended to only call
    /// `exec` at a point where it is fine to not run any destructors. Note,
    /// that the `execvp` syscall independently guarantees that all memory is
    /// freed and all file descriptors with the `CLOEXEC` option (set by default
    /// on all file descriptors opened by the standard library) are closed.
    ///
    /// This function, unlike `spawn`, will **not** `fork` the process to create
    /// a new child. Like spawn, however, the default behavior for the stdio
    /// descriptors will be to inherited from the current process.
    ///
    /// # Notes
    ///
    /// The process may be in a "broken state" if this function returns in
    /// error. For example the working directory, environment variables, signal
    /// handling settings, various user/group information, or aspects of stdio
    /// file descriptors may have changed. If a "transactional spawn" is
    /// required to gracefully handle errors it is recommended to use the
    /// cross-platform `spawn` instead.
    #[stable(feature = "process_exec2", since = "1.9.0")]
    fn exec(&mut self) -> io::Error;

    /// Set executable argument
    ///
    /// Set the first process argument, `argv[0]`, to something other than the
    /// default executable path.
    #[stable(feature = "process_set_argv0", since = "1.45.0")]
    fn arg0<S>(&mut self, arg: S) -> &mut process::Command
    where
        S: AsRef<OsStr>;

    /// Sets the process group ID (PGID) of the child process. Equivalent to a
    /// `setpgid` call in the child process, but may be more efficient.
    ///
    /// Process groups determine which processes receive signals.
    ///
    /// # Examples
    ///
    /// Pressing Ctrl-C in a terminal will send SIGINT to all processes in
    /// the current foreground process group. By spawning the `sleep`
    /// subprocess in a new process group, it will not receive SIGINT from the
    /// terminal.
    ///
    /// The parent process could install a signal handler and manage the
    /// subprocess on its own terms.
    ///
    /// A process group ID of 0 will use the process ID as the PGID.
    ///
    /// ```no_run
    /// use std::process::Command;
    /// use std::os::unix::process::CommandExt;
    ///
    /// Command::new("sleep")
    ///     .arg("10")
    ///     .process_group(0)
    ///     .spawn()?
    ///     .wait()?;
    /// #
    /// # Ok::<_, Box<dyn std::error::Error>>(())
    /// ```
    #[stable(feature = "process_set_process_group", since = "1.64.0")]
    fn process_group(&mut self, pgroup: i32) -> &mut process::Command;
}

#[stable(feature = "rust1", since = "1.0.0")]
impl CommandExt for process::Command {
    fn uid(&mut self, id: UserId) -> &mut process::Command {
        self.as_inner_mut().uid(id);
        self
    }

    fn gid(&mut self, id: GroupId) -> &mut process::Command {
        self.as_inner_mut().gid(id);
        self
    }

    fn groups(&mut self, groups: &[GroupId]) -> &mut process::Command {
        self.as_inner_mut().groups(groups);
        self
    }

    unsafe fn pre_exec<F>(&mut self, f: F) -> &mut process::Command
    where
        F: FnMut() -> io::Result<()> + Send + Sync + 'static,
    {
        unsafe { self.as_inner_mut().pre_exec(Box::new(f)) };
        self
    }

    fn exec(&mut self) -> io::Error {
        // NOTE: This may *not* be safe to call after `libc::fork`, because it
        // may allocate. That may be worth fixing at some point in the future.
        self.as_inner_mut().exec(crate::sys::process::Stdio::Inherit)
    }

    fn arg0<S>(&mut self, arg: S) -> &mut process::Command
    where
        S: AsRef<OsStr>,
    {
        self.as_inner_mut().set_arg_0(arg.as_ref());
        self
    }

    fn process_group(&mut self, pgroup: i32) -> &mut process::Command {
        self.as_inner_mut().pgroup(pgroup);
        self
    }
}

#[stable(feature = "process_extensions", since = "1.2.0")]
impl FromRawFd for process::Stdio {
    #[inline]
    unsafe fn from_raw_fd(fd: RawFd) -> process::Stdio {
        unimplemented!()
    }
}

#[stable(feature = "io_safety", since = "1.63.0")]
impl From<OwnedFd> for process::Stdio {
    #[inline]
    fn from(fd: OwnedFd) -> process::Stdio {
        unimplemented!()
    }
}

#[stable(feature = "process_extensions", since = "1.2.0")]
impl AsRawFd for process::ChildStdin {
    #[inline]
    fn as_raw_fd(&self) -> RawFd {
        unimplemented!()
    }
}

#[stable(feature = "process_extensions", since = "1.2.0")]
impl AsRawFd for process::ChildStdout {
    #[inline]
    fn as_raw_fd(&self) -> RawFd {
        unimplemented!()
    }
}

#[stable(feature = "process_extensions", since = "1.2.0")]
impl AsRawFd for process::ChildStderr {
    #[inline]
    fn as_raw_fd(&self) -> RawFd {
        unimplemented!()
    }
}

#[stable(feature = "into_raw_os", since = "1.4.0")]
impl IntoRawFd for process::ChildStdin {
    #[inline]
    fn into_raw_fd(self) -> RawFd {
        unimplemented!()
    }
}

#[stable(feature = "into_raw_os", since = "1.4.0")]
impl IntoRawFd for process::ChildStdout {
    #[inline]
    fn into_raw_fd(self) -> RawFd {
        unimplemented!()
    }
}

#[stable(feature = "into_raw_os", since = "1.4.0")]
impl IntoRawFd for process::ChildStderr {
    #[inline]
    fn into_raw_fd(self) -> RawFd {
        unimplemented!()
    }
}

#[stable(feature = "io_safety", since = "1.63.0")]
impl AsFd for crate::process::ChildStdin {
    #[inline]
    fn as_fd(&self) -> BorrowedFd<'_> {
        unimplemented!()
    }
}

#[stable(feature = "io_safety", since = "1.63.0")]
impl From<crate::process::ChildStdin> for OwnedFd {
    #[inline]
    fn from(child_stdin: crate::process::ChildStdin) -> OwnedFd {
        unimplemented!()
    }
}

#[stable(feature = "io_safety", since = "1.63.0")]
impl AsFd for crate::process::ChildStdout {
    #[inline]
    fn as_fd(&self) -> BorrowedFd<'_> {
        unimplemented!()
    }
}

#[stable(feature = "io_safety", since = "1.63.0")]
impl From<crate::process::ChildStdout> for OwnedFd {
    #[inline]
    fn from(child_stdout: crate::process::ChildStdout) -> OwnedFd {
        unimplemented!()
    }
}

#[stable(feature = "io_safety", since = "1.63.0")]
impl AsFd for crate::process::ChildStderr {
    #[inline]
    fn as_fd(&self) -> BorrowedFd<'_> {
        unimplemented!()
    }
}

#[stable(feature = "io_safety", since = "1.63.0")]
impl From<crate::process::ChildStderr> for OwnedFd {
    #[inline]
    fn from(child_stderr: crate::process::ChildStderr) -> OwnedFd {
        unimplemented!()
    }
}
