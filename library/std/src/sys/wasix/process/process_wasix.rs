#![allow(unused, dead_code)]
use crate::fmt;
use crate::io;
use crate::num::NonZeroI32;
use crate::os::fd::FromRawFd;
use crate::sys::pipe::AnonPipe;
use crate::sys::process::process_common::*;
use crate::sys::{unsupported, unsupported_err};
use core::ffi::{CStr, NonZero_c_int};
use wasi::{JoinStatus, JoinStatusType};

use crate::io::ErrorKind;

use libc::{c_int, pid_t};

pub use crate::sys::{cvt, cvt_nz, cvt_r};

fn to_anon_pipe(option_fd: wasi::OptionFd) -> Option<AnonPipe> {
    match wasi::Option::from(option_fd.tag) {
        wasi::OPTION_SOME => Some(unsafe { AnonPipe::from_raw_fd(option_fd.u.some as i32) }),
        _ => None,
    }
}

fn to_wasi_mode(input: &Stdio) -> wasi::StdioMode {
    match input {
        Stdio::Inherit => wasi::STDIO_MODE_INHERIT,
        Stdio::MakePipe => wasi::STDIO_MODE_PIPED,
        _ => wasi::STDIO_MODE_NULL,
    }
}

////////////////////////////////////////////////////////////////////////////////
// Command
////////////////////////////////////////////////////////////////////////////////

impl Command {
    pub fn spawn(
        &mut self,
        default: Stdio,
        needs_stdin: bool,
    ) -> io::Result<(Process, StdioPipes)> {
        let null = Stdio::Null;
        let default_stdin = if needs_stdin { &default } else { &null };
        let program = self
            .get_program()
            .to_str()
            .ok_or_else(|| io::const_io_error!(ErrorKind::Other, "Spawn failed",))?;

        let handle = unsafe {
            wasi::proc_spawn(
                program,
                wasi::BOOL_FALSE,
                &self.get_argv_string(),
                "",
                to_wasi_mode(self.get_stdin().unwrap_or(default_stdin)),
                to_wasi_mode(self.get_stdout().unwrap_or(&default)),
                to_wasi_mode(self.get_stderr().unwrap_or(&default)),
                ".",
            )
        }
        .map_err(|_| io::const_io_error!(ErrorKind::Other, "Spawn failed",))?;

        Ok((
            Process { pid: handle.pid as i32, status: None },
            StdioPipes {
                stdin: to_anon_pipe(handle.stdin),
                stderr: to_anon_pipe(handle.stderr),
                stdout: to_anon_pipe(handle.stdout),
            },
        ))
    }

    fn get_argv_string(&self) -> String {
        let argv = self
            .get_argv()
            .into_iter()
            .map(|p| unsafe { CStr::from_ptr(p.clone()) }.to_string_lossy())
            .collect::<Vec<_>>();
        argv.join("\n")
    }

    pub fn output(&mut self) -> io::Result<(ExitStatus, Vec<u8>, Vec<u8>)> {
        let (proc, pipes) = self.spawn(Stdio::MakePipe, false)?;
        crate::sys_common::process::wait_with_output(proc, pipes)
    }

    pub fn exec(&mut self, default: Stdio) -> io::Error {
        let envp = self.capture_env();

        if self.saw_nul() {
            return io::const_io_error!(ErrorKind::InvalidInput, "nul byte found in provided data",);
        }

        match self.setup_io(default, true) {
            Ok((_, theirs)) => unsafe {
                let Err(e) = self.do_exec(theirs, envp.as_ref());
                e
            },
            Err(e) => e,
        }
    }

    unsafe fn do_exec(
        &mut self,
        stdio: ChildPipes,
        maybe_envp: Option<&CStringArray>,
    ) -> Result<!, io::Error> {
        use crate::sys::{self, cvt_r};

        if let Some(fd) = stdio.stdin.fd() {
            cvt_r(|| libc::dup2(fd, libc::STDIN_FILENO))?;
        }
        if let Some(fd) = stdio.stdout.fd() {
            cvt_r(|| libc::dup2(fd, libc::STDOUT_FILENO))?;
        }
        if let Some(fd) = stdio.stderr.fd() {
            cvt_r(|| libc::dup2(fd, libc::STDERR_FILENO))?;
        }

        if let Some(ref cwd) = *self.get_cwd() {
            cvt(libc::chdir(cwd.as_ptr()))?;
        }

        for callback in self.get_closures().iter_mut() {
            callback()?;
        }

        libc::execvp(self.get_program_cstr().as_ptr(), self.get_argv().as_ptr());
        Err(io::Error::last_os_error())
    }
}

////////////////////////////////////////////////////////////////////////////////
// Processes
////////////////////////////////////////////////////////////////////////////////

pub struct Process {
    pid: pid_t,
    status: Option<ExitStatus>,
}

impl Process {
    unsafe fn new(pid: pid_t, _pidfd: pid_t) -> Self {
        Process { pid, status: None }
    }

    pub fn id(&self) -> u32 {
        self.pid as u32
    }

