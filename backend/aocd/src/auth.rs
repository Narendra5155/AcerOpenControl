//! Who may change settings.
//!
//! The socket is world-connectable. Each connection gets an [`Access`] level:
//! - read-only requests are open to everyone;
//! - root and members of [`ADMIN_GROUPS`] may change anything. Membership is
//!   looked up from the group database (not the caller's login session), so
//!   adding a user to a group works without logging out;
//! - users named in the config's `lease_users` (default: CoolerControl's
//!   `cc-plugin-user`) may only borrow the fans ([`LEASE_CMDS`]).

use std::ffi::{CStr, CString};
use std::io;
use std::mem::size_of;
use std::os::fd::AsRawFd;
use std::os::unix::net::UnixStream;

pub const ADMIN_GROUPS: &[&str] = &["sudo", "admin", "wheel", "acer-open-control"];

/// Requests that change state. Anything not listed here is read-only.
const MUTATING: &[&str] = &[
    "set_mode",
    "set_curve",
    "reapply",
    "set_kbd",
    "set_perf_profile",
    "set_charge_limit",
    "set_webcam",
    "claim_fans",
    "set_fan_duty",
    "release_fans",
];

/// The only changes a lease user may make.
pub const LEASE_CMDS: &[&str] = &["claim_fans", "set_fan_duty", "release_fans"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Access {
    ReadOnly,
    FanLease,
    Admin,
}

pub fn is_mutating(cmd: &str) -> bool {
    MUTATING.contains(&cmd)
}

pub fn allowed(access: Access, cmd: &str) -> bool {
    !is_mutating(cmd) || access == Access::Admin || (access == Access::FanLease && LEASE_CMDS.contains(&cmd))
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

/// Login name of a uid (works for systemd dynamic users through nss-systemd).
pub fn user_name(uid: u32) -> Option<String> {
    let mut pw: libc::passwd = unsafe { std::mem::zeroed() };
    let mut buf = vec![0 as libc::c_char; 16384];
    let mut out: *mut libc::passwd = std::ptr::null_mut();
    // SAFETY: reentrant lookup into caller-provided buffers.
    let r = unsafe { libc::getpwuid_r(uid, &mut pw, buf.as_mut_ptr(), buf.len(), &mut out) };
    if r != 0 || out.is_null() {
        return None;
    }
    // SAFETY: pw_name points into buf and is NUL-terminated.
    Some(unsafe { CStr::from_ptr(pw.pw_name) }.to_string_lossy().into_owned())
}

pub fn access_for(uid: u32, lease_users: &[String]) -> Access {
    if may_modify(uid, &user_gids(uid), &admin_gids()) {
        Access::Admin
    } else if user_name(uid).is_some_and(|n| lease_users.contains(&n)) {
        Access::FanLease
    } else {
        Access::ReadOnly
    }
}

pub fn peer_access(stream: &UnixStream, lease_users: &[String]) -> Access {
    match peer_uid(stream) {
        Ok(0) => Access::Admin,
        Ok(uid) => access_for(uid, lease_users),
        Err(_) => Access::ReadOnly,
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
        for cmd in LEASE_CMDS {
            assert!(is_mutating(cmd), "{cmd} must be gated");
            assert!(allowed(Access::FanLease, cmd));
            assert!(!allowed(Access::ReadOnly, cmd));
        }
        assert!(!allowed(Access::FanLease, "set_mode"), "lease users may only borrow fans");
        assert!(!allowed(Access::FanLease, "set_charge_limit"));
        assert!(allowed(Access::Admin, "set_mode"));
        assert!(allowed(Access::ReadOnly, "status"));
    }

    #[test]
    fn peer_is_self() {
        let (a, _b) = UnixStream::pair().unwrap();
        assert_eq!(peer_uid(&a).unwrap(), unsafe { libc::getuid() });
        assert!(user_gids(unsafe { libc::getuid() }).contains(&unsafe { libc::getgid() }));
        assert!(user_name(0).as_deref() == Some("root"));
        assert_eq!(access_for(65534, &["nobody".into()]), Access::FanLease);
        assert_eq!(access_for(65534, &[]), Access::ReadOnly);
    }
}
