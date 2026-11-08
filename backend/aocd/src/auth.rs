//! Who may change settings.
//!
//! The socket is world-connectable. Read-only requests are open to everyone;
//! requests that change hardware state require root or membership in one of
//! [`ADMIN_GROUPS`]. Membership is looked up from the group database on every
//! request (not from the caller's login session), so adding a user to a
//! group takes effect immediately, without logging out.

use std::ffi::{CStr, CString};
use std::io;
use std::mem::size_of;
use std::os::fd::AsRawFd;
use std::os::unix::net::UnixStream;

pub const ADMIN_GROUPS: &[&str] = &["sudo", "admin", "wheel", "acer-open-control"];

const MUTATING: &[&str] = &[
    "set_mode",
    "set_curve",
    "reapply",
    "set_kbd",
    "set_perf_profile",
    "set_charge_limit",
    "set_webcam",
];

pub fn is_mutating(cmd: &str) -> bool {
    MUTATING.contains(&cmd)
}

pub fn may_modify(uid: u32, user_gids: &[u32], admin_gids: &[u32]) -> bool {
    uid == 0 || user_gids.iter().any(|g| admin_gids.contains(g))
}

/// uid of the process on the other end of a Unix socket (SO_PEERCRED).
pub fn peer_uid(stream: &UnixStream) -> io::Result<u32> {
    // SAFETY: zeroed ucred is a valid out-parameter.
    let mut cred: libc::ucred = unsafe { std::mem::zeroed() };
    let mut len = size_of::<libc::ucred>() as libc::socklen_t;
    // SAFETY: valid fd, out-pointer and length.
    let r = unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            &mut cred as *mut libc::ucred as *mut libc::c_void,
            &mut len,
        )
    };
    if r < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(cred.uid)
}

fn group_gid(name: &str) -> Option<u32> {
    let cname = CString::new(name).ok()?;
    let mut grp: libc::group = unsafe { std::mem::zeroed() };
    let mut buf = vec![0 as libc::c_char; 16384];
    let mut out: *mut libc::group = std::ptr::null_mut();
    // SAFETY: reentrant lookup into caller-provided buffers.
    let r = unsafe { libc::getgrnam_r(cname.as_ptr(), &mut grp, buf.as_mut_ptr(), buf.len(), &mut out) };
    (r == 0 && !out.is_null()).then_some(grp.gr_gid)
}

pub fn admin_gids() -> Vec<u32> {
    ADMIN_GROUPS.iter().filter_map(|g| group_gid(g)).collect()
}

/// Every group the user belongs to, read from the group database now.
pub fn user_gids(uid: u32) -> Vec<u32> {
    let mut pw: libc::passwd = unsafe { std::mem::zeroed() };
    let mut buf = vec![0 as libc::c_char; 16384];
    let mut out: *mut libc::passwd = std::ptr::null_mut();
    // SAFETY: reentrant lookup into caller-provided buffers.
    let r = unsafe { libc::getpwuid_r(uid, &mut pw, buf.as_mut_ptr(), buf.len(), &mut out) };
    if r != 0 || out.is_null() {
        return vec![];
    }
    // SAFETY: pw_name points into buf and is NUL-terminated.
    let name = unsafe { CStr::from_ptr(pw.pw_name) }.to_owned();
    let mut n: libc::c_int = 64;
    loop {
        let mut groups = vec![0 as libc::gid_t; n as usize];
        let before = n;
        // SAFETY: groups has room for n entries; n is updated with the real count.
        let r = unsafe { libc::getgrouplist(name.as_ptr(), pw.pw_gid, groups.as_mut_ptr(), &mut n) };
        if r >= 0 {
            groups.truncate(n as usize);
            return groups;
        }
        if n <= before {
            n = before * 2;
        }
        if n > 65536 {
            return vec![pw.pw_gid];
        }
    }
}

pub fn peer_may_modify(stream: &UnixStream) -> bool {
    match peer_uid(stream) {
        Ok(0) => true,
        Ok(uid) => may_modify(uid, &user_gids(uid), &admin_gids()),
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rules() {
        assert!(may_modify(0, &[], &[]));
        assert!(may_modify(1000, &[1000, 27], &[27]));
        assert!(!may_modify(1000, &[1000, 4], &[27]));
        assert!(is_mutating("set_mode"));
        assert!(!is_mutating("status"));
        assert!(!is_mutating("get_model"));
    }

    #[test]
    fn peer_is_self() {
        let (a, _b) = UnixStream::pair().unwrap();
        assert_eq!(peer_uid(&a).unwrap(), unsafe { libc::getuid() });
        assert!(user_gids(unsafe { libc::getuid() }).contains(&unsafe { libc::getgid() }));
    }
}
