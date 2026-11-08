//! Subscriber for the kernel's `acpi_event` generic-netlink multicast group.
//! The WMI core forwards firmware notifies (e.g. Clevo hotkeys, 0xD0) here.

use std::io;
use std::mem::size_of;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::time::Duration;

const NETLINK_GENERIC: libc::c_int = 16;
const SOL_NETLINK: libc::c_int = 270;
const NETLINK_ADD_MEMBERSHIP: libc::c_int = 1;
const GENL_ID_CTRL: u16 = 0x10;
const CTRL_CMD_GETFAMILY: u8 = 3;
const CTRL_ATTR_FAMILY_ID: u16 = 1;
const CTRL_ATTR_FAMILY_NAME: u16 = 2;
const CTRL_ATTR_MCAST_GROUPS: u16 = 7;
const CTRL_ATTR_MCAST_GRP_NAME: u16 = 1;
const CTRL_ATTR_MCAST_GRP_ID: u16 = 2;
const NLM_F_REQUEST: u16 = 1;
const NLMSG_ERROR: u16 = 2;
const NLMSG_HDRLEN: usize = 16;
const GENL_HDRLEN: usize = 4;
const ACPI_GENL_ATTR_EVENT: u16 = 1;
/// struct acpi_genl_event { char class[20]; char bus_id[15]; u32 type; u32 data; }
const ACPI_EVENT_SIZE: usize = 44;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcpiEvent {
    pub device_class: String,
    pub bus_id: String,
    pub kind: u32,
    pub data: u32,
}

fn align4(n: usize) -> usize {
    (n + 3) & !3
}

fn cstr(b: &[u8]) -> String {
    let end = b.iter().position(|&c| c == 0).unwrap_or(b.len());
    String::from_utf8_lossy(&b[..end]).into_owned()
}

/// Iterate netlink attributes: (type, payload).
pub fn attrs(buf: &[u8]) -> Vec<(u16, &[u8])> {
    let mut out = Vec::new();
    let mut off = 0;
    while off + 4 <= buf.len() {
        let len = u16::from_ne_bytes([buf[off], buf[off + 1]]) as usize;
        let typ = u16::from_ne_bytes([buf[off + 2], buf[off + 3]]) & 0x3FFF;
        if len < 4 || off + len > buf.len() {
            break;
        }
        out.push((typ, &buf[off + 4..off + len]));
        off += align4(len);
    }
    out
}

/// Parse a genetlink payload (after the genl header) carrying an ACPI event.
pub fn parse_acpi_event(payload: &[u8]) -> Option<AcpiEvent> {
    for (typ, val) in attrs(payload) {
        if typ == ACPI_GENL_ATTR_EVENT && val.len() >= ACPI_EVENT_SIZE {
            return Some(AcpiEvent {
                device_class: cstr(&val[0..20]),
                bus_id: cstr(&val[20..35]),
                kind: u32::from_ne_bytes(val[36..40].try_into().ok()?),
                data: u32::from_ne_bytes(val[40..44].try_into().ok()?),
            });
        }
    }
    None
}

/// Split a datagram into netlink messages: (type, payload after nlmsghdr).
pub fn messages(data: &[u8]) -> Vec<(u16, &[u8])> {
    let mut out = Vec::new();
    let mut off = 0;
    while off + NLMSG_HDRLEN <= data.len() {
        let len = u32::from_ne_bytes(data[off..off + 4].try_into().unwrap()) as usize;
        let typ = u16::from_ne_bytes([data[off + 4], data[off + 5]]);
        if len < NLMSG_HDRLEN || off + len > data.len() {
            break;
        }
        out.push((typ, &data[off + NLMSG_HDRLEN..off + len]));
        off += align4(len);
    }
    out
}

pub struct Listener {
    fd: OwnedFd,
    family: u16,
}

