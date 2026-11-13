//! The system calls. Every `unsafe` block is a single call into libc
//! with arguments built just above it, and says what makes it sound.

#![allow(unsafe_code)]

use std::collections::VecDeque;
use std::ffi::CStr;
use std::fs::File;
use std::io::{self, Read, Write};
use std::os::fd::AsRawFd;
use std::time::Duration;

use crate::{Addresses, Mac, permission, timeval, wanted};

/// An interface's MAC address and first IPv4 address, from
/// `getifaddrs(3)`.
///
/// # Errors
/// The call failed, or no interface of that name has a link-layer
/// address.
pub fn addresses(interface: &str) -> io::Result<Addresses> {
    let mut list: *mut libc::ifaddrs = std::ptr::null_mut();
    // SAFETY: `list` is a valid out-pointer; on success it holds a list
    // this function frees below with `freeifaddrs`.
    if unsafe { libc::getifaddrs(&raw mut list) } != 0 {
        return Err(io::Error::last_os_error());
    }
    let mut mac = None;
    let mut ipv4 = None;
    let mut entry = list;
    while !entry.is_null() {
        // SAFETY: `entry` is a non-null node of the list `getifaddrs`
        // returned, which stays allocated until `freeifaddrs` below.
        let ifa = unsafe { &*entry };
        entry = ifa.ifa_next;
        if ifa.ifa_name.is_null() || ifa.ifa_addr.is_null() {
            continue;
        }
        // SAFETY: `ifa_name` is a NUL-terminated string owned by the list.
        let name = unsafe { CStr::from_ptr(ifa.ifa_name) };
        if name.to_bytes() != interface.as_bytes() {
            continue;
        }
        // SAFETY: `ifa_addr` is non-null and points at a sockaddr whose
        // family field is always readable.
        let family = i32::from(unsafe { (*ifa.ifa_addr).sa_family });
        if family == libc::AF_INET && ipv4.is_none() {
            // SAFETY: an AF_INET address is a `sockaddr_in`.
            let sin = unsafe { &*ifa.ifa_addr.cast::<libc::sockaddr_in>() };
            ipv4 = Some(sin.sin_addr.s_addr.to_ne_bytes());
        }
        if mac.is_none() {
            mac = link_address(family, ifa.ifa_addr);
        }
    }
    // SAFETY: `list` came from `getifaddrs` and is freed exactly once.
    unsafe { libc::freeifaddrs(list) };
    let mac = mac.ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            format!("no network interface named {interface:?} with a MAC address"),
        )
    })?;
    Ok(Addresses { mac, ipv4 })
}

/// The six-byte hardware address in a link-layer sockaddr.
#[cfg(any(target_os = "macos", target_os = "ios", target_os = "freebsd"))]
fn link_address(family: i32, addr: *const libc::sockaddr) -> Option<Mac> {
    if family != libc::AF_LINK {
        return None;
    }
    // SAFETY: an AF_LINK address is a `sockaddr_dl`.
    let dl = unsafe { &*addr.cast::<libc::sockaddr_dl>() };
    let (name_len, addr_len) = (usize::from(dl.sdl_nlen), usize::from(dl.sdl_alen));
    if addr_len != 6 {
        return None;
    }
    // The address follows the interface name in `sdl_data`, which may run
    // past the struct's nominal 12 bytes; `sdl_len` says how far the
    // kernel filled it.
    let end = 8 + name_len + addr_len;
    if end > usize::from(dl.sdl_len) {
        return None;
    }
    let mut mac = [0u8; 6];
    // SAFETY: bytes `8 + name_len .. end` are within the `sdl_len` bytes
    // the kernel wrote for this address, checked above.
    unsafe {
        std::ptr::copy_nonoverlapping(addr.cast::<u8>().add(8 + name_len), mac.as_mut_ptr(), 6);
    }
    Some(mac)
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn link_address(family: i32, addr: *const libc::sockaddr) -> Option<Mac> {
    if family != libc::AF_PACKET {
        return None;
    }
    // SAFETY: an AF_PACKET address is a `sockaddr_ll`.
    let ll = unsafe { &*addr.cast::<libc::sockaddr_ll>() };
    (ll.sll_halen == 6).then(|| {
        let a = ll.sll_addr;
        [a[0], a[1], a[2], a[3], a[4], a[5]]
    })
}

#[cfg(not(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "freebsd",
    target_os = "linux",
    target_os = "android"
)))]
fn link_address(_family: i32, _addr: *const libc::sockaddr) -> Option<Mac> {
    None
}

