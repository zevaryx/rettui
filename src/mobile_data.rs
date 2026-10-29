//! Getting rettui's connections off mobile data once the phone has Wi-Fi
//! again. A TCP interface that (re)connects while Wi-Fi is briefly away
//! goes over mobile data, and stays there after Wi-Fi is back: the phone
//! keeps mobile data up in the background, and the hub's announces and the
//! keep-alive every few seconds keep the mobile radio from sleeping. On a
//! Pixel a 4 second Wi-Fi drop left one there for 12 hours, about a third of
//! the battery used. The Android app says when the phone's network changes
//! (`Native.networkChanged`); a connection shut down here reconnects by
//! itself, over the network the phone now uses.

#![cfg_attr(not(target_os = "android"), allow(dead_code))]

use std::collections::HashSet;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

/// Shut down this process's TCP connections that go over mobile data; how
/// many. Call it only when the phone's own network isn't mobile data, or
/// they'd only reconnect the same way.
pub fn leave() -> usize {
    let addresses = mobile_data_addresses();
    if addresses.is_empty() {
        return 0;
    }
    shut_down_streams(|local| addresses.contains(&local.ip().to_canonical()))
}

/// The addresses of the phone's mobile data interfaces.
fn mobile_data_addresses() -> HashSet<IpAddr> {
    let mut found = HashSet::new();
    let mut list: *mut libc::ifaddrs = std::ptr::null_mut();
    // SAFETY: getifaddrs fills `list`, freed below; each entry is read only
    // while the list is alive.
    unsafe {
        if libc::getifaddrs(&mut list) != 0 {
            return found;
        }
        let mut entry = list;
        while !entry.is_null() {
            let item = &*entry;
            entry = item.ifa_next;
            if item.ifa_addr.is_null() || item.ifa_name.is_null() {
                continue;
            }
            let name = std::ffi::CStr::from_ptr(item.ifa_name).to_string_lossy();
            if !crate::reticulum::is_mobile_data_device(&name) {
                continue;
            }
            if let Some(address) = socket_address(item.ifa_addr) {
                found.insert(address.ip().to_canonical());
            }
        }
        libc::freeifaddrs(list);
    }
    found
}

/// Shut down (without closing: their owners still hold them) the connected
/// TCP sockets of this process whose own address `matches`; how many. Their
/// owners read the end of the stream, as when the other side closes.
fn shut_down_streams(matches: impl Fn(SocketAddr) -> bool) -> usize {
    let Ok(fds) = std::fs::read_dir("/proc/self/fd") else { return 0 };
    let mut count = 0;
    for fd in fds.flatten().filter_map(|entry| entry.file_name().to_str()?.parse::<libc::c_int>().ok()) {
        if is_connected_stream(fd).is_some_and(&matches) {
            // SAFETY: a plain call on a descriptor of this process.
            if unsafe { libc::shutdown(fd, libc::SHUT_RDWR) } == 0 {
                count += 1;
            }
        }
    }
    count
}

/// The local address of `fd`, if it's a TCP socket connected to another.
fn is_connected_stream(fd: libc::c_int) -> Option<SocketAddr> {
    // SAFETY: fstat, getsockopt, getsockname and getpeername only write to
    // the buffers passed, of the sizes given.
    unsafe {
        let mut stat: libc::stat = std::mem::zeroed();
        if libc::fstat(fd, &mut stat) != 0 || stat.st_mode & libc::S_IFMT != libc::S_IFSOCK {
            return None;
        }
        let mut kind: libc::c_int = 0;
        let mut size = std::mem::size_of::<libc::c_int>() as libc::socklen_t;
        if libc::getsockopt(fd, libc::SOL_SOCKET, libc::SO_TYPE, (&raw mut kind).cast(), &mut size) != 0 || kind != libc::SOCK_STREAM {
            return None;
        }
        let mut peer: libc::sockaddr_storage = std::mem::zeroed();
        let mut size = std::mem::size_of::<libc::sockaddr_storage>() as libc::socklen_t;
        // A listening socket, or one still connecting, has no peer.
        if libc::getpeername(fd, (&raw mut peer).cast(), &mut size) != 0 {
            return None;
        }
        let mut local: libc::sockaddr_storage = std::mem::zeroed();
        let mut size = std::mem::size_of::<libc::sockaddr_storage>() as libc::socklen_t;
        if libc::getsockname(fd, (&raw mut local).cast(), &mut size) != 0 {
            return None;
        }
        socket_address((&raw const local).cast())
    }
}

/// An IPv4 or IPv6 socket address; None for any other kind.
///
/// # Safety
/// `address` points to a socket address as large as its family says.
unsafe fn socket_address(address: *const libc::sockaddr) -> Option<SocketAddr> {
    // SAFETY: the caller's promise; the family says which it is.
    unsafe {
        match libc::c_int::from((*address).sa_family) {
            libc::AF_INET => {
                let v4 = &*address.cast::<libc::sockaddr_in>();
                Some(SocketAddr::new(Ipv4Addr::from(u32::from_be(v4.sin_addr.s_addr)).into(), u16::from_be(v4.sin_port)))
            }
            libc::AF_INET6 => {
                let v6 = &*address.cast::<libc::sockaddr_in6>();
                Some(SocketAddr::new(Ipv6Addr::from(v6.sin6_addr.s6_addr).into(), u16::from_be(v6.sin6_port)))
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};

    #[test]
    fn shuts_down_only_the_connections_asked_for() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let mut leaving = TcpStream::connect(address).unwrap();
        let (mut leaving_far, _) = listener.accept().unwrap();
        let mut staying = TcpStream::connect(address).unwrap();
        let (mut staying_far, _) = listener.accept().unwrap();
        let mine = leaving.local_addr().unwrap();

        // Only the one asked for.
        assert_eq!(super::shut_down_streams(|local| local == mine), 1);
        let mut buf = [0u8; 8];
        assert_eq!(leaving.read(&mut buf).unwrap(), 0, "its owner reads the end of the stream");
        assert_eq!(leaving_far.read(&mut buf).unwrap(), 0, "and so does the other side");

        staying.write_all(b"still").unwrap();
        assert_eq!(staying_far.read(&mut buf).unwrap(), 5);
        // The accepted ones share the listener's address; it keeps listening.
        assert_eq!(super::shut_down_streams(|local| local == address), 2);
        let _again = TcpStream::connect(address).unwrap();
        assert!(listener.accept().is_ok(), "the listener isn't shut down");
    }

    #[test]
    fn reads_both_kinds_of_address() {
        for text in ["127.0.0.1:0", "[::1]:0"] {
            let Ok(socket) = std::net::TcpListener::bind(text) else { continue };
            let fd = std::os::fd::AsRawFd::as_raw_fd(&socket);
            let mut local: libc::sockaddr_storage = unsafe { std::mem::zeroed() };
            let mut size = std::mem::size_of::<libc::sockaddr_storage>() as libc::socklen_t;
            assert_eq!(unsafe { libc::getsockname(fd, (&raw mut local).cast(), &mut size) }, 0);
            assert_eq!(unsafe { super::socket_address((&raw const local).cast()) }, Some(socket.local_addr().unwrap()));
        }
    }

    #[test]
    fn leaves_nothing_without_mobile_data() {
        // No rmnet or the like here: nothing to do.
        assert!(super::mobile_data_addresses().is_empty());
        assert_eq!(super::leave(), 0);
    }
}