impl Listener {
    pub fn subscribe() -> io::Result<Listener> {
        // SAFETY: plain socket(2) call; the fd is immediately owned.
        let raw = unsafe { libc::socket(libc::AF_NETLINK, libc::SOCK_RAW | libc::SOCK_CLOEXEC, NETLINK_GENERIC) };
        if raw < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: raw is a fresh, valid fd we own.
        let fd = unsafe { OwnedFd::from_raw_fd(raw) };
        // SAFETY: zeroed sockaddr_nl is a valid "any" address.
        let mut addr: libc::sockaddr_nl = unsafe { std::mem::zeroed() };
        addr.nl_family = libc::AF_NETLINK as u16;
        // SAFETY: addr is a properly sized sockaddr_nl.
        let r = unsafe {
            libc::bind(
                fd.as_raw_fd(),
                &addr as *const _ as *const libc::sockaddr,
                size_of::<libc::sockaddr_nl>() as u32,
            )
        };
        if r < 0 {
            return Err(io::Error::last_os_error());
        }
        let mut l = Listener { fd, family: 0 };
        let (family, group) = l.resolve("acpi_event", "acpi_mc_group")?;
        l.family = family;
        // SAFETY: option value is a u32 group id.
        let r = unsafe {
            libc::setsockopt(
                l.fd.as_raw_fd(),
                SOL_NETLINK,
                NETLINK_ADD_MEMBERSHIP,
                &group as *const u32 as *const libc::c_void,
                size_of::<u32>() as u32,
            )
        };
        if r < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(l)
    }

