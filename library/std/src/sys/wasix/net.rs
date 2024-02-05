#![deny(unsafe_op_in_unsafe_fn)]
#![allow(dead_code)]

use super::err2io;
use super::fd::WasiFd;
use crate::fmt;
use crate::collections::VecDeque;
use crate::io::{self, IoSlice, IoSliceMut};
use crate::net::{IpAddr, Ipv4Addr, Ipv6Addr, Shutdown, SocketAddr};
use crate::os::wasi::io::{AsFd, AsRawFd, BorrowedFd, FromRawFd, IntoRawFd, RawFd};

use crate::sys_common::{AsInner, FromInner, IntoInner};
use crate::sync::{Arc, Mutex};
use crate::time::Instant;
use crate::time::Duration;
use libc::c_int;

pub use crate::sys::cvt;

#[stable(feature = "rust1", since = "1.0.0")]
pub struct Socket {
    fd: Option<WasiFd>,
    addr: SocketAddr,
    peer: Arc<Mutex<Option<SocketAddr>>>,
}

impl Socket
{
    #[stable(feature = "rust1", since = "1.0.0")]
    pub fn new(addr: &SocketAddr, ty: c_int) -> io::Result<Socket> {
        let fam = match *addr {
            SocketAddr::V4(..) => AF_INET,
            SocketAddr::V6(..) => AF_INET6,
        };
        let mut sock = Self::new_raw(fam, ty)?;
        sock.addr = addr.clone();
        unsafe {
            let addr = to_wasi_addr_port(addr.clone());
            wasi::sock_bind(sock.fd(), &addr).map_err(err2io)?;
        }
        Ok(sock)
    }

    #[stable(feature = "rust1", since = "1.0.0")]
    pub fn new_raw(fam: c_int, ty: c_int) -> io::Result<Socket> {
        unsafe {
            let wasi_fam = match fam {
                AF_INET6 => wasi::ADDRESS_FAMILY_INET6,
                AF_INET => wasi::ADDRESS_FAMILY_INET4,
                _ => { return Err(io::const_io_error!(io::ErrorKind::Uncategorized, "invalid address family")); }
            };
            let wasi_ty = match ty {
                SOCK_DGRAM => wasi::SOCK_TYPE_SOCKET_DGRAM,
                SOCK_STREAM => wasi::SOCK_TYPE_SOCKET_STREAM,
                SOCK_RAW => wasi::SOCK_TYPE_SOCKET_RAW,
                _ => { return Err(io::const_io_error!(io::ErrorKind::Uncategorized, "invalid socket type")); }
            };
            let wasi_proto = match ty {
                SOCK_DGRAM => wasi::SOCK_PROTO_UDP,
                SOCK_STREAM => wasi::SOCK_PROTO_TCP,
                _ => { return Err(io::const_io_error!(io::ErrorKind::Uncategorized, "invalid socket protocol")); }
            };
            let ip = match fam {
                AF_INET6 => IpAddr::V6(Ipv6Addr::UNSPECIFIED),
                AF_INET => IpAddr::V4(Ipv4Addr::UNSPECIFIED),
                _ => { return Err(io::const_io_error!(io::ErrorKind::Uncategorized, "invalid address family")); }
            };
            let fd = wasi::sock_open(wasi_fam, wasi_ty, wasi_proto).map_err(err2io)?;
            Ok(Socket {
                fd: Some(WasiFd::from_raw_fd(fd as RawFd)),
                addr: SocketAddr::new(ip, 0),
                peer: Arc::new(Mutex::new(None)),
            })
        }
    }

    #[stable(feature = "rust1", since = "1.0.0")]
    pub fn new_pair(fam: c_int, _ty: c_int) -> io::Result<(Socket, Socket)> {
        let ip = match fam {
            AF_INET6 => IpAddr::V6(Ipv6Addr::UNSPECIFIED),
            AF_INET => IpAddr::V4(Ipv4Addr::UNSPECIFIED),
            _ => { return Err(io::const_io_error!(io::ErrorKind::Uncategorized, "invalid address family")); }
        };
        let (fd1, fd2) = unsafe {
            wasi::fd_pipe().map_err(err2io)?
        };
        let socket1 = Socket {
            fd: Some(unsafe { WasiFd::from_raw_fd(fd1 as RawFd) }),
            addr: SocketAddr::new(ip, 0),
            peer: Arc::new(Mutex::new(None)),
        };
        let socket2 = Socket {
            fd: Some(unsafe { WasiFd::from_raw_fd(fd2 as RawFd) }),
            addr: SocketAddr::new(ip, 0),
            peer: Arc::new(Mutex::new(None)),
        };
        Ok((socket1, socket2))
    }

    #[stable(feature = "rust1", since = "1.0.0")]
    pub fn connect(&self, addr: &SocketAddr) -> io::Result<()> {
        let timeout = self.timeout_internal(wasi::SOCK_OPTION_CONNECT_TIMEOUT)?
            .unwrap_or_else(|| Duration::from_secs(20));
        self.connect_timeout(addr, timeout)
    }

    #[stable(feature = "rust1", since = "1.0.0")]
    pub fn connect_timeout(&self, addr: &SocketAddr, timeout: Duration) -> io::Result<()> {
        let r = unsafe {
            let addr = to_wasi_addr_port(*addr);
            wasi::sock_connect(self.fd(), &addr).map_err(err2io)
        };

        match r {
            Ok(_) => return Ok(()),
            Err(ref e) if e.raw_os_error() == Some(wasi::ERRNO_INPROGRESS.raw() as i32) => {}
            Err(e) => return Err(e),
        }

        {
            let mut peer = self.peer.lock().unwrap();
            peer.replace(addr.clone());
        }

        let mut pollfd = libc::pollfd { fd: self.as_raw_fd(), events: libc::POLLOUT, revents: 0 };

        if timeout.as_secs() == 0 && timeout.subsec_nanos() == 0 {
            return Err(io::const_io_error!(
                io::ErrorKind::InvalidInput,
                "cannot set a 0 duration timeout",
            ));
        }

        let start = Instant::now();

        loop {
            let elapsed = start.elapsed();
            if elapsed >= timeout {
                return Err(io::const_io_error!(io::ErrorKind::TimedOut, "connection timed out"));
            }

            let timeout = timeout - elapsed;
            let mut timeout = timeout
                .as_secs()
                .saturating_mul(1_000)
                .saturating_add(timeout.subsec_nanos() as u64 / 1_000_000);
            if timeout == 0 {
                timeout = 1;
            }

            let timeout = timeout.min(libc::c_int::MAX as u64) as c_int;

            match unsafe { libc::poll(&mut pollfd, 1, timeout) } {
                -1 => {
                    let err = io::Error::last_os_error();
                    if err.kind() != io::ErrorKind::Interrupted {
                        return Err(err);
                    }
                }
                0 => {}
                _ => {
                    // linux returns POLLOUT|POLLERR|POLLHUP for refused connections (!), so look
                    // for POLLHUP rather than read readiness
                    if pollfd.revents & libc::POLLHUP != 0 {
                        let e = self.take_error()?.unwrap_or_else(|| {
                            io::const_io_error!(
                                io::ErrorKind::Uncategorized,
                                "no error set after POLLHUP",
                            )
                        });
                        return Err(e);
                    }

                    return Ok(());
                }
            }
        }
    }