/// One interface's frames of one `EtherType`.
pub struct RawSocket {
    file: File,
    mac: Mac,
    ethertype: u16,
    buf: Vec<u8>,
    pending: VecDeque<Vec<u8>>,
    timeout: Option<Duration>,
}

impl RawSocket {
    /// The interface's own MAC address.
    #[must_use]
    pub const fn mac(&self) -> Mac {
        self.mac
    }

    /// Sends one complete frame (destination, source, type, payload).
    ///
    /// # Errors
    /// The kernel refused it.
    pub fn send(&mut self, frame: &[u8]) -> io::Result<()> {
        let n = self.file.write(frame)?;
        if n == frame.len() {
            Ok(())
        } else {
            Err(io::Error::new(
                io::ErrorKind::WriteZero,
                format!("{n} of {} bytes sent", frame.len()),
            ))
        }
    }

    /// Waits up to `timeout` for a frame of the `EtherType` that this
    /// socket did not send.
    ///
    /// # Errors
    /// The read failed for another reason than the timeout.
    pub fn recv(&mut self, timeout: Duration) -> io::Result<Option<Vec<u8>>> {
        if let Some(f) = self.pending.pop_front() {
            return Ok(Some(f));
        }
        if self.timeout != Some(timeout) {
            self.set_timeout(timeout)?;
            self.timeout = Some(timeout);
        }
        let n = match self.file.read(&mut self.buf) {
            Ok(n) => n,
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut | io::ErrorKind::Interrupted
                ) =>
            {
                return Ok(None);
            }
            Err(e) => return Err(e),
        };
        self.take(n);
        Ok(self.pending.pop_front())
    }
}

#[cfg(any(target_os = "macos", target_os = "ios", target_os = "freebsd"))]
mod bpf {
    //! `net/bpf.h`, as the macOS SDK compiles it on a 64-bit target:
    //! `_IOW('B', n, T)` with `sizeof(T)` in the number.
    pub const BIOCGBLEN: libc::c_ulong = 0x4004_4266;
    pub const BIOCSETF: libc::c_ulong = 0x8010_4267;
    pub const BIOCSETIF: libc::c_ulong = 0x8020_426C;
    pub const BIOCSRTIMEOUT: libc::c_ulong = 0x8010_426D;
    pub const BIOCIMMEDIATE: libc::c_ulong = 0x8004_4270;
    pub const BIOCSHDRCMPLT: libc::c_ulong = 0x8004_4275;
    pub const BIOCSSEESENT: libc::c_ulong = 0x8004_4277;

    /// `struct ifreq`: the name and a 16-byte union this crate leaves 0.
    #[repr(C)]
    pub struct IfReq {
        pub name: [u8; 16],
        pub data: [u8; 16],
    }

    /// `struct bpf_program`.
    #[repr(C)]
    pub struct Program {
        pub len: u32,
        pub insns: *const crate::BpfInsn,
    }
}

