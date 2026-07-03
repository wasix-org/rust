//! System bindings for the wasm/web platform on WASIX
//!
//! This module mirrors `../wasi/mod.rs` (the WASIp1/p2 PAL), with WASIX
//! specific implementations of `os` and `pipe`.

#![allow(unused)]
#![allow(unused_imports)]

use crate::{io as std_io, mem};

#[path = "../wasi/conf.rs"]
pub mod conf;
#[allow(unused)]
#[path = "../wasm/atomics/futex.rs"]
pub mod futex;
pub mod os;
pub mod pipe;
#[path = "../wasi/stack_overflow.rs"]
pub mod stack_overflow;
#[path = "../unix/time.rs"]
pub mod time;

#[path = "../unsupported/common.rs"]
#[deny(unsafe_op_in_unsafe_fn)]
#[allow(unused)]
mod common;
pub use common::*;

pub fn abort_internal() -> ! {
    unsafe { libc::abort() }
}

pub use os::{IsMinusOne, cvt, cvt_nz, cvt_r};

pub(crate) fn err2io<I>(err: I) -> std_io::Error
where
    I: Into<wasi::Errno>,
{
    std_io::Error::from_raw_os_error(err.into().raw().into())
}