    #[stable(feature = "rust1", since = "1.0.0")]
    pub fn accept(&self) -> io::Result<Socket> {
        let (fd, addr) = unsafe {
            wasi::sock_accept_v2(self.fd(), 0).map_err(err2io)?
        };
        let addr = conv_addr_port(addr);
        Ok(Socket {
            fd: Some(unsafe { WasiFd::from_raw_fd(fd as RawFd) }),
            addr,
            peer: Arc::new(Mutex::new(Some(addr))),
        })
    }
    
    #[stable(feature = "rust1", since = "1.0.0")]
    pub fn accept_timeout(&self, timeout: Duration) -> io::Result<Socket> {
        self.set_nonblocking(true)?;
        loop {
            let mut pollfd = libc::pollfd { fd: self.as_raw_fd(), events: libc::POLLIN, revents: 0 };
            let start = Instant::now();
            loop {
                let elapsed = start.elapsed();
                if elapsed >= timeout {
                    return Err(io::const_io_error!(io::ErrorKind::TimedOut, "connection timed out"));
                }

                let timeout = timeout - elapsed;
                let mut timeout = timeout
                    .as_secs()
                    .saturating_mul(1_000)
                    .saturating_add(timeout.subsec_nanos() as u64 / 1_000_000);
                if timeout == 0 {
                    timeout = 1;
                }

                let timeout = timeout.min(libc::c_int::MAX as u64) as c_int;

                match unsafe { libc::poll(&mut pollfd, 1, timeout) } {
                    -1 => {
                        let err = io::Error::last_os_error();
                        if err.kind() != io::ErrorKind::Interrupted {
                            return Err(err);
                        }
                    }
                    0 => {}
                    _ => {
                        break;
                    }
                }
            }
            
            unsafe {
                match wasi::sock_accept_v2(self.fd(), 0) {
                    Ok((fd, addr)) => {
                        let addr = conv_addr_port(addr);
                        return Ok(Socket {
                            fd: Some(WasiFd::from_raw_fd(fd as RawFd)),
                            addr,
                            peer: Arc::new(Mutex::new(Some(addr))),
                        });
                    }
                    Err(wasi::x::ERRNO_AGAIN) => {
                        crate::thread::yield_now();
                        continue;
                    }
                    Err(err) => {
                        return Err(err2io(err))
                    }
                }
            }
        }
    }

    #[stable(feature = "rust1", since = "1.0.0")]
    pub fn duplicate(&self) -> io::Result<Socket> {
        let peer = self.peer.lock().unwrap();
        let fd = unsafe {
            wasi::fd_dup(self.fd()).map_err(err2io)?
        };
        Ok(
            Socket {
                fd: Some(unsafe { WasiFd::from_raw_fd(fd as RawFd) }),
                addr: self.addr.clone(),
                peer: Arc::new(Mutex::new(peer.clone())),
            }
        )
    }