#[cfg(any(target_os = "macos", target_os = "ios", target_os = "freebsd"))]
impl RawSocket {
    /// Opens the first free BPF device on `interface`.
    ///
    /// # Errors
    /// No such interface, no access to `/dev/bpf*`, or the kernel
    /// refused a setting.
    pub fn open(interface: &str, ethertype: u16) -> io::Result<Self> {
        let mac = crate::addresses(interface)?.mac;
        if interface.len() > 15 || interface.contains('\0') {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "interface name too long",
            ));
        }
        let mut file = None;
        for n in 0..256 {
            match File::options()
                .read(true)
                .write(true)
                .open(format!("/dev/bpf{n}"))
            {
                Ok(f) => {
                    file = Some(f);
                    break;
                }
                // In use by another capture: try the next.
                Err(e) if e.raw_os_error() == Some(libc::EBUSY) => {}
                Err(e) if e.kind() == io::ErrorKind::NotFound => break,
                Err(e) => {
                    return Err(permission(
                        e,
                        "raw Ethernet needs read and write access to /dev/bpf* (root, or the access_bpf group)",
                    ));
                }
            }
        }
        let file = file.ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no free /dev/bpf device"))?;
        let fd = file.as_raw_fd();
        let mut req = bpf::IfReq {
            name: [0; 16],
            data: [0; 16],
        };
        req.name[..interface.len()].copy_from_slice(interface.as_bytes());
        // SAFETY: `fd` is an open BPF device and `req` a properly sized,
        // NUL-terminated `struct ifreq` that outlives the call.
        check(unsafe { libc::ioctl(fd, bpf::BIOCSETIF, &raw const req) })?;
        let on: libc::c_uint = 1;
        let off: libc::c_uint = 0;
        for (request, value) in [
            // Hand frames over as they arrive, not when the buffer fills.
            (bpf::BIOCIMMEDIATE, &raw const on),
            // The caller writes the source address.
            (bpf::BIOCSHDRCMPLT, &raw const on),
            // Do not hand back what this device sends.
            (bpf::BIOCSSEESENT, &raw const off),
        ] {
            // SAFETY: each request takes a pointer to a `u_int`, which
            // `value` is, alive for the call.
            check(unsafe { libc::ioctl(fd, request, value) })?;
        }
        let insns = crate::bpf_filter(ethertype);
        let program = bpf::Program {
            len: 7,
            insns: insns.as_ptr(),
        };
        // SAFETY: `program` points at `len` instructions that outlive the
        // call; the kernel copies them.
        check(unsafe { libc::ioctl(fd, bpf::BIOCSETF, &raw const program) })?;
        let mut len: libc::c_uint = 0;
        // SAFETY: `len` is a valid `u_int` out-parameter.
        check(unsafe { libc::ioctl(fd, bpf::BIOCGBLEN, &raw mut len) })?;
        Ok(Self {
            file,
            mac,
            ethertype,
            // A read must offer the whole buffer length the device uses.
            buf: vec![0; len as usize],
            pending: VecDeque::new(),
            timeout: None,
        })
    }

    fn set_timeout(&self, timeout: Duration) -> io::Result<()> {
        let tv = tv(timeout);
        // SAFETY: `tv` is a `struct timeval` alive for the call.
        check(unsafe { libc::ioctl(self.file.as_raw_fd(), bpf::BIOCSRTIMEOUT, &raw const tv) })
    }

    /// Queues the frames of a read of `n` bytes.
    fn take(&mut self, n: usize) {
        for frame in crate::parse_bpf(&self.buf[..n]) {
            if wanted(frame, self.ethertype, self.mac) {
                self.pending.push_back(frame.to_vec());
            }
        }
    }
}