    fn send(&self, buf: &[u8]) -> io::Result<()> {
        // SAFETY: buf is a valid slice.
        let n = unsafe { libc::send(self.fd.as_raw_fd(), buf.as_ptr() as *const libc::c_void, buf.len(), 0) };
        if n < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    fn recv(&self, buf: &mut [u8]) -> io::Result<usize> {
        // SAFETY: buf is a valid mutable slice.
        let n = unsafe { libc::recv(self.fd.as_raw_fd(), buf.as_mut_ptr() as *mut libc::c_void, buf.len(), 0) };
        if n < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(n as usize)
    }

    fn resolve(&self, family: &str, group: &str) -> io::Result<(u16, u32)> {
        let mut name = family.as_bytes().to_vec();
        name.push(0);
        let attr_len = 4 + name.len();
        let mut body = vec![CTRL_CMD_GETFAMILY, 1, 0, 0];
        body.extend_from_slice(&(attr_len as u16).to_ne_bytes());
        body.extend_from_slice(&CTRL_ATTR_FAMILY_NAME.to_ne_bytes());
        body.extend_from_slice(&name);
        body.resize(GENL_HDRLEN + align4(attr_len), 0);
        let mut msg = Vec::new();
        msg.extend_from_slice(&((NLMSG_HDRLEN + body.len()) as u32).to_ne_bytes());
        msg.extend_from_slice(&GENL_ID_CTRL.to_ne_bytes());
        msg.extend_from_slice(&NLM_F_REQUEST.to_ne_bytes());
        msg.extend_from_slice(&1u32.to_ne_bytes());
        msg.extend_from_slice(&0u32.to_ne_bytes());
        msg.extend_from_slice(&body);
        self.send(&msg)?;

        let mut buf = vec![0u8; 65536];
        let n = self.recv(&mut buf)?;
        let not_found = || io::Error::new(io::ErrorKind::NotFound, "acpi_event netlink family/group not found");
        let (typ, payload) = *messages(&buf[..n]).first().ok_or_else(not_found)?;
        if typ == NLMSG_ERROR || payload.len() < GENL_HDRLEN {
            return Err(not_found());
        }
        let (mut fam, mut grp) = (None, None);
        for (t, v) in attrs(&payload[GENL_HDRLEN..]) {
            if t == CTRL_ATTR_FAMILY_ID && v.len() >= 2 {
                fam = Some(u16::from_ne_bytes([v[0], v[1]]));
            } else if t == CTRL_ATTR_MCAST_GROUPS {
                for (_, g) in attrs(v) {
                    let a = attrs(g);
                    let gname = a.iter().find(|(t, _)| *t == CTRL_ATTR_MCAST_GRP_NAME).map(|(_, v)| cstr(v));
                    let gid = a
                        .iter()
                        .find(|(t, _)| *t == CTRL_ATTR_MCAST_GRP_ID)
                        .and_then(|(_, v)| v.get(..4).map(|b| u32::from_ne_bytes(b.try_into().unwrap())));
                    if gname.as_deref() == Some(group) {
                        grp = gid;
                    }
                }
            }
        }
        Ok((fam.ok_or_else(not_found)?, grp.ok_or_else(not_found)?))
    }

    pub fn set_timeout(&self, t: Option<Duration>) -> io::Result<()> {
        let tv = match t {
            Some(d) => libc::timeval { tv_sec: d.as_secs() as _, tv_usec: d.subsec_micros() as _ },
            None => libc::timeval { tv_sec: 0, tv_usec: 0 },
        };
        // SAFETY: timeval option for SO_RCVTIMEO.
        let r = unsafe {
            libc::setsockopt(
                self.fd.as_raw_fd(),
                libc::SOL_SOCKET,
                libc::SO_RCVTIMEO,
                &tv as *const _ as *const libc::c_void,
                size_of::<libc::timeval>() as u32,
            )
        };
        if r < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    /// Block until the next datagram; returns the ACPI events it carried.
    /// With a timeout set, returns `Ok(vec![])` when it expires.
    pub fn next_events(&self) -> io::Result<Vec<AcpiEvent>> {
        let mut buf = vec![0u8; 65536];
        let n = match self.recv(&mut buf) {
            Ok(n) => n,
            Err(e) if matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut) => return Ok(vec![]),
            Err(e) => return Err(e),
        };
        Ok(messages(&buf[..n])
            .into_iter()
            .filter(|(t, p)| *t == self.family && p.len() >= GENL_HDRLEN)
            .filter_map(|(_, p)| parse_acpi_event(&p[GENL_HDRLEN..]))
            .collect())
    }
}

#[cfg(test)]
pub(crate) fn encode_event(class: &str, bus: &str, kind: u32, data: u32) -> Vec<u8> {
    let mut v = vec![0u8; ACPI_EVENT_SIZE];
    v[..class.len()].copy_from_slice(class.as_bytes());
    v[20..20 + bus.len()].copy_from_slice(bus.as_bytes());
    v[36..40].copy_from_slice(&kind.to_ne_bytes());
    v[40..44].copy_from_slice(&data.to_ne_bytes());
    let mut a = ((4 + v.len()) as u16).to_ne_bytes().to_vec();
    a.extend_from_slice(&ACPI_GENL_ATTR_EVENT.to_ne_bytes());
    a.extend_from_slice(&v);
    a
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_event() {
        let ev = parse_acpi_event(&encode_event("", "ABBC0F6B-8EA1-1", 0xD0, 0)).unwrap();
        assert_eq!(ev, AcpiEvent { device_class: "".into(), bus_id: "ABBC0F6B-8EA1-1".into(), kind: 0xD0, data: 0 });
    }

    /// Needs a real kernel with ACPI: `cargo test -p aoc-core -- --ignored`.
    #[test]
    #[ignore]
    fn live_subscribe() {
        let l = Listener::subscribe().expect("subscribe to acpi_event");
        assert!(l.family > 0);
        l.set_timeout(Some(std::time::Duration::from_millis(200))).unwrap();
        assert!(l.next_events().is_ok(), "timeout must not be an error");
    }

    #[test]
    fn split_messages() {
        let payload = encode_event("ac_adapter", "ACPI0003:00", 0x80, 1);
        let mut msg = ((16 + 4 + payload.len()) as u32).to_ne_bytes().to_vec();
        msg.extend_from_slice(&32u16.to_ne_bytes());
        msg.extend_from_slice(&[0u8; 10]);
        msg.extend_from_slice(&[1, 1, 0, 0]);
        msg.extend_from_slice(&payload);
        let m = messages(&msg);
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].0, 32);
        assert_eq!(parse_acpi_event(&m[0].1[4..]).unwrap().device_class, "ac_adapter");
    }
}