    fn recv_with_flags(
        &self,
        ri_data: &mut [IoSliceMut<'_>],
        ri_flags: wasi::Riflags,
    ) -> io::Result<(usize, wasi::Roflags)> {
        let (amt, flags) = unsafe {
            wasi::sock_recv(self.fd(), super::fd::iovec(ri_data), ri_flags).map_err(err2io)?
        };
        Ok((amt as usize, flags))
    }

    #[stable(feature = "rust1", since = "1.0.0")]
    pub fn recv(&self, buf: &mut [u8]) -> io::Result<usize> {
        let mut data = [ IoSliceMut::new(buf) ];
        let ret = self.recv_with_flags(&mut data, 0)?;
        Ok(ret.0)
    }

    #[stable(feature = "rust1", since = "1.0.0")]
    pub fn read(&self, buf: &mut [u8]) -> io::Result<usize> {
        self.recv(buf)
    }

    #[stable(feature = "rust1", since = "1.0.0")]
    pub fn peek(&self, buf: &mut [u8]) -> io::Result<usize> {
        let mut data = [ IoSliceMut::new(buf) ];
        let ret = self.recv_with_flags(&mut data, MSG_PEEK as u16)?;
        Ok(ret.0)
    }

    #[stable(feature = "rust1", since = "1.0.0")]
    pub fn read_buf(&self, mut buf: io::BorrowedCursor<'_>) -> io::Result<()> {
        let mut data = [ IoSliceMut::new(buf.init_mut()) ];
        let ret = self.recv_with_flags(&mut data, 0)?;
        unsafe { buf.advance(ret.0); }
        Ok(())
    }

    #[stable(feature = "rust1", since = "1.0.0")]
    pub fn read_vectored(&self, bufs: &mut [IoSliceMut<'_>]) -> io::Result<usize> {
        self.recv_vectored(bufs)
    }

    #[stable(feature = "rust1", since = "1.0.0")]
    #[inline]
    pub fn is_read_vectored(&self) -> bool {
        self.is_recv_vectored()
    }

    #[stable(feature = "rust1", since = "1.0.0")]
    pub fn recv_vectored(&self, bufs: &mut [IoSliceMut<'_>]) -> io::Result<usize> {
        let ret = self.recv_with_flags(bufs, 0)?;
        Ok(ret.0)
    }

    #[stable(feature = "rust1", since = "1.0.0")]
    #[inline]
    pub fn is_recv_vectored(&self) -> bool {
        true
    }

    fn recv_from_with_flags(
        &self,
        ri_data: &mut [IoSliceMut<'_>],
        ri_flags: wasi::Riflags,
    ) -> io::Result<(usize, wasi::Roflags, SocketAddr)> {
        let ret = unsafe {
            wasi::sock_recv_from(self.fd(), super::fd::iovec(ri_data), ri_flags).map_err(err2io)?
        };
        Ok((
            ret.0 as usize,
            ret.1,
            conv_addr_port(ret.2)
        ))
    }

    #[stable(feature = "rust1", since = "1.0.0")]
    pub fn recv_from(&self, buf: &mut [u8]) -> io::Result<(usize, SocketAddr)> {
        let mut data = [ IoSliceMut::new(buf) ];
        let ret = self.recv_from_with_flags(&mut data, 0)?;
        Ok((ret.0, ret.2))
    }

    #[stable(feature = "rust1", since = "1.0.0")]
    pub fn recv_msg(&self, msg: &mut libc::msghdr) -> io::Result<usize> {
        let n = cvt(unsafe { libc::recvmsg(self.as_raw_fd(), msg, libc::MSG_CMSG_CLOEXEC) })?;
        Ok(n as usize)
    }

    #[stable(feature = "rust1", since = "1.0.0")]
    pub fn peek_from(&self, buf: &mut [u8]) -> io::Result<(usize, SocketAddr)> {
        let mut data = [ IoSliceMut::new(buf) ];
        let ret = self.recv_from_with_flags(&mut data, MSG_PEEK as u16)?;
        Ok((ret.0, ret.2))
    }

    fn send_with_flags(
        &self,
        si_data: &[IoSlice<'_>],
        si_flags: wasi::Siflags
    ) -> io::Result<usize> {
        unsafe {
            wasi::sock_send(self.fd(), super::fd::ciovec(si_data), si_flags).map(|a| a as usize).map_err(err2io)
        }
    }

    #[stable(feature = "rust1", since = "1.0.0")]
    pub fn send(&self, buf: &[u8]) -> io::Result<usize> {
        let data = [ IoSlice::new(buf) ];
        self.send_with_flags(&data, 0)
    }

    #[stable(feature = "rust1", since = "1.0.0")]
    pub fn send_vectored(&self, bufs: &[IoSlice<'_>]) -> io::Result<usize> {
        self.send_with_flags(bufs, 0)
    }

    #[stable(feature = "rust1", since = "1.0.0")]
    #[inline]
    pub fn is_send_vectored(&self) -> bool {
        true
    }

    #[stable(feature = "rust1", since = "1.0.0")]
    pub fn write(&self, buf: &[u8]) -> io::Result<usize> {
        self.send(buf)
    }

    #[stable(feature = "rust1", since = "1.0.0")]
    pub fn write_vectored(&self, bufs: &[IoSlice<'_>]) -> io::Result<usize> {
        self.send_vectored(bufs)
    }

    #[stable(feature = "rust1", since = "1.0.0")]
    #[inline]
    pub fn is_write_vectored(&self) -> bool {
        self.is_send_vectored()
    }

    fn send_to_with_flags(
        &self,
        si_data: &[IoSlice<'_>],
        si_flags: wasi::Siflags,
        addr: SocketAddr
    ) -> io::Result<usize> {
        let addr = to_wasi_addr_port(addr);
        unsafe {
            wasi::sock_send_to(self.fd(), super::fd::ciovec(si_data), si_flags, &addr).map(|a| a as usize).map_err(err2io)
        }
    }

    #[stable(feature = "rust1", since = "1.0.0")]
    pub fn send_to(&self, buf: &[u8], addr: SocketAddr) -> io::Result<usize> {
        let data = [ IoSlice::new(buf) ];
        self.send_to_with_flags(&data, 0, addr)
    }

    #[stable(feature = "rust1", since = "1.0.0")]
    pub fn send_to_vectored(&self, bufs: &[IoSlice<'_>], addr: SocketAddr) -> io::Result<usize> {
        self.send_to_with_flags(bufs, 0, addr)
    }

    #[stable(feature = "rust1", since = "1.0.0")]
    pub fn send_msg(&self, msg: &mut libc::msghdr) -> io::Result<usize> {
        let n = cvt(unsafe { libc::sendmsg(self.as_raw_fd(), msg, 0) })?;
        Ok(n as usize)
    }

    #[stable(feature = "rust1", since = "1.0.0")]
    pub fn set_timeout(&self, dur: Option<Duration>, kind: libc::c_int) -> io::Result<()> {
        let option = match kind {
            SO_RCVTIMEO => wasi::SOCK_OPTION_RECV_TIMEOUT,
            SO_SNDTIMEO => wasi::SOCK_OPTION_SEND_TIMEOUT,
            SO_CONNTIMEO => wasi::SOCK_OPTION_CONNECT_TIMEOUT,
            SO_ACCPTIMEO => wasi::SOCK_OPTION_ACCEPT_TIMEOUT,
            _ => { return Err(io::const_io_error!(io::ErrorKind::Uncategorized, "invalid timeout type")); }
        };
        self.set_timeout_internal(dur, option)
    }

    #[stable(feature = "rust1", since = "1.0.0")]
    pub fn timeout(&self, kind: libc::c_int) -> io::Result<Option<Duration>> {
        let option = match kind {
            SO_RCVTIMEO => wasi::SOCK_OPTION_RECV_TIMEOUT,
            SO_SNDTIMEO => wasi::SOCK_OPTION_SEND_TIMEOUT,
            SO_CONNTIMEO => wasi::SOCK_OPTION_CONNECT_TIMEOUT,
            SO_ACCPTIMEO => wasi::SOCK_OPTION_ACCEPT_TIMEOUT,
            _ => { return Err(io::const_io_error!(io::ErrorKind::Uncategorized, "invalid timeout type")); }
        };
        self.timeout_internal(option)
    }

    fn set_timeout_internal(&self, dur: Option<Duration>, option: wasi::SockOption) -> io::Result<()> {
        let dur = match dur {
            Some(dur) => wasi::OptionTimestamp {
                tag: wasi::OPTION_SOME.raw(),
                u: wasi::OptionTimestampU {
                    some: dur.as_nanos() as u64,
                }
            },
            None => wasi::OptionTimestamp {
                tag: wasi::OPTION_NONE.raw(),
                u: wasi::OptionTimestampU {
                    none: 0,
                }
            }
        };
        unsafe {
            wasi::sock_set_opt_time(self.fd(), option, &dur).map_err(err2io)
        }
    }

    fn timeout_internal(&self, option: wasi::SockOption) -> io::Result<Option<Duration>> {
        let ret = unsafe {
            wasi::sock_get_opt_time(self.fd(), option).map_err(err2io)?
        };
        Ok(
            match ret.tag {
                a if a == wasi::OPTION_SOME.raw() => {
                    unsafe {
                        Some(Duration::from_nanos(ret.u.some as u64))
                    }
                },
                a if a == wasi::OPTION_NONE.raw() => {
                    None
                },
                _ => { return Err(io::const_io_error!(io::ErrorKind::Uncategorized, "invalid response")); }
            }
        )
    }

    #[stable(feature = "rust1", since = "1.0.0")]
    pub fn peer_addr(&self) -> io::Result<SocketAddr> {
        use crate::ops::Deref;
        let peer = self.peer.lock().unwrap();
        if let Some(peer) = peer.deref() {
            Ok(peer.clone())
        } else {
            Ok(
                match self.addr {
                    SocketAddr::V4(..) => SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 0),
                    SocketAddr::V6(..) => SocketAddr::new(IpAddr::V6(Ipv6Addr::UNSPECIFIED), 0),
                }
            )
        }
    }

    #[stable(feature = "rust1", since = "1.0.0")]
    pub fn socket_addr(&self) -> io::Result<SocketAddr> {
        Ok(self.addr.clone())
    }

    #[stable(feature = "rust1", since = "1.0.0")]
    pub fn shutdown(&self, how: Shutdown) -> io::Result<()> {
        let how = match how {
            Shutdown::Read => wasi::SDFLAGS_RD,
            Shutdown::Write => wasi::SDFLAGS_WR,
            Shutdown::Both => wasi::SDFLAGS_WR | wasi::SDFLAGS_RD,
        };
        unsafe { wasi::sock_shutdown(self.fd(), how).map_err(err2io) }
    }

    #[stable(feature = "rust1", since = "1.0.0")]
    pub fn set_reuse_addr(&self, reuse: bool) -> io::Result<()> {
        self.set_opt_flag(wasi::SOCK_OPTION_REUSE_ADDR, reuse)
    }

    #[stable(feature = "rust1", since = "1.0.0")]
    pub fn reuse_addr(&self) -> io::Result<bool> {
        self.get_opt_flag(wasi::SOCK_OPTION_REUSE_ADDR)
    }

    #[stable(feature = "rust1", since = "1.0.0")]
    pub fn set_reuse_port(&self, reuse: bool) -> io::Result<()> {
        self.set_opt_flag(wasi::SOCK_OPTION_REUSE_PORT, reuse)
    }

    #[stable(feature = "rust1", since = "1.0.0")]
    pub fn reuse_port(&self) -> io::Result<bool> {
        self.get_opt_flag(wasi::SOCK_OPTION_REUSE_PORT)
    }

    #[stable(feature = "rust1", since = "1.0.0")]
    pub fn set_nodelay(&self, nodelay: bool) -> io::Result<()> {
        self.set_opt_flag(wasi::SOCK_OPTION_NO_DELAY, nodelay)
    }

    #[stable(feature = "rust1", since = "1.0.0")]
    pub fn nodelay(&self) -> io::Result<bool> {
        self.get_opt_flag(wasi::SOCK_OPTION_NO_DELAY)
    }

    #[stable(feature = "rust1", since = "1.0.0")]
    pub fn set_only_v6(&self, only_v6: bool) -> io::Result<()> {
        self.set_opt_flag(wasi::SOCK_OPTION_ONLY_V6, only_v6)
    }

    #[stable(feature = "rust1", since = "1.0.0")]
    pub fn only_v6(&self) -> io::Result<bool> {
        self.get_opt_flag(wasi::SOCK_OPTION_ONLY_V6)
    }

    #[stable(feature = "rust1", since = "1.0.0")]
    pub fn set_broadcast(&self, broadcast: bool) -> io::Result<()> {
        self.set_opt_flag(wasi::SOCK_OPTION_BROADCAST, broadcast)
    }

    #[stable(feature = "rust1", since = "1.0.0")]
    pub fn broadcast(&self) -> io::Result<bool> {
        self.get_opt_flag(wasi::SOCK_OPTION_BROADCAST)
    }

    #[stable(feature = "rust1", since = "1.0.0")]
    pub fn set_multicast_loop_v4(&self, val: bool) -> io::Result<()> {
        self.set_opt_flag(wasi::SOCK_OPTION_MULTICAST_LOOP_V4, val)
    }

    #[stable(feature = "rust1", since = "1.0.0")]
    pub fn multicast_loop_v4(&self) -> io::Result<bool> {
        self.get_opt_flag(wasi::SOCK_OPTION_MULTICAST_LOOP_V4)
    }

    #[stable(feature = "rust1", since = "1.0.0")]
    pub fn set_multicast_loop_v6(&self, val: bool) -> io::Result<()> {
        self.set_opt_flag(wasi::SOCK_OPTION_MULTICAST_LOOP_V6, val)
    }

    #[stable(feature = "rust1", since = "1.0.0")]
    pub fn multicast_loop_v6(&self) -> io::Result<bool> {
        self.get_opt_flag(wasi::SOCK_OPTION_MULTICAST_LOOP_V6)
    }

    fn set_opt_flag(&self, sockopt: wasi::SockOption, val: bool) -> io::Result<()> {
        unsafe {
            let val = match val {
                false => wasi::BOOL_FALSE,
                true => wasi::BOOL_TRUE
            };
            wasi::sock_set_opt_flag(self.fd(), sockopt, val).map_err(err2io)
        }
    }

    fn get_opt_flag(&self, sockopt: wasi::SockOption) -> io::Result<bool> {
        let val = unsafe {
            wasi::sock_get_opt_flag(self.fd(), sockopt).map_err(err2io)?
        };
        Ok(
            match val {
                wasi::BOOL_TRUE => true,
                wasi::BOOL_FALSE | _ => false,
            }
        )
    }

    #[stable(feature = "rust1", since = "1.0.0")]
    pub fn set_linger(&self, val: Option<Duration>) -> io::Result<()> {
        let val = match val {
            Some(dur) => wasi::OptionTimestamp {
                tag: wasi::OPTION_SOME.raw(),
                u: wasi::OptionTimestampU {
                    some: dur.as_nanos() as u64,
                }
            },
            None => wasi::OptionTimestamp {
                tag: wasi::OPTION_NONE.raw(),
                u: wasi::OptionTimestampU {
                    none: 0,
                }
            }
        };
        unsafe {
            wasi::sock_set_opt_time(self.fd(), wasi::SOCK_OPTION_LINGER, &val).map_err(err2io)
        }
    }

    #[stable(feature = "rust1", since = "1.0.0")]
    pub fn linger(&self) -> io::Result<Option<Duration>> {
        let ret = unsafe {
            wasi::sock_get_opt_time(self.fd(), wasi::SOCK_OPTION_LINGER).map_err(err2io)?
        };
        Ok(
            match ret.tag {
                a if a == wasi::OPTION_SOME.raw() => {
                    unsafe {
                        Some(Duration::from_nanos(ret.u.some as u64))
                    }
                },
                a if a == wasi::OPTION_NONE.raw() => {
                    None
                },
                _ => { return Err(io::const_io_error!(io::ErrorKind::Uncategorized, "invalid response")); }
            }
        )
    }

    #[stable(feature = "rust1", since = "1.0.0")]
    pub fn set_nonblocking(&self, nonblocking: bool) -> io::Result<()> {
        let fdstat = unsafe {
            wasi::fd_fdstat_get(self.fd()).map_err(err2io)?
        };

        let mut flags = fdstat.fs_flags;

        if nonblocking {
            flags |= wasi::FDFLAGS_NONBLOCK;
        } else {
            flags &= !wasi::FDFLAGS_NONBLOCK;
        }

        unsafe {
            wasi::fd_fdstat_set_flags(self.fd(), flags)
                .map_err(err2io)?;
        }

        Ok(())
    }

    #[stable(feature = "rust1", since = "1.0.0")]
    pub fn set_ttl(&self, ttl: u32) -> io::Result<()> {
        unsafe {
            wasi::sock_set_opt_size(self.fd(), wasi::SOCK_OPTION_TTL, ttl as wasi::Filesize).map_err(err2io)
        }
    }

    #[stable(feature = "rust1", since = "1.0.0")]
    pub fn ttl(&self) -> io::Result<u32> {
        let ttl = unsafe {
            wasi::sock_get_opt_size(self.fd(), wasi::SOCK_OPTION_TTL).map_err(err2io)? as u32
        };
        Ok(ttl)
    }

    #[stable(feature = "rust1", since = "1.0.0")]
    pub fn set_multicast_ttl_v4(&self, ttl: u32) -> io::Result<()> {
        unsafe {
            wasi::sock_set_opt_size(self.fd(), wasi::SOCK_OPTION_MULTICAST_TTL_V4, ttl as wasi::Filesize).map_err(err2io)
        }
    }

    #[stable(feature = "rust1", since = "1.0.0")]
    pub fn multicast_ttl_v4(&self) -> io::Result<u32> {
        let ttl = unsafe {
            wasi::sock_get_opt_size(self.fd(), wasi::SOCK_OPTION_MULTICAST_TTL_V4).map_err(err2io)? as u32
        };
        Ok(ttl)
    }

    #[stable(feature = "rust1", since = "1.0.0")]
    pub fn join_multicast_v4(&self, multiaddr: &Ipv4Addr, interface: &Ipv4Addr) -> io::Result<()> {
        let multiaddr = to_wasi_addr_v4(*multiaddr);
        let interface = to_wasi_addr_v4(*interface);
        unsafe {
            wasi::sock_join_multicast_v4(self.fd(), &multiaddr, &interface).map_err(err2io)
        }
    }

    #[stable(feature = "rust1", since = "1.0.0")]
    pub fn join_multicast_v6(&self, multiaddr: &Ipv6Addr, interface: u32) -> io::Result<()> {
        let multiaddr = to_wasi_addr_v6(*multiaddr);
        unsafe {
            wasi::sock_join_multicast_v6(self.fd(), &multiaddr, interface).map_err(err2io)
        }
    }

    #[stable(feature = "rust1", since = "1.0.0")]
    pub fn leave_multicast_v4(&self, multiaddr: &Ipv4Addr, interface: &Ipv4Addr) -> io::Result<()> {
        let multiaddr = to_wasi_addr_v4(*multiaddr);
        let interface = to_wasi_addr_v4(*interface);
        unsafe {
            wasi::sock_leave_multicast_v4(self.fd(), &multiaddr, &interface).map_err(err2io)
        }
    }

    #[stable(feature = "rust1", since = "1.0.0")]
    pub fn leave_multicast_v6(&self, multiaddr: &Ipv6Addr, interface: u32) -> io::Result<()> {
        let multiaddr = to_wasi_addr_v6(*multiaddr);
        unsafe {
            wasi::sock_leave_multicast_v6(self.fd(), &multiaddr, interface).map_err(err2io)
        }
    }

    #[stable(feature = "rust1", since = "1.0.0")]
    pub fn take_error(&self) -> io::Result<Option<io::Error>> {
        Ok(None)
    }

    #[stable(feature = "rust1", since = "1.0.0")]
    pub fn as_raw(&self) -> RawFd {
        self.as_raw_fd()
    }

    fn fd(&self) -> wasi::Fd {
        self.as_raw_fd() as wasi::Fd
    }
}

#[stable(feature = "rust1", since = "1.0.0")]
impl Drop
for Socket
{
    fn drop(&mut self) {
        unsafe {
            if let Some(fd) = self.fd.take() {
                let _ = wasi::fd_close(fd.as_raw_fd() as wasi::Fd);
            }
        }
    }
}

fn conv_addr_port(addr: wasi::AddrPort) -> SocketAddr {
    unsafe {
        match addr.tag {
            a if a == wasi::ADDRESS_FAMILY_INET6.raw() => {
                let u = &addr.u.inet6.addr;
                let ip = IpAddr::V6(Ipv6Addr::new(u.n0, u.n1, u.n2, u.n3, u.h0, u.h1, u.h2, u.h3));
                SocketAddr::new(ip, addr.u.inet6.port)
            }
            _ => {
                let u = &addr.u.inet4.addr;
                let ip = IpAddr::V4(Ipv4Addr::new(u.n0, u.n1, u.h0, u.h1));
                SocketAddr::new(ip, addr.u.inet4.port)
            },
        }
    }
}

fn conv_addr_v4(u: wasi::AddrIp4) -> Ipv4Addr {
    Ipv4Addr::new(u.n0, u.n1, u.h0, u.h1)
}

fn conv_addr_v6(u: wasi::AddrIp6) -> Ipv6Addr {
    Ipv6Addr::new(u.n0, u.n1, u.n2, u.n3, u.h0, u.h1, u.h2, u.h3)
}

fn conv_addr(addr: wasi::Addr) -> IpAddr {
    unsafe {
        match addr.tag {
            a if a == wasi::ADDRESS_FAMILY_INET6.raw() => {
                IpAddr::V6(conv_addr_v6(addr.u.inet6))
            },
            _ => {
                IpAddr::V4(conv_addr_v4(addr.u.inet4))
            }
        }
    }
}

fn to_wasi_addr_port(addr: SocketAddr) -> wasi::AddrPort {
    let ip = to_wasi_addr(addr.ip());
    unsafe {
        wasi::AddrPort {
            tag: ip.tag,
            u: match ip.tag {
                a if a == wasi::ADDRESS_FAMILY_INET6.raw() => {
                    wasi::AddrPortU {
                        inet6: wasi::AddrIp6Port {
                            addr: ip.u.inet6,
                            port: addr.port()
                        }
                    }
                },
                _ => {
                    wasi::AddrPortU {
                        inet4: wasi::AddrIp4Port {
                            addr: ip.u.inet4,
                            port: addr.port()
                        }
                    }
                }            
            }
        }
    }
}

fn to_wasi_addr_v4(ip: Ipv4Addr) -> wasi::AddrIp4 {
    let octs = ip.octets();
    wasi::AddrIp4 {
        n0: octs[0],
        n1: octs[1],
        h0: octs[2],
        h1: octs[3],
    }
}

fn to_wasi_addr_v6(ip: Ipv6Addr) -> wasi::AddrIp6 {
    let segs = ip.segments();
    wasi::AddrIp6 {
        n0: segs[0],
        n1: segs[1],
        n2: segs[2],
        n3: segs[3],
        h0: segs[4],
        h1: segs[5],
        h2: segs[6],
        h3: segs[7]
    }
}

fn to_wasi_addr(addr: IpAddr) -> wasi::Addr {
    match addr {
        IpAddr::V4(ip) => {
            wasi::Addr {
                tag: wasi::ADDRESS_FAMILY_INET4.raw(),
                u: wasi::AddrU {
                    inet4: to_wasi_addr_v4(ip)
                }
            }
        },
        IpAddr::V6(ip) => {
            wasi::Addr {
                tag: wasi::ADDRESS_FAMILY_INET6.raw(),
                u: wasi::AddrU {
                    inet6: to_wasi_addr_v6(ip)
                }
            }
        }
    }
}

#[stable(feature = "rust1", since = "1.0.0")]
impl AsInner<WasiFd> for Socket {
    fn as_inner(&self) -> &WasiFd {
        self.fd.as_ref().unwrap()
    }
}

#[stable(feature = "rust1", since = "1.0.0")]
impl IntoInner<WasiFd> for Socket {
    fn into_inner(mut self) -> WasiFd {
        self.fd.take().unwrap()
    }
}

#[stable(feature = "rust1", since = "1.0.0")]
impl FromInner<WasiFd> for Socket {
    fn from_inner(inner: WasiFd) -> Socket {
        Socket {
            fd: Some(inner),
            addr: SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 0),
            peer: Arc::new(Mutex::new(None)),
        }
    }
}

#[stable(feature = "rust1", since = "1.0.0")]
impl AsFd for Socket {
    fn as_fd(&self) -> BorrowedFd<'_> {
        let fd = self.as_raw_fd();
        unsafe {
            BorrowedFd::borrow_raw(fd)
        }
    }
}

#[stable(feature = "rust1", since = "1.0.0")]
impl AsRawFd for Socket {
    fn as_raw_fd(&self) -> RawFd {
        self.fd.as_ref().map(|fd| fd.as_raw_fd()).unwrap_or_default()
    }
}

#[stable(feature = "rust1", since = "1.0.0")]
impl IntoRawFd for Socket {
    fn into_raw_fd(mut self) -> RawFd {
        self.fd.take().map(|fd| fd.as_raw_fd()).unwrap_or_default()
    }
}

#[stable(feature = "rust1", since = "1.0.0")]
impl FromRawFd for Socket {
    unsafe fn from_raw_fd(raw_fd: RawFd) -> Self {
        Self {
            fd: Some(unsafe { WasiFd::from_raw_fd(raw_fd) }),
            addr: SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 0),
            peer: Arc::new(Mutex::new(None)),
        }
    }
}

#[stable(feature = "rust1", since = "1.0.0")]
pub struct TcpStream {
    inner: Socket,
}

impl TcpStream {
    #[stable(feature = "rust1", since = "1.0.0")]
    pub fn connect(addr: io::Result<&SocketAddr>) -> io::Result<TcpStream> {
        let addr = addr?;
        let fam = match *addr {
            SocketAddr::V4(..) => AF_INET,
            SocketAddr::V6(..) => AF_INET6,
        };
        let sock = Socket::new_raw(fam, SOCK_STREAM)?;
        sock.connect(addr)?;
        Ok(
            TcpStream {
                inner: sock
            }
        )
    }

    #[stable(feature = "rust1", since = "1.0.0")]
    pub fn connect_timeout(addr: &SocketAddr, timeout: Duration) -> io::Result<TcpStream> {
        let fam = match *addr {
            SocketAddr::V4(..) => AF_INET,
            SocketAddr::V6(..) => AF_INET6,
        };
        let sock = Socket::new_raw(fam, SOCK_STREAM)?;
        sock.connect_timeout(addr, timeout)?;
        Ok(
            TcpStream {
                inner: sock
            }
        )
    }

    #[stable(feature = "rust1", since = "1.0.0")]
    pub fn set_read_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        self.inner.set_timeout_internal(timeout, wasi::SOCK_OPTION_RECV_TIMEOUT)
    }

    #[stable(feature = "rust1", since = "1.0.0")]
    pub fn set_write_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        self.inner.set_timeout_internal(timeout, wasi::SOCK_OPTION_SEND_TIMEOUT)
    }

    #[stable(feature = "rust1", since = "1.0.0")]
    pub fn read_timeout(&self) -> io::Result<Option<Duration>> {
        self.inner.timeout_internal(wasi::SOCK_OPTION_RECV_TIMEOUT)
    }

    #[stable(feature = "rust1", since = "1.0.0")]
    pub fn write_timeout(&self) -> io::Result<Option<Duration>> {
        self.inner.timeout_internal(wasi::SOCK_OPTION_SEND_TIMEOUT)
    }

    #[stable(feature = "rust1", since = "1.0.0")]
    pub fn peek(&self, buf: &mut [u8]) -> io::Result<usize> {
        self.inner.peek(buf)
    }

    #[stable(feature = "rust1", since = "1.0.0")]
    pub fn read(&self, buf: &mut [u8]) -> io::Result<usize> {
        self.inner.recv(buf)
    }

    #[stable(feature = "rust1", since = "1.0.0")]
    pub fn read_buf(&self, buf: io::BorrowedCursor<'_>) -> io::Result<()> {
        self.inner.read_buf(buf)
    }

    #[stable(feature = "rust1", since = "1.0.0")]
    pub fn read_vectored(&self, bufs: &mut [IoSliceMut<'_>]) -> io::Result<usize> {
        self.inner.recv_vectored(bufs)
    }

    #[stable(feature = "rust1", since = "1.0.0")]
    pub fn is_read_vectored(&self) -> bool {
        self.inner.is_recv_vectored()
    }

    #[stable(feature = "rust1", since = "1.0.0")]
    pub fn write(&self, buf: &[u8]) -> io::Result<usize> {
        self.inner.send(buf)
    }

    #[stable(feature = "rust1", since = "1.0.0")]
    pub fn write_vectored(&self, bufs: &[IoSlice<'_>]) -> io::Result<usize> {
        self.inner.send_vectored(bufs)
    }

    #[stable(feature = "rust1", since = "1.0.0")]
    pub fn is_write_vectored(&self) -> bool {
        self.inner.is_send_vectored()
    }

    #[stable(feature = "rust1", since = "1.0.0")]
    pub fn peer_addr(&self) -> io::Result<SocketAddr> {
        self.inner.peer_addr()
    }

    #[stable(feature = "rust1", since = "1.0.0")]
    pub fn socket_addr(&self) -> io::Result<SocketAddr> {
        self.inner.socket_addr()
    }

    #[stable(feature = "rust1", since = "1.0.0")]
    pub fn shutdown(&self, shutdown: Shutdown) -> io::Result<()> {
        self.inner.shutdown(shutdown)
    }

    #[stable(feature = "rust1", since = "1.0.0")]
    pub fn duplicate(&self) -> io::Result<TcpStream> {
        Ok(
            TcpStream {
                inner: self.inner.duplicate()?
            }
        )
    }

    #[stable(feature = "rust1", since = "1.0.0")]
    pub fn set_linger(&self, linger: Option<Duration>) -> io::Result<()> {
        self.inner.set_linger(linger)
    }

    #[stable(feature = "rust1", since = "1.0.0")]
    pub fn linger(&self) -> io::Result<Option<Duration>> {
        self.inner.linger()
    }

    #[stable(feature = "rust1", since = "1.0.0")]
    pub fn set_nodelay(&self, nodelay: bool) -> io::Result<()> {
        self.inner.set_nodelay(nodelay)
    }

    #[stable(feature = "rust1", since = "1.0.0")]
    pub fn nodelay(&self) -> io::Result<bool> {
        self.inner.nodelay()
    }

    #[stable(feature = "rust1", since = "1.0.0")]
    pub fn set_ttl(&self, ttl: u32) -> io::Result<()> {
        self.inner.set_ttl(ttl)
    }

    #[stable(feature = "rust1", since = "1.0.0")]
    pub fn ttl(&self) -> io::Result<u32> {
        self.inner.ttl()
    }

    #[stable(feature = "rust1", since = "1.0.0")]
    pub fn take_error(&self) -> io::Result<Option<io::Error>> {
        self.inner.take_error()
    }

    #[stable(feature = "rust1", since = "1.0.0")]
    pub fn set_nonblocking(&self, state: bool) -> io::Result<()> {
        self.inner.set_nonblocking(state)
    }

    #[stable(feature = "rust1", since = "1.0.0")]
    pub fn socket(&self) -> &Socket {
        &self.inner
    }

    #[stable(feature = "rust1", since = "1.0.0")]
    pub fn into_socket(self) -> Socket {
        self.inner
    }
}

#[stable(feature = "rust1", since = "1.0.0")]
impl io::Read for TcpStream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.inner.read(buf)
    }

    fn read_buf(&mut self, buf: io::BorrowedCursor<'_>) -> io::Result<()> {
        self.inner.read_buf(buf)
    }

    fn read_vectored(&mut self, bufs: &mut [io::IoSliceMut<'_>]) -> io::Result<usize> {
        self.inner.read_vectored(bufs)
    }

    #[inline]
    fn is_read_vectored(&self) -> bool {
        self.inner.is_read_vectored()
    }
}
#[stable(feature = "rust1", since = "1.0.0")]
impl io::Write for TcpStream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.inner.write(buf)
    }

    fn write_vectored(&mut self, bufs: &[io::IoSlice<'_>]) -> io::Result<usize> {
        self.inner.write_vectored(bufs)
    }

    #[inline]
    fn is_write_vectored(&self) -> bool {
        self.inner.is_write_vectored()
    }

    #[inline]
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
#[stable(feature = "rust1", since = "1.0.0")]
impl io::Read for &TcpStream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.inner.read(buf)
    }

    fn read_buf(&mut self, buf: io::BorrowedCursor<'_>) -> io::Result<()> {
        self.inner.read_buf(buf)
    }

    fn read_vectored(&mut self, bufs: &mut [io::IoSliceMut<'_>]) -> io::Result<usize> {
        self.inner.read_vectored(bufs)
    }

    #[inline]
    fn is_read_vectored(&self) -> bool {
        self.inner.is_read_vectored()
    }
}
#[stable(feature = "rust1", since = "1.0.0")]
impl io::Write for &TcpStream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.inner.write(buf)
    }

    fn write_vectored(&mut self, bufs: &[io::IoSlice<'_>]) -> io::Result<usize> {
        self.inner.write_vectored(bufs)
    }

    #[inline]
    fn is_write_vectored(&self) -> bool {
        self.inner.is_write_vectored()
    }

    #[inline]
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[stable(feature = "rust1", since = "1.0.0")]
impl FromInner<Socket> for TcpStream {
    fn from_inner(socket: Socket) -> TcpStream {
        TcpStream { inner: socket }
    }
}

#[stable(feature = "rust1", since = "1.0.0")]
impl fmt::Debug for TcpStream {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TcpStream").field("fd", &self.inner.as_raw_fd()).finish()
    }
}

#[stable(feature = "rust1", since = "1.0.0")]
pub struct TcpListener {
    inner: Socket,
}

impl TcpListener {
    #[stable(feature = "rust1", since = "1.0.0")]
    pub fn bind(addr: io::Result<&SocketAddr>) -> io::Result<TcpListener> {
        let addr = addr?;
        let sock = Socket::new(addr, SOCK_STREAM)?;

        sock.set_reuse_addr(true)?;

        unsafe {
            wasi::sock_listen(sock.fd(), 128).map_err(err2io)?;
        }

        Ok(
            TcpListener {
                inner: sock
            }
        )
    }

    #[stable(feature = "rust1", since = "1.0.0")]
    pub fn socket_addr(&self) -> io::Result<SocketAddr> {
        self.inner.socket_addr()
    }

    #[stable(feature = "rust1", since = "1.0.0")]
    pub fn accept(&self) -> io::Result<(TcpStream, SocketAddr)> {
        let socket = self.inner.accept()?;
        let addr = socket.socket_addr()?;
        Ok(
            (
                TcpStream {
                    inner: socket,
                },
                addr
            )
        )
    }

    #[stable(feature = "rust1", since = "1.0.0")]
    pub fn accept_timeout(&self, timeout: crate::time::Duration) -> io::Result<(TcpStream, SocketAddr)> {
        let socket = self.inner.accept_timeout(timeout)?;
        let addr = socket.socket_addr()?;
        Ok(
            (
                TcpStream {
                    inner: socket,
                },
                addr
            )
        )
    }

    #[stable(feature = "rust1", since = "1.0.0")]
    pub fn duplicate(&self) -> io::Result<TcpListener> {
        Ok(
            TcpListener {
                inner: self.inner.duplicate()?
            }
        )
    }

    #[stable(feature = "rust1", since = "1.0.0")]
    pub fn set_ttl(&self, ttl: u32) -> io::Result<()> {
        self.inner.set_ttl(ttl)
    }

    #[stable(feature = "rust1", since = "1.0.0")]
    pub fn ttl(&self) -> io::Result<u32> {
        self.inner.ttl()
    }

    #[stable(feature = "rust1", since = "1.0.0")]
    pub fn set_only_v6(&self, only_v6: bool) -> io::Result<()> {
        self.inner.set_only_v6(only_v6)
    }

    #[stable(feature = "rust1", since = "1.0.0")]
    pub fn only_v6(&self) -> io::Result<bool> {
        self.inner.only_v6()
    }

    #[stable(feature = "rust1", since = "1.0.0")]
    pub fn take_error(&self) -> io::Result<Option<io::Error>> {
        self.inner.take_error()
    }

    #[stable(feature = "rust1", since = "1.0.0")]
    pub fn set_nonblocking(&self, state: bool) -> io::Result<()> {
        self.inner.set_nonblocking(state)
    }

    #[stable(feature = "rust1", since = "1.0.0")]
    pub fn socket(&self) -> &Socket {
        &self.inner
    }

    #[stable(feature = "rust1", since = "1.0.0")]
    pub fn into_socket(self) -> Socket {
        self.inner
    }
}

#[stable(feature = "rust1", since = "1.0.0")]
impl AsInner<Socket> for TcpListener {
    fn as_inner(&self) -> &Socket {
        &self.inner
    }
}

#[stable(feature = "rust1", since = "1.0.0")]
impl IntoInner<Socket> for TcpListener {
    fn into_inner(self) -> Socket {
        self.inner
    }
}

#[stable(feature = "rust1", since = "1.0.0")]
impl FromInner<Socket> for TcpListener {
    fn from_inner(inner: Socket) -> TcpListener {
        TcpListener { inner }
    }
}

#[stable(feature = "rust1", since = "1.0.0")]
impl fmt::Debug for TcpListener {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TcpListener").field("fd", &self.inner.as_raw_fd()).finish()
    }
}

#[stable(feature = "rust1", since = "1.0.0")]
pub struct UdpSocket {
    inner: Socket,
}

impl UdpSocket {
    #[stable(feature = "rust1", since = "1.0.0")]
    pub fn bind(addr: io::Result<&SocketAddr>) -> io::Result<UdpSocket> {
        let addr = addr?;
        let sock = Socket::new(addr, SOCK_DGRAM)?;
        Ok(
            UdpSocket {
                inner: sock
            }
        )
    }