    pub fn kill(&mut self) -> io::Result<()> {
        // If we've already waited on this process then the pid can be recycled
        // and used for another process, and we probably shouldn't be killing
        // random processes, so just return an error.
        if self.status.is_some() {
            Err(io::const_io_error!(
                ErrorKind::InvalidInput,
                "invalid argument: can't kill an exited process",
            ))
        } else {
            unsafe { wasi::proc_signal(self.pid as u32, wasi::SIGNAL_KILL) }
                .map_err(|_| io::const_io_error!(ErrorKind::Other, "Kill failed",))
        }
    }

    pub fn wait(&mut self) -> io::Result<ExitStatus> {
        if let Some(status) = self.status {
            return Ok(status);
        }

        let mut pid = wasi::OptionPid { tag: 1, u: wasi::OptionPidU { some: self.pid as u32 } };

        let join_status = unsafe { wasi::proc_join(&mut pid, 0) }
            .map_err(|_| io::const_io_error!(ErrorKind::Other, "Join failed",))?;

        let status = ExitStatus(join_status);

        self.status = Some(status.clone());
        Ok(status)
    }

    pub fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        if let s @ Some(status) = self.status {
            return Ok(s);
        }

        let mut pid = wasi::OptionPid { tag: 1, u: wasi::OptionPidU { some: self.pid as u32 } };

        let join_status = unsafe { wasi::proc_join(&mut pid, wasi::JOIN_FLAGS_NON_BLOCKING) }
            .map_err(|_| io::const_io_error!(ErrorKind::Other, "Join failed",))?;

        let status = ExitStatus(join_status);

        if status.code().is_some() || status.signal().is_some() {
            self.status = Some(status.clone());
            Ok(Some(status))
        } else {
            Ok(None)
        }
    }
}

#[derive(Clone, Copy)]
pub struct ExitStatus(JoinStatus);

impl PartialEq<ExitStatus> for ExitStatus {
    fn eq(&self, other: &Self) -> bool {
        self.code().eq(&other.code())
    }

    fn ne(&self, other: &Self) -> bool {
        self.code().ne(&other.code())
    }
}

impl Eq for ExitStatus {}

impl fmt::Debug for ExitStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("wasix_wait_status")
            .field(&JoinStatusType::from(self.0.tag).message())
            .finish()
    }
}

impl ExitStatus {
    pub fn new(status: JoinStatus) -> ExitStatus {
        ExitStatus(status)
    }

    pub fn exit_ok(&self) -> Result<(), ExitStatusError> {
        if self.code() == Some(0) {
            Ok(())
        } else {
            Err(ExitStatusError(self.0))
        }
    }

    pub fn code(&self) -> Option<i32> {
        unsafe {
            match JoinStatusType::from(self.0.tag) {
                wasi::JOIN_STATUS_TYPE_EXIT_NORMAL => Some(self.0.u.exit_normal.raw() as i32),
                wasi::JOIN_STATUS_TYPE_EXIT_SIGNAL => {
                    Some(self.0.u.exit_signal.exit_code.raw() as i32)
                }
                _ => None,
            }
        }
    }

    pub fn signal(&self) -> Option<i32> {
        unsafe {
            match JoinStatusType::from(self.0.tag) {
                wasi::JOIN_STATUS_TYPE_STOPPED => Some(self.0.u.stopped.raw() as i32),
                wasi::JOIN_STATUS_TYPE_EXIT_SIGNAL => {
                    Some(self.0.u.exit_signal.signal.raw() as i32)
                }
                _ => None,
            }
        }
    }

    pub fn core_dumped(&self) -> bool {
        self.code().map(|c| libc::WIFSIGNALED(c) && libc::WCOREDUMP(c)).unwrap_or(false)
    }

    pub fn stopped_signal(&self) -> Option<i32> {
        unsafe {
            match JoinStatusType::from(self.0.tag) {
                wasi::JOIN_STATUS_TYPE_STOPPED => Some(self.0.u.stopped.raw() as i32),
                _ => None,
            }
        }
    }

    pub fn continued(&self) -> bool {
        self.code().map(|c| libc::WIFCONTINUED(c)).unwrap_or(false)
    }

    pub fn into_raw(&self) -> JoinStatus {
        self.0
    }
}

impl From<JoinStatus> for ExitStatus {
    fn from(s: JoinStatus) -> ExitStatus {
        ExitStatus(s)
    }
}

impl fmt::Display for ExitStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", JoinStatusType::from(self.0.tag).message())
    }
}

#[derive(Clone, Copy)]
pub struct ExitStatusError(JoinStatus);

impl fmt::Debug for ExitStatusError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("wasix_wait_status_error")
            .field(&JoinStatusType::from(self.0.tag).message())
            .finish()
    }
}

impl PartialEq<ExitStatusError> for ExitStatusError {
    fn eq(&self, other: &Self) -> bool {
        self.code().eq(&other.code())
    }

    fn ne(&self, other: &Self) -> bool {
        self.code().ne(&other.code())
    }
}

impl Eq for ExitStatusError {}

impl Into<ExitStatus> for ExitStatusError {
    fn into(self) -> ExitStatus {
        ExitStatus(self.0)
    }
}

impl ExitStatusError {
    pub fn code(self) -> Option<NonZeroI32> {
        ExitStatus(self.0).code().map(|st| st.try_into().unwrap())
    }
}
