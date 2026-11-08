//! An `LD_PRELOAD` shim that binds every outbound INET socket to one network interface.
//!
//! Deno's own `fetch` (streams, binary bodies, `FormData`, `Request` objects, WebSocket) is the
//! real implementation, and the host cannot reproduce all of it through a relay. This shim instead
//! leaves the API alone and constrains the *sockets*: `SO_BINDTODEVICE` on every AF_INET/AF_INET6
//! `connect`/`sendto` before it happens, so the kernel picks that device's route or fails.
//!
//! Loaded into the Deno dispatcher process with `LANRURUGI_EGRESS_IF=<interface>` set. The
//! constructor exports `LANRURUGI_EGRESS_ACTIVE=1`, which is how the dispatcher knows the shim is
//! really in place before it decides to keep native `fetch` instead of falling back to the host
//! relay — a missing or unloadable `.so` must never leave traffic unconstrained.
//!
//! Loopback destinations are left alone: a container's own DNS listener (gluetun answers on
//! `127.0.0.1`) and `localhost` callbacks are not egress and binding them to `tun0` would break
//! resolution rather than secure it.

use std::ffi::{c_int, c_void, CString};
use std::sync::OnceLock;

type ConnectFn = unsafe extern "C" fn(c_int, *const libc::sockaddr, libc::socklen_t) -> c_int;
type SendToFn = unsafe extern "C" fn(
    c_int,
    *const c_void,
    libc::size_t,
    c_int,
    *const libc::sockaddr,
    libc::socklen_t,
) -> isize;

static REAL_CONNECT: OnceLock<Option<ConnectFn>> = OnceLock::new();
static REAL_SENDTO: OnceLock<Option<SendToFn>> = OnceLock::new();
static IFACE: OnceLock<Option<CString>> = OnceLock::new();

fn real_connect() -> Option<ConnectFn> {
    *REAL_CONNECT.get_or_init(|| unsafe {
        let sym = libc::dlsym(libc::RTLD_NEXT, c"connect".as_ptr());
        (!sym.is_null()).then(|| std::mem::transmute::<*mut c_void, ConnectFn>(sym))
    })
}

fn real_sendto() -> Option<SendToFn> {
    *REAL_SENDTO.get_or_init(|| unsafe {
        let sym = libc::dlsym(libc::RTLD_NEXT, c"sendto".as_ptr());
        (!sym.is_null()).then(|| std::mem::transmute::<*mut c_void, SendToFn>(sym))
    })
}

fn iface() -> Option<&'static CString> {
    IFACE
        .get_or_init(|| {
            let raw = std::env::var_os("LANRURUGI_EGRESS_IF")?;
            let text = raw.to_string_lossy();
            let text = text.trim();
            (!text.is_empty()).then(|| CString::new(text).ok())?
        })
        .as_ref()
}

/// Is this destination loopback (or the unspecified/`localhost` family) and therefore not egress?
///
/// # Safety
///
/// `addr` must point at a `sockaddr` whose `sa_family` describes the memory actually behind it —
/// the caller (`connect`/`sendto`) received it from libc under exactly that contract.
///
/// The `sockaddr` is read field by field: `connect()` hands us whatever the caller passed, and the
/// family is the only field guaranteed to exist for both address families.
unsafe fn is_loopback(addr: *const libc::sockaddr) -> bool {
    if addr.is_null() {
        return true;
    }
    match (*addr).sa_family as c_int {
        libc::AF_INET => {
            let sin = &*(addr as *const libc::sockaddr_in);
            // `s_addr` holds the address in network order; read back through the host's own byte
            // order to recover the octets as they sit in memory.
            let first = sin.sin_addr.s_addr.to_ne_bytes()[0];
            first == 127 || first == 0
        }
        libc::AF_INET6 => {
            let sin6 = &*(addr as *const libc::sockaddr_in6);
            let octets = sin6.sin6_addr.s6_addr;
            let loopback = octets[..15].iter().all(|b| *b == 0) && octets[15] == 1;
            // ::ffff:127.x.x.x — an IPv4 loopback expressed as IPv6.
            let v4_mapped_loopback = octets[..10].iter().all(|b| *b == 0)
                && octets[10] == 0xff
                && octets[11] == 0xff
                && octets[12] == 127;
            // Link-local (fe80::/10) is not routable through the interface we are pinning to.
            let link_local = octets[0] == 0xfe && (octets[1] & 0xc0) == 0x80;
            loopback || v4_mapped_loopback || link_local
        }
        _ => true,
    }
}