    #[stable(feature = "rust1", since = "1.0.0")]
    pub fn peer_addr(&self) -> io::Result<SocketAddr> {
        self.inner.peer_addr()
    }

    #[stable(feature = "rust1", since = "1.0.0")]
    pub fn socket_addr(&self) -> io::Result<SocketAddr> {
        self.inner.socket_addr()
    }

    #[stable(feature = "rust1", since = "1.0.0")]
    pub fn recv_from(&self, buf: &mut [u8]) -> io::Result<(usize, SocketAddr)> {
        self.inner.recv_from(buf)
    }

    #[stable(feature = "rust1", since = "1.0.0")]
    pub fn peek_from(&self, buf: &mut [u8]) -> io::Result<(usize, SocketAddr)> {
        self.inner.peek_from(buf)
    }

    #[stable(feature = "rust1", since = "1.0.0")]
    pub fn send_to(&self, buf: &[u8], addr: &SocketAddr) -> io::Result<usize> {
        self.inner.send_to(buf, *addr)
    }

    #[stable(feature = "rust1", since = "1.0.0")]
    pub fn duplicate(&self) -> io::Result<UdpSocket> {
        let sock = self.inner.duplicate()?;
        Ok(
            UdpSocket {
                inner: sock
            }
        )
    }

    #[stable(feature = "rust1", since = "1.0.0")]
    pub fn set_read_timeout(&self, dur: Option<Duration>) -> io::Result<()> {
        self.inner.set_timeout(dur, SO_RCVTIMEO)
    }

