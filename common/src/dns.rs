//! Minimal DNS reverse (PTR) lookup packets, used both as unicast DNS to the
//! router and as a legacy unicast mDNS query to the host itself.

use std::net::Ipv4Addr;

const TYPE_PTR: u16 = 12;
const CLASS_IN: u16 = 1;

/// `<d>.<c>.<b>.<a>.in-addr.arpa PTR?` with recursion desired.
pub fn ptr_query(id: u16, ip: Ipv4Addr) -> Vec<u8> {
    let mut q = Vec::with_capacity(48);
    q.extend_from_slice(&id.to_be_bytes());
    q.extend_from_slice(&[0x01, 0x00, 0, 1, 0, 0, 0, 0, 0, 0]); // RD, 1 question
    let o = ip.octets();
    for label in [o[3], o[2], o[1], o[0]].iter().map(|b| b.to_string()).chain(["in-addr".into(), "arpa".into()]) {
        q.push(label.len() as u8);
        q.extend_from_slice(label.as_bytes());
    }
    q.push(0);
    q.extend_from_slice(&TYPE_PTR.to_be_bytes());
    q.extend_from_slice(&CLASS_IN.to_be_bytes());
    q
}

/// The first PTR target in a response to query `id`.
pub fn parse_ptr_response(id: u16, msg: &[u8]) -> Option<String> {
    if msg.len() < 12 || u16::from_be_bytes([msg[0], msg[1]]) != id {
        return None;
    }
    let (flags, qd, an) = (be16(msg, 2)?, be16(msg, 4)?, be16(msg, 6)?);
    if flags & 0x8000 == 0 || flags & 0x000f != 0 {
        return None; // not a response, or an error rcode
    }
    let mut pos = 12;
    for _ in 0..qd {
        pos = skip_name(msg, pos)? + 4;
    }
    for _ in 0..an {
        pos = skip_name(msg, pos)?;
        let (ty, len) = (be16(msg, pos)?, be16(msg, pos + 8)? as usize);
        let rdata = pos + 10;
        if ty == TYPE_PTR {
            return read_name(msg, rdata);
        }
        pos = rdata + len;
    }
    None
}

/// Display form of a resolved name: its first label (`nas.local` -> `nas`).
pub fn short_name(full: &str) -> Option<String> {
    let label = full.split('.').next()?.trim();
    if label.is_empty() {
        return None;
    }
    Some(label.chars().map(|c| if c.is_ascii_graphic() { c } else { '?' }).collect())
}

fn be16(msg: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_be_bytes([*msg.get(at)?, *msg.get(at + 1)?]))
}

fn skip_name(msg: &[u8], mut pos: usize) -> Option<usize> {
    loop {
        let len = *msg.get(pos)?;
        match len {
            0 => return Some(pos + 1),
            l if l & 0xc0 == 0xc0 => return Some(pos + 2),
            l => pos += 1 + l as usize,
        }
    }
}

fn read_name(msg: &[u8], mut pos: usize) -> Option<String> {
    let mut out = String::new();
    for _ in 0..32 {
        let len = *msg.get(pos)?;
        if len == 0 {
            return Some(out);
        }
        if len & 0xc0 == 0xc0 {
            pos = (u16::from_be_bytes([len, *msg.get(pos + 1)?]) & 0x3fff) as usize;
            continue;
        }
        let label = msg.get(pos + 1..pos + 1 + len as usize)?;
        if !out.is_empty() {
            out.push('.');
        }
        out.extend(label.iter().map(|&b| b as char));
        pos += 1 + len as usize;
    }
    None // pointer loop
}

#[cfg(test)]
mod tests {
    use super::*;

    fn response(id: u16, q: &[u8], target: &[u8]) -> Vec<u8> {
        let mut r = q.to_vec();
        r[0..2].copy_from_slice(&id.to_be_bytes());
        r[2..4].copy_from_slice(&[0x81, 0x80]);
        r[6..8].copy_from_slice(&[0, 1]);
        r.extend_from_slice(&[0xc0, 12, 0, 12, 0, 1, 0, 0, 0, 60]); // ptr to qname
        r.extend_from_slice(&(target.len() as u16).to_be_bytes());
        r.extend_from_slice(target);
        r
    }

    #[test]
    fn builds_reverse_query() {
        let q = ptr_query(0x1234, Ipv4Addr::new(192, 168, 0, 7));
        assert_eq!(&q[..2], &[0x12, 0x34]);
        assert_eq!(&q[12..], b"\x017\x010\x03168\x03192\x07in-addr\x04arpa\x00\x00\x0c\x00\x01");
    }

    #[test]
    fn parses_ptr_with_compression() {
        let q = ptr_query(7, Ipv4Addr::new(10, 0, 0, 2));
        let r = response(7, &q, b"\x03nas\x05local\x00");
        assert_eq!(parse_ptr_response(7, &r).as_deref(), Some("nas.local"));
        assert_eq!(parse_ptr_response(8, &r), None);
        let tail = q.windows(5).position(|w| w == b"\x04arpa").unwrap();
        let r = response(7, &q, &[3, b'p', b'c', b'1', 0xc0, tail as u8]);
        assert_eq!(parse_ptr_response(7, &r).as_deref(), Some("pc1.arpa"));
    }

    #[test]
    fn rejects_errors_and_loops() {
        let q = ptr_query(1, Ipv4Addr::new(10, 0, 0, 2));
        let mut r = response(1, &q, b"\x01a\x00");
        r[3] |= 3; // NXDOMAIN
        assert_eq!(parse_ptr_response(1, &r), None);
        let r = response(1, &q, &[0xc0, (q.len() + 12) as u8]); // points at itself
        assert_eq!(parse_ptr_response(1, &r), None);
    }

    #[test]
    fn short_names() {
        assert_eq!(short_name("ikegam-desktop.local").as_deref(), Some("ikegam-desktop"));
        assert_eq!(short_name("tv").as_deref(), Some("tv"));
        assert_eq!(short_name(".local"), None);
    }
}
