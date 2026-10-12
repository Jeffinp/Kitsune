//! DNS messages: name normalisation, building a query and parsing a response (A and CNAME records).

use super::*;

/// Normalize a host name for the wire and the cache: one trailing dot dropped,
/// ASCII letters lowercased. `None` if it is not a valid host name: empty
/// labels, labels over 63 bytes, more than 253 bytes, or characters other than
/// letters, digits, `-` and `_` (internationalized names must arrive already as
/// punycode).
pub fn normalize_name(name: &str) -> Option<String> {
    let n = name.strip_suffix('.').unwrap_or(name);
    if n.is_empty() || n.len() > MAX_NAME {
        return None;
    }
    for label in n.split('.') {
        if label.is_empty() || label.len() > MAX_LABEL {
            return None;
        }
        if !label
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        {
            return None;
        }
    }
    Some(n.to_ascii_lowercase())
}

/// Build an `A IN` query for `name` with transaction id `id` into `out`.
/// Returns the message length, or `None` for an invalid name or a buffer that is
/// too small (`HDR + name + 6` bytes are needed).
pub fn build_query(out: &mut [u8], id: u16, name: &str) -> Option<usize> {
    let name = normalize_name(name)?;
    let need = HDR + name.len() + 2 + 4;
    if out.len() < need {
        return None;
    }
    out[0..2].copy_from_slice(&id.to_be_bytes());
    out[2..4].copy_from_slice(&0x0100u16.to_be_bytes()); // RD
    out[4..6].copy_from_slice(&1u16.to_be_bytes()); // QDCOUNT
    out[6..HDR].fill(0);
    let mut o = HDR;
    for label in name.split('.') {
        out[o] = label.len() as u8;
        out[o + 1..o + 1 + label.len()].copy_from_slice(label.as_bytes());
        o += 1 + label.len();
    }
    out[o] = 0;
    o += 1;
    out[o..o + 2].copy_from_slice(&TYPE_A.to_be_bytes());
    out[o + 2..o + 4].copy_from_slice(&CLASS_IN.to_be_bytes());
    Some(o + 4)
}

/// Addresses of an answer (at most [`MAX_ADDRS`]).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Addrs {
    pub(super) list: [Ipv4; MAX_ADDRS],
    pub(super) len: u8,
}

impl Addrs {
    pub fn as_slice(&self) -> &[Ipv4] {
        &self.list[..self.len as usize]
    }
    pub fn first(&self) -> Option<Ipv4> {
        self.as_slice().first().copied()
    }
}

/// What a well-formed, matching response says.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Reply {
    /// One or more A records; `ttl` is the smallest TTL of the records used.
    Addrs { addrs: Addrs, ttl: u32 },
    /// The name does not exist (rcode 3): authoritative, do not ask others.
    NxDomain,
    /// The name exists but has no A record (NOERROR, empty answer).
    NoData,
    /// The server could not answer (SERVFAIL, REFUSED, truncated ... ); another
    /// server may.
    ServerFail(u8),
}

/// Decode the name at `off`, following compression pointers. Returns the
/// lowercase dotted name and the offset just past the name *in the original
/// position* (a pointer ends it after two bytes).
pub(super) fn read_name(msg: &[u8], off: usize) -> Option<(String, usize)> {
    let mut name = String::new();
    let mut pos = off;
    let mut end = None;
    let mut jumps = 0;
    loop {
        let len = *msg.get(pos)? as usize;
        if len == 0 {
            end.get_or_insert(pos + 1);
            break;
        }
        if len & 0xC0 == 0xC0 {
            let lo = *msg.get(pos + 1)? as usize;
            let target = (len & 0x3F) << 8 | lo;
            end.get_or_insert(pos + 2);
            // Strictly backwards: a pointer to itself or forward could loop.
            if target >= pos {
                return None;
            }
            jumps += 1;
            if jumps > MAX_JUMPS {
                return None;
            }
            pos = target;
            continue;
        }
        if len & 0xC0 != 0 {
            return None; // reserved label types
        }
        let label = msg.get(pos + 1..pos + 1 + len)?;
        if !name.is_empty() {
            name.push('.');
        }
        for &b in label {
            name.push(b.to_ascii_lowercase() as char);
        }
        if name.len() > MAX_NAME {
            return None;
        }
        pos += 1 + len;
    }
    Some((name, end?))
}

pub(super) fn be16(m: &[u8], o: usize) -> Option<u16> {
    Some(u16::from_be_bytes([*m.get(o)?, *m.get(o + 1)?]))
}

/// Parse a response to the query `(id, name)`. `None` means "not an answer to
/// this query" (wrong id, a query instead of a response, different question,
/// malformed): the caller keeps waiting. Everything else is a [`Reply`].
pub fn parse_response(msg: &[u8], id: u16, name: &str) -> Option<Reply> {
    let qname = normalize_name(name)?;
    if msg.len() < HDR || be16(msg, 0)? != id {
        return None;
    }
    let flags = be16(msg, 2)?;
    if flags & 0x8000 == 0 {
        return None; // a query, not a response
    }
    let qd = be16(msg, 4)? as usize;
    let an = be16(msg, 6)? as usize;
    if qd != 1 {
        return None;
    }
    // The question must be the one we asked.
    let (qn, mut off) = read_name(msg, HDR)?;
    if qn != qname || be16(msg, off)? != TYPE_A || be16(msg, off + 2)? != CLASS_IN {
        return None;
    }
    off += 4;

    let rcode = (flags & 0x000F) as u8;
    if rcode == 3 {
        return Some(Reply::NxDomain);
    }

    // Answer section: A records owned by the name or by a CNAME target of it.
    let mut owners: Vec<String> = alloc::vec![qname];
    let mut cnames = 0;
    let mut addrs = Addrs {
        list: [Ipv4([0; 4]); MAX_ADDRS],
        len: 0,
    };
    let mut ttl = u32::MAX;
    for _ in 0..an.min(64) {
        let Some((owner, o)) = read_name(msg, off) else {
            break;
        };
        let (Some(rtype), Some(class), Some(rttl), Some(rdlen)) = (
            be16(msg, o),
            be16(msg, o + 2),
            be16(msg, o + 4).zip(be16(msg, o + 6)),
            be16(msg, o + 8),
        ) else {
            break;
        };
        let rttl = (u32::from(rttl.0) << 16) | u32::from(rttl.1);
        let rd = o + 10;
        let rdlen = rdlen as usize;
        let Some(data) = msg.get(rd..rd + rdlen) else {
            break;
        };
        off = rd + rdlen;
        if class != CLASS_IN || !owners.contains(&owner) {
            continue;
        }
        match rtype {
            TYPE_A if rdlen == 4 => {
                if (addrs.len as usize) < MAX_ADDRS {
                    addrs.list[addrs.len as usize] = Ipv4([data[0], data[1], data[2], data[3]]);
                    addrs.len += 1;
                }
                ttl = ttl.min(rttl);
            }
            TYPE_CNAME if cnames < MAX_CNAMES => {
                if let Some((target, _)) = read_name(msg, rd) {
                    cnames += 1;
                    ttl = ttl.min(rttl);
                    owners.push(target);
                }
            }
            _ => {}
        }
    }
    if addrs.len > 0 {
        return Some(Reply::Addrs { addrs, ttl });
    }
    if rcode != 0 {
        return Some(Reply::ServerFail(rcode));
    }
    if flags & 0x0200 != 0 {
        return Some(Reply::ServerFail(0)); // truncated, and no TCP fallback here
    }
    Some(Reply::NoData)
}
