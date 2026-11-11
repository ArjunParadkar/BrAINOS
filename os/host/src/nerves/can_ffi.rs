//! BFS  LEVEL    Host - the only crate with an operating system (CLI, scenarios, OS nerves)  [crate brainos-host]
//! BFS  PURPOSE  The only unsafe code for CAN: opens a Linux SocketCAN raw socket and reads and writes classic CAN frames on it
//! BFS  DEFINES  OpenError, CanSocket
//! BFS  USES     external: std
//! BFS  USED BY  host/nerves/socketcan
//!
//! A Linux SocketCAN raw socket. **This file is where the `unsafe` is.**
//!
//! Hardware forces it: the standard library has TCP, UDP and Unix
//! sockets and nothing for the `AF_CAN` family. The six C library
//! calls needed are declared by hand; no crate is added.
//!
//! A CAN interface is a local field bus, not a network: frames go to
//! the wires of one bus and nowhere else. It is therefore not behind
//! `BRAINOS_NET`. (A `vcan` interface is the kernel's loopback version
//! of the same thing.)
//!
//! Layouts are from `linux/can.h`:
//!
//! ```text
//!   struct sockaddr_can { u16 family; i32 ifindex; u8 addr[16]; }   // padded to 24 bytes
//!   struct can_frame    { u32 can_id; u8 len; u8 pad, res0, res1; u8 data[8]; }  // 16 bytes
//! ```

#![allow(unsafe_code)]

use std::ffi::{c_char, c_int, c_uint, c_void, CString};

const AF_CAN: c_int = 29;
const SOCK_RAW: c_int = 3;
const CAN_RAW: c_int = 1;
const SOL_SOCKET: c_int = 1;
const SO_RCVTIMEO: c_int = 20;
/// classic CAN: 11-bit identifiers only
const CAN_SFF_MASK: u32 = 0x7FF;

extern "C" {
    fn socket(domain: c_int, ty: c_int, protocol: c_int) -> c_int;
    fn bind(fd: c_int, addr: *const c_void, len: c_uint) -> c_int;
    fn if_nametoindex(name: *const c_char) -> c_uint;
    fn setsockopt(fd: c_int, level: c_int, name: c_int, val: *const c_void, len: c_uint) -> c_int;
    fn read(fd: c_int, buf: *mut c_void, n: usize) -> isize;
    fn write(fd: c_int, buf: *const c_void, n: usize) -> isize;
    fn close(fd: c_int) -> c_int;
}

#[repr(C)]
struct SockaddrCan {
    family: u16,
    ifindex: c_int,
    addr: [u8; 16],
}

#[repr(C)]
#[derive(Default, Clone, Copy)]
struct RawFrame {
    can_id: u32,
    len: u8,
    pad: u8,
    res0: u8,
    res1: u8,
    data: [u8; 8],
}

#[repr(C)]
struct Timeval {
    sec: i64,
    usec: i64,
}

/// Why the socket could not be opened.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum OpenError {
    /// no interface by that name (`ip link` does not list it)
    NoSuchInterface,
    /// the kernel has no CAN support loaded, or refused the socket
    NoCanSupport,
    /// the interface exists but could not be bound (is it up?)
    CannotBind,
}

/// One open raw CAN socket, closed on drop.
pub struct CanSocket {
    fd: c_int,
}

impl CanSocket {
    /// Open a raw socket on a named interface (`can0`, `vcan0`) with a
    /// receive timeout.
    pub fn open(ifname: &str, recv_timeout_ms: u32) -> Result<CanSocket, OpenError> {
        let name = CString::new(ifname).map_err(|_| OpenError::NoSuchInterface)?;
        // SAFETY: `name` is NUL-terminated and outlives the call. Every
        // return value is checked. The sockaddr and timeval are live
        // locals of the layouts the kernel expects, passed with their
        // exact sizes. The fd is closed on every error path.
        unsafe {
            let index = if_nametoindex(name.as_ptr());
            if index == 0 {
                return Err(OpenError::NoSuchInterface);
            }
            let fd = socket(AF_CAN, SOCK_RAW, CAN_RAW);
            if fd < 0 {
                return Err(OpenError::NoCanSupport);
            }
            let addr = SockaddrCan {
                family: AF_CAN as u16,
                ifindex: index as c_int,
                addr: [0; 16],
            };
            if bind(
                fd,
                &addr as *const SockaddrCan as *const c_void,
                std::mem::size_of::<SockaddrCan>() as c_uint,
            ) != 0
            {
                close(fd);
                return Err(OpenError::CannotBind);
            }
            let tv = Timeval {
                sec: (recv_timeout_ms / 1000) as i64,
                usec: (recv_timeout_ms % 1000) as i64 * 1000,
            };
            setsockopt(
                fd,
                SOL_SOCKET,
                SO_RCVTIMEO,
                &tv as *const Timeval as *const c_void,
                std::mem::size_of::<Timeval>() as c_uint,
            );
            Ok(CanSocket { fd })
        }
    }

    /// Send one classic frame. False if the kernel did not take it.
    pub fn send(&mut self, id: u16, data: &[u8]) -> bool {
        let mut f = RawFrame {
            can_id: id as u32 & CAN_SFF_MASK,
            len: data.len().min(8) as u8,
            ..RawFrame::default()
        };
        f.data[..f.len as usize].copy_from_slice(&data[..f.len as usize]);
        // SAFETY: a live repr(C) struct of exactly the size written.
        let n = unsafe {
            write(
                self.fd,
                &f as *const RawFrame as *const c_void,
                std::mem::size_of::<RawFrame>(),
            )
        };
        n == std::mem::size_of::<RawFrame>() as isize
    }

    /// Receive one frame, or `None` when the timeout passes. Frames
    /// with extended identifiers or error flags are not ours and are
    /// skipped.
    pub fn recv(&mut self) -> Option<(u16, u8, [u8; 8])> {
        let mut f = RawFrame::default();
        // SAFETY: a live repr(C) struct of exactly the size read into;
        // a short or failed read is treated as nothing received.
        let n = unsafe {
            read(
                self.fd,
                &mut f as *mut RawFrame as *mut c_void,
                std::mem::size_of::<RawFrame>(),
            )
        };
        if n != std::mem::size_of::<RawFrame>() as isize || f.can_id > CAN_SFF_MASK || f.len > 8 {
            return None;
        }
        Some((f.can_id as u16, f.len, f.data))
    }
}

impl Drop for CanSocket {
    fn drop(&mut self) {
        // SAFETY: the fd was returned by socket() and is closed once.
        unsafe {
            close(self.fd);
        }
    }
}