    #[stable(feature = "rust1", since = "1.0.0")]
    pub fn set_write_timeout(&self, dur: Option<Duration>) -> io::Result<()> {
        self.inner.set_timeout(dur, SO_SNDTIMEO)
    }

    #[stable(feature = "rust1", since = "1.0.0")]
    pub fn read_timeout(&self) -> io::Result<Option<Duration>> {
        self.inner.timeout(SO_RCVTIMEO)
    }

    #[stable(feature = "rust1", since = "1.0.0")]
    pub fn write_timeout(&self) -> io::Result<Option<Duration>> {
        self.inner.timeout(SO_SNDTIMEO)
    }

    #[stable(feature = "rust1", since = "1.0.0")]
    pub fn set_broadcast(&self, broadcast: bool) -> io::Result<()> {
        self.inner.set_broadcast(broadcast)
    }

    #[stable(feature = "rust1", since = "1.0.0")]
    pub fn broadcast(&self) -> io::Result<bool> {
        self.inner.broadcast()
    }

    #[stable(feature = "rust1", since = "1.0.0")]
    pub fn set_multicast_loop_v4(&self, val: bool) -> io::Result<()> {
        self.inner.set_multicast_loop_v4(val)
    }

    #[stable(feature = "rust1", since = "1.0.0")]
    pub fn multicast_loop_v4(&self) -> io::Result<bool> {
        self.inner.multicast_loop_v4()
    }