/// Pins `fd` to the configured interface. Returns `false` when the interface cannot be used, which
/// the callers turn into a failed syscall: a socket that cannot honour the choice must not fall
/// back to the default route, or the setting would be advisory rather than binding.
///
/// # Safety
///
/// `fd` must be a file descriptor the caller owns and is allowed to `setsockopt` on. A descriptor
/// of another kind (a regular file, say) makes this fail with `ENOTSOCK`, which the caller turns
/// into a refused connection rather than a crash.
unsafe fn bind_device(fd: c_int) -> bool {
    let Some(name) = iface() else {
        return true;
    };
    let bytes = name.as_bytes_with_nul();
    let ok = libc::setsockopt(
        fd,
        libc::SOL_SOCKET,
        libc::SO_BINDTODEVICE,
        bytes.as_ptr() as *const c_void,
        bytes.len() as libc::socklen_t,
    ) == 0;
    if !ok {
        // Leave errno from setsockopt in place so the caller sees EPERM/ENODEV, not a bare -1.
        return false;
    }
    true
}

/// libc's `connect`, wrapped to pin the socket before the real call. `#[no_mangle]` keeps the
/// symbol name the dynamic loader looks up for `LD_PRELOAD` interposition.
///
/// # Safety
///
/// The signature mirrors libc's own: `addr` must describe `len` bytes of a valid `sockaddr` for
/// `fd`, and `fd` must be a socket the caller owns. Arguments are forwarded unchanged to the real
/// function resolved through `RTLD_NEXT`.
#[no_mangle]
pub unsafe extern "C" fn connect(
    fd: c_int,
    addr: *const libc::sockaddr,
    len: libc::socklen_t,
) -> c_int {
    if !is_loopback(addr) && !bind_device(fd) {
        return -1;
    }
    match real_connect() {
        Some(f) => f(fd, addr, len),
        None => {
            *libc::__errno_location() = libc::ENOSYS;
            -1
        }
    }
}

/// libc's `sendto`, wrapped the same way — UDP flows (DNS in particular) call this instead of
/// `connect`.
///
/// # Safety
///
/// The signature mirrors libc's own: `buf` must be readable for `len` bytes, `addr` must describe
/// `addr_len` bytes of a valid `sockaddr` for `fd`, and `fd` must be a socket the caller owns.
#[no_mangle]
pub unsafe extern "C" fn sendto(
    fd: c_int,
    buf: *const c_void,
    len: libc::size_t,
    flags: c_int,
    addr: *const libc::sockaddr,
    addr_len: libc::socklen_t,
) -> isize {
    if !is_loopback(addr) && !bind_device(fd) {
        return -1;
    }
    match real_sendto() {
        Some(f) => f(fd, buf, len, flags, addr, addr_len),
        None => {
            *libc::__errno_location() = libc::ENOSYS;
            -1
        }
    }
}

/// Tells the dispatcher the shim is loaded and in force. Runs before `main`, so the environment is
/// single-threaded and this is the same mechanism glibc uses for its own constructors.
extern "C" fn mark_active() {
    std::env::set_var("LANRURUGI_EGRESS_ACTIVE", "1");
}

#[used]
#[link_section = ".init_array"]
static INIT: extern "C" fn() = mark_active;
