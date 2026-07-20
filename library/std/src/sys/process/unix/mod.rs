#[cfg_attr(any(target_os = "espidf", target_os = "horizon", target_os = "nuttx"), allow(unused))]
mod common;

cfg_select! {
    target_os = "fuchsia" => {
        mod fuchsia;
        use fuchsia as imp;
    }
    target_os = "vxworks" => {
        mod vxworks;
        use vxworks as imp;
    }
    any(target_os = "espidf", target_os = "horizon", target_os = "vita", target_os = "nuttx") => {
        mod unsupported;
        use unsupported as imp;
        pub use unsupported::output;
    }
    all(target_os = "wasi", target_vendor = "wasmer") => {
        mod wasix;
        use wasix as imp;
    }
    _ => {
        mod unix;
        use unix as imp;
    }
}

pub use imp::{ExitStatus, ExitStatusError, Process};

#[cfg_attr(all(target_os = "wasi", target_vendor = "wasmer"), allow(unused_imports))]
pub use self::common::getppid;
pub use self::common::{ChildPipe, Command, CommandArgs, ExitCode, Stdio, getpid, read_output};
pub use crate::ffi::OsString as EnvKey;