    #[stable(feature = "rust1", since = "1.0.0")]
    pub fn set_multicast_ttl_v4(&self, ttl: u32) -> io::Result<()> {
        self.inner.set_multicast_ttl_v4(ttl)
    }

    #[stable(feature = "rust1", since = "1.0.0")]
    pub fn multicast_ttl_v4(&self) -> io::Result<u32> {
        self.inner.multicast_ttl_v4()
    }

    #[stable(feature = "rust1", since = "1.0.0")]
    pub fn set_multicast_loop_v6(&self, val: bool) -> io::Result<()> {
        self.inner.set_multicast_loop_v6(val)
    }

    #[stable(feature = "rust1", since = "1.0.0")]
    pub fn multicast_loop_v6(&self) -> io::Result<bool> {
        self.inner.multicast_loop_v6()
    }

    #[stable(feature = "rust1", since = "1.0.0")]
    pub fn join_multicast_v4(&self, multiaddr: &Ipv4Addr, interface: &Ipv4Addr) -> io::Result<()> {
        self.inner.join_multicast_v4(multiaddr, interface)
    }

    #[stable(feature = "rust1", since = "1.0.0")]
    pub fn join_multicast_v6(&self, multiaddr: &Ipv6Addr, interface: u32) -> io::Result<()> {
        self.inner.join_multicast_v6(multiaddr, interface)
    }