#[cfg(any(target_os = "linux", target_os = "android"))]
impl RawSocket {
    /// Opens a packet socket on `interface` for `ethertype`.
    ///
    /// # Errors
    /// No such interface, no `CAP_NET_RAW`, or the kernel refused the
    /// bind.
    pub fn open(interface: &str, ethertype: u16) -> io::Result<Self> {
        use std::ffi::CString;
        use std::os::fd::{FromRawFd, OwnedFd};

        let mac = crate::addresses(interface)?.mac;
        let name = CString::new(interface)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "interface name"))?;
        // SAFETY: `name` is a NUL-terminated string alive for the call.
        let index = unsafe { libc::if_nametoindex(name.as_ptr()) };
        if index == 0 {
            return Err(io::Error::last_os_error());
        }
        let protocol = ethertype.to_be();
        // SAFETY: plain arguments; the result is checked before use.
        let fd = unsafe {
            libc::socket(
                libc::AF_PACKET,
                libc::SOCK_RAW | libc::SOCK_CLOEXEC,
                i32::from(protocol),
            )
        };
        if fd < 0 {
            return Err(permission(
                io::Error::last_os_error(),
                "raw Ethernet needs CAP_NET_RAW (root, or setcap cap_net_raw+ep on the binary)",
            ));
        }
        // SAFETY: `fd` is a socket this function just created and owns.
        let owned = unsafe { OwnedFd::from_raw_fd(fd) };
        // SAFETY: an all-zero `sockaddr_ll` is a valid value of the type.
        let mut addr: libc::sockaddr_ll = unsafe { std::mem::zeroed() };
        addr.sll_family =
            libc::c_ushort::try_from(libc::AF_PACKET).map_err(|_| io::Error::other("AF_PACKET"))?;
        addr.sll_protocol = protocol;
        addr.sll_ifindex = i32::try_from(index).map_err(|_| io::Error::other("interface index"))?;
        // SAFETY: `addr` is a `sockaddr_ll` of the length passed, alive
        // for the call.
        check(unsafe {
            libc::bind(
                owned.as_raw_fd(),
                (&raw const addr).cast::<libc::sockaddr>(),
                socklen::<libc::sockaddr_ll>(),
            )
        })?;
        Ok(Self {
            file: File::from(owned),
            mac,
            ethertype,
            buf: vec![0; 2048],
            pending: VecDeque::new(),
            timeout: None,
        })
    }

    fn set_timeout(&self, timeout: Duration) -> io::Result<()> {
        let tv = tv(timeout);
        // SAFETY: `tv` is a `struct timeval` of the length passed, alive
        // for the call.
        check(unsafe {
            libc::setsockopt(
                self.file.as_raw_fd(),
                libc::SOL_SOCKET,
                libc::SO_RCVTIMEO,
                (&raw const tv).cast::<libc::c_void>(),
                socklen::<libc::timeval>(),
            )
        })
    }

    /// Queues the one frame a read of `n` bytes returned.
    fn take(&mut self, n: usize) {
        let frame = &self.buf[..n];
        if wanted(frame, self.ethertype, self.mac) {
            self.pending.push_back(frame.to_vec());
        }
    }
}

#[cfg(not(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "freebsd",
    target_os = "linux",
    target_os = "android"
)))]
impl RawSocket {
    /// # Errors
    /// Always: this platform has no raw Ethernet here.
    pub fn open(_interface: &str, _ethertype: u16) -> io::Result<Self> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "raw Ethernet is not implemented on this platform",
        ))
    }

    fn set_timeout(&self, _timeout: Duration) -> io::Result<()> {
        Ok(())
    }

    fn take(&mut self, _n: usize) {}
}

/// `timeout` as the `struct timeval` both kernels take.
fn tv(timeout: Duration) -> libc::timeval {
    let (sec, usec) = timeval(timeout);
    // The microseconds are below a million and the seconds a wait this
    // crate's callers ask for: both fit whatever width the platform uses.
    #[allow(clippy::cast_possible_truncation)]
    libc::timeval {
        tv_sec: sec as libc::time_t,
        tv_usec: usec as libc::suseconds_t,
    }
}

/// The size of `T` as the length argument of a socket call.
#[cfg(any(target_os = "linux", target_os = "android"))]
fn socklen<T>() -> libc::socklen_t {
    // Socket address and option structs are a few dozen bytes.
    libc::socklen_t::try_from(size_of::<T>()).unwrap_or(libc::socklen_t::MAX)
}

