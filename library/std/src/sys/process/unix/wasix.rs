#![allow(unused, dead_code)]

use libc::{c_int, pid_t};
use wasi::{JoinStatus, JoinStatusType};

use super::common::{sigaddset, sigemptyset, *};
use crate::io::ErrorKind;
use crate::mem::MaybeUninit;
use crate::num::NonZeroI32;
pub use crate::sys::{cvt, cvt_nz, cvt_r};
use crate::sys::{self, unsupported_err};
use crate::{fmt, io};

////////////////////////////////////////////////////////////////////////////////
// Command
////////////////////////////////////////////////////////////////////////////////

impl Command {
    pub fn spawn(
        &mut self,
        default: Stdio,
        needs_stdin: bool,
    ) -> io::Result<(Process, StdioPipes)> {
        let envp = self.capture_env();

        if self.saw_nul() {
            return Err(io::const_error!(
                ErrorKind::InvalidInput,
                "nul byte found in provided data",
            ));
        }

        let (ours, theirs) = self.setup_io(default, needs_stdin)?;
        let p = self.posix_spawn(&theirs, envp.as_ref())?;
        Ok((p, ours))
    }

    fn posix_spawn(
        &mut self,
        stdio: &ChildPipes,
        envp: Option<&CStringArray>,
    ) -> io::Result<Process> {
        if self.get_gid().is_some()
            || self.get_uid().is_some()
            || !self.get_closures().is_empty()
            || self.get_groups().is_some()
            || self.get_chroot().is_some()
            || self.get_pgroup().is_some()
            || self.get_setsid()
        {
            return Err(unsupported_err());
        }

        struct PosixSpawnFileActions<'a>(&'a mut MaybeUninit<libc::posix_spawn_file_actions_t>);

        impl Drop for PosixSpawnFileActions<'_> {
            fn drop(&mut self) {
                unsafe {
                    libc::posix_spawn_file_actions_destroy(self.0.as_mut_ptr());
                }
            }
        }

        struct PosixSpawnattr<'a>(&'a mut MaybeUninit<libc::posix_spawnattr_t>);

        impl Drop for PosixSpawnattr<'_> {
            fn drop(&mut self) {
                unsafe {
                    libc::posix_spawnattr_destroy(self.0.as_mut_ptr());
                }
            }
        }

        unsafe {
            let mut attrs = MaybeUninit::uninit();
            cvt_nz(libc::posix_spawnattr_init(attrs.as_mut_ptr()))?;
            let attrs = PosixSpawnattr(&mut attrs);

            let mut flags = 0;

            let mut file_actions = MaybeUninit::uninit();
            cvt_nz(libc::posix_spawn_file_actions_init(file_actions.as_mut_ptr()))?;
            let file_actions = PosixSpawnFileActions(&mut file_actions);

            if let Some(fd) = stdio.stdin.fd() {
                cvt_nz(libc::posix_spawn_file_actions_adddup2(
                    file_actions.0.as_mut_ptr(),
                    fd,
                    libc::STDIN_FILENO,
                ))?;
            }
            if let Some(fd) = stdio.stdout.fd() {
                cvt_nz(libc::posix_spawn_file_actions_adddup2(
                    file_actions.0.as_mut_ptr(),
                    fd,
                    libc::STDOUT_FILENO,
                ))?;
            }
            if let Some(fd) = stdio.stderr.fd() {
                cvt_nz(libc::posix_spawn_file_actions_adddup2(
                    file_actions.0.as_mut_ptr(),
                    fd,
                    libc::STDERR_FILENO,
                ))?;
            }
            if let Some(cwd) = self.get_cwd() {
                cvt_nz(libc::posix_spawn_file_actions_addchdir_np(
                    file_actions.0.as_mut_ptr(),
                    cwd.as_ptr(),
                ))?;
            }

            // Reset SIGPIPE to SIG_DFL in the child for backward compatibility.
            let mut default_set = MaybeUninit::<libc::sigset_t>::uninit();
            cvt(sigemptyset(default_set.as_mut_ptr()))?;
            cvt(sigaddset(default_set.as_mut_ptr(), libc::SIGPIPE))?;
            cvt_nz(libc::posix_spawnattr_setsigdefault(
                attrs.0.as_mut_ptr(),
                default_set.as_ptr(),
            ))?;
            flags |= libc::POSIX_SPAWN_SETSIGDEF;

            cvt_nz(libc::posix_spawnattr_setflags(attrs.0.as_mut_ptr(), flags as _))?;

            let _env_lock = sys::env::env_read_lock();
            let envp = envp
                .map(|c| c.as_ptr())
                .unwrap_or_else(|| libc::__wasilibc_get_environ() as *const _);

            let mut pid = 0;
            cvt_nz(libc::posix_spawnp(
                &mut pid,
                self.get_program_cstr().as_ptr(),
                file_actions.0.as_ptr(),
                attrs.0.as_ptr(),
                self.get_argv().as_ptr() as *const _,
                envp as *const _,
            ))?;

            Ok(unsafe { Process::new(pid, -1) })
        }
    }

    pub fn exec(&mut self, default: Stdio) -> io::Error {
        let envp = self.capture_env();

        if self.saw_nul() {
            return io::const_error!(ErrorKind::InvalidInput, "nul byte found in provided data",);
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

        if let Some(ref cwd) = self.get_cwd() {
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
            Err(io::const_error!(
                ErrorKind::InvalidInput,
                "invalid argument: can't kill an exited process",
            ))
        } else {
            unsafe { wasi::proc_signal(self.pid as u32, wasi::SIGNAL_KILL) }
                .map_err(|_| io::const_error!(ErrorKind::Other, "Kill failed",))
        }
    }

    pub fn wait(&mut self) -> io::Result<ExitStatus> {
        if let Some(status) = self.status {
            return Ok(status);
        }

        let mut pid = wasi::OptionPid { tag: 1, u: wasi::OptionPidU { some: self.pid as u32 } };

        let join_status = unsafe { wasi::proc_join(&mut pid, 0) }
            .map_err(|_| io::const_error!(ErrorKind::Other, "Join failed",))?;

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
            .map_err(|_| io::const_error!(ErrorKind::Other, "Join failed",))?;

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

impl Default for ExitStatus {
    fn default() -> Self {
        Self(JoinStatus {
            tag: wasi::JOIN_STATUS_TYPE_NOTHING.raw(),
            u: wasi::JoinStatusU { nothing: 0 },
        })
    }
}

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
        if self.code() == Some(0) { Ok(()) } else { Err(ExitStatusError(self.0)) }
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