    #[stable(feature = "rust1", since = "1.0.0")]
    pub fn leave_multicast_v4(&self, multiaddr: &Ipv4Addr, interface: &Ipv4Addr) -> io::Result<()> {
        self.inner.leave_multicast_v4(multiaddr, interface)
    }

    #[stable(feature = "rust1", since = "1.0.0")]
    pub fn leave_multicast_v6(&self, multiaddr: &Ipv6Addr, interface: u32) -> io::Result<()> {
        self.inner.leave_multicast_v6(multiaddr, interface)
    }

    #[stable(feature = "rust1", since = "1.0.0")]
    pub fn set_ttl(&self, ttl: u32) -> io::Result<()> {
        self.inner.set_ttl(ttl)
    }

    #[stable(feature = "rust1", since = "1.0.0")]
    pub fn ttl(&self) -> io::Result<u32> {
        self.inner.ttl()
    }

    #[stable(feature = "rust1", since = "1.0.0")]
    pub fn take_error(&self) -> io::Result<Option<io::Error>> {
        self.inner.take_error()
    }

    #[stable(feature = "rust1", since = "1.0.0")]
    pub fn set_nonblocking(&self, val: bool) -> io::Result<()> {
        self.inner.set_nonblocking(val)
    }

    #[stable(feature = "rust1", since = "1.0.0")]
    pub fn recv(&self, buf: &mut [u8]) -> io::Result<usize> {
        self.inner.recv(buf)
    }