/// A libc return value as a result.
fn check(ret: libc::c_int) -> io::Result<()> {
    if ret < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

/// An interface's first IPv4 address and its netmask, from
/// `getifaddrs(3)`; `None` when it has no IPv4 address.
///
/// # Errors
/// The call failed.
pub fn ipv4_net(interface: &str) -> io::Result<Option<([u8; 4], [u8; 4])>> {
    let mut list: *mut libc::ifaddrs = std::ptr::null_mut();
    // SAFETY: `list` is a valid out-pointer; on success it holds a list
    // this function frees below with `freeifaddrs`.
    if unsafe { libc::getifaddrs(&raw mut list) } != 0 {
        return Err(io::Error::last_os_error());
    }
    let mut found = None;
    let mut entry = list;
    while !entry.is_null() && found.is_none() {
        // SAFETY: `entry` is a non-null node of the list `getifaddrs`
        // returned, which stays allocated until `freeifaddrs` below.
        let ifa = unsafe { &*entry };
        entry = ifa.ifa_next;
        if ifa.ifa_name.is_null() || ifa.ifa_addr.is_null() || ifa.ifa_netmask.is_null() {
            continue;
        }
        // SAFETY: `ifa_name` is a NUL-terminated string owned by the list.
        let name = unsafe { CStr::from_ptr(ifa.ifa_name) };
        // SAFETY: `ifa_addr` is non-null and points at a sockaddr whose
        // family field is always readable.
        let family = i32::from(unsafe { (*ifa.ifa_addr).sa_family });
        if name.to_bytes() != interface.as_bytes() || family != libc::AF_INET {
            continue;
        }
        // SAFETY: an AF_INET address, and the netmask beside it, are
        // `sockaddr_in`s.
        let (addr, mask) = unsafe {
            (
                &*ifa.ifa_addr.cast::<libc::sockaddr_in>(),
                &*ifa.ifa_netmask.cast::<libc::sockaddr_in>(),
            )
        };
        found = Some((
            addr.sin_addr.s_addr.to_ne_bytes(),
            mask.sin_addr.s_addr.to_ne_bytes(),
        ));
    }
    // SAFETY: `list` came from `getifaddrs` and is freed exactly once.
    unsafe { libc::freeifaddrs(list) };
    Ok(found)
}

/// Ties `socket` to `interface`: it then hears only what arrives there,
/// and what it sends, broadcasts included, leaves there. `IP_BOUND_IF`
/// on macOS (`ip(4)`), `SO_BINDTODEVICE` on Linux (`socket(7)`; before
/// Linux 5.7 it needs `CAP_NET_RAW`).
///
/// # Errors
/// No such interface, the kernel refused, or the platform has neither.
pub fn bind_to_interface(socket: &std::net::UdpSocket, interface: &str) -> io::Result<()> {
    let name = std::ffi::CString::new(interface)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "interface name"))?;
    bind_device(socket.as_raw_fd(), &name)
}

#[cfg(any(target_os = "macos", target_os = "ios"))]
fn bind_device(fd: std::os::fd::RawFd, name: &CStr) -> io::Result<()> {
    // SAFETY: `name` is a NUL-terminated string alive for the call.
    let index: libc::c_uint = unsafe { libc::if_nametoindex(name.as_ptr()) };
    if index == 0 {
        return Err(io::Error::last_os_error());
    }
    // An unsigned int is four bytes on every Apple target.
    let len = libc::socklen_t::try_from(size_of::<libc::c_uint>()).unwrap_or(4);
    // SAFETY: `index` is a `c_uint` of the length passed, alive for the
    // call; `fd` is an open socket the caller owns.
    check(unsafe {
        libc::setsockopt(
            fd,
            libc::IPPROTO_IP,
            libc::IP_BOUND_IF,
            (&raw const index).cast::<libc::c_void>(),
            len,
        )
    })
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn bind_device(fd: std::os::fd::RawFd, name: &CStr) -> io::Result<()> {
    let bytes = name.to_bytes_with_nul();
    let len = libc::socklen_t::try_from(bytes.len())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "interface name"))?;
    // SAFETY: `bytes` is the interface name with its NUL, of the length
    // passed, alive for the call; `fd` is an open socket the caller owns.
    check(unsafe {
        libc::setsockopt(
            fd,
            libc::SOL_SOCKET,
            libc::SO_BINDTODEVICE,
            bytes.as_ptr().cast::<libc::c_void>(),
            len,
        )
    })
    .map_err(|e| permission(e, "binding to an interface needs CAP_NET_RAW before Linux 5.7"))
}

#[cfg(not(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "linux",
    target_os = "android"
)))]
fn bind_device(_fd: std::os::fd::RawFd, _name: &CStr) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "binding a socket to an interface is not implemented on this platform",
    ))
}