    #[stable(feature = "rust1", since = "1.0.0")]
    pub fn peek(&self, buf: &mut [u8]) -> io::Result<usize> {
        self.inner.peek(buf)
    }

    #[stable(feature = "rust1", since = "1.0.0")]
    pub fn send(&self, buf: &[u8]) -> io::Result<usize> {
        self.inner.send(buf)
    }

    #[stable(feature = "rust1", since = "1.0.0")]
    pub fn connect(&self, addr: io::Result<&SocketAddr>) -> io::Result<()> {
        let addr = addr?;
        self.inner.connect(addr)
    }

    #[stable(feature = "rust1", since = "1.0.0")]
    pub fn socket(&self) -> &Socket {
        &self.inner
    }

    #[stable(feature = "rust1", since = "1.0.0")]
    pub fn into_socket(self) -> Socket {
        self.inner
    }
}

#[stable(feature = "rust1", since = "1.0.0")]
impl AsInner<Socket> for UdpSocket {
    fn as_inner(&self) -> &Socket {
        &self.inner
    }
}

#[stable(feature = "rust1", since = "1.0.0")]
impl IntoInner<Socket> for UdpSocket {
    fn into_inner(self) -> Socket {
        self.inner
    }
}

#[stable(feature = "rust1", since = "1.0.0")]
impl FromInner<Socket> for UdpSocket {
    fn from_inner(inner: Socket) -> UdpSocket {
        UdpSocket { inner }
    }
}

#[stable(feature = "rust1", since = "1.0.0")]
impl fmt::Debug for UdpSocket {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("UdpSocket").field("fd", &self.inner.as_raw_fd()).finish()
    }
}

#[stable(feature = "rust1", since = "1.0.0")]
pub struct LookupHost(VecDeque<IpAddr>, u16);

#[stable(feature = "rust1", since = "1.0.0")]
impl LookupHost {
    pub fn port(&self) -> u16 {
        self.1
    }
}

#[stable(feature = "rust1", since = "1.0.0")]
impl Iterator for LookupHost {
    type Item = SocketAddr;
    fn next(&mut self) -> Option<SocketAddr> {
        self.0.pop_front()
            .map(|ip| {
                SocketAddr::new(ip, self.port())
            })
    }
}

#[stable(feature = "rust1", since = "1.0.0")]
impl<'a> TryFrom<&'a str> for LookupHost {
    type Error = io::Error;

    fn try_from(v: &'a str) -> io::Result<LookupHost> {
        macro_rules! try_opt {
            ($e:expr, $msg:expr) => {
                match $e {
                    Some(r) => r,
                    None => return Err(io::const_io_error!(io::ErrorKind::InvalidInput, $msg)),
                }
            };
        }

        // split the string by ':' and convert the second part to u16
        let (host, port_str) = try_opt!(v.rsplit_once(':'), "invalid socket address");
        let port: u16 = try_opt!(port_str.parse().ok(), "invalid port value");
        TryFrom::try_from((host, port))
    }
}

#[stable(feature = "rust1", since = "1.0.0")]
impl<'a> TryFrom<(&'a str, u16)> for LookupHost {
    type Error = io::Error;

    fn try_from(v: (&'a str, u16)) -> io::Result<LookupHost> {
        let host = v.0;
        let port = v.1;
        let mut ret = VecDeque::new();
        unsafe {
            let mut ips = [crate::mem::MaybeUninit::<wasi::Addr>::zeroed(); 50];
            let cnt = wasi::resolve(host, port, ips.as_mut_ptr() as *mut wasi::Addr, ips.len()).map_err(err2io)?;
            for n in 0..cnt {
                let ip = ips[n].assume_init();
                let ip = conv_addr(ip);
                ret.push_back(ip);
            }
        }
        Ok(
            LookupHost(ret, port)
        )
    }
}

#[allow(nonstandard_style)]
pub mod netc {
    pub const AF_UNSPEC: i32 = 0;
    pub const AF_INET: i32 = 1;
    pub const AF_INET6: i32 = 2;
    pub const AF_UNIX: i32 = 3;
    pub type sa_family_t = u8;

    pub const SOCK_STREAM: i32 = 0;
    pub const SOCK_DGRAM: i32 = 1;
    pub const SOCK_RAW: i32 = 2;

    pub const SO_RCVTIMEO: i32 = 19;
    pub const SO_SNDTIMEO: i32 = 20;
    pub const SO_CONNTIMEO: i32 = 21;
    pub const SO_ACCPTIMEO: i32 = 22;

    pub const MSG_PEEK: u32 = 1;
    pub const MSG_WAITALL: u32 = 2;
    pub const MSG_TRUNC: u32 = 4;

    #[derive(Copy, Clone)]
    pub struct in_addr {
        pub s_addr: u32,
    }

    #[derive(Copy, Clone)]
    pub struct sockaddr_in {
        pub sin_family: sa_family_t,
        pub sin_port: u16,
        pub sin_addr: in_addr,
    }

    #[derive(Copy, Clone)]
    pub struct in6_addr {
        pub s6_addr: [u8; 16],
    }

    #[derive(Copy, Clone)]
    pub struct sockaddr_in6 {
        pub sin6_family: sa_family_t,
        pub sin6_port: u16,
        pub sin6_addr: in6_addr,
        pub sin6_flowinfo: u32,
        pub sin6_scope_id: u32,
    }

    #[derive(Copy, Clone)]
    pub struct sockaddr {}

    pub type socklen_t = usize;
}

use netc::*;
