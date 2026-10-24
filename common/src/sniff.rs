//! Passive per-host traffic accounting from 802.11 data frame headers.
//!
//! Payloads are WPA-encrypted, so only the MAC header (addresses, direction)
//! and the frame length are used. A host is identified by its MAC address.

use core::fmt;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Mac(pub [u8; 6]);

impl Mac {
    pub fn is_multicast(&self) -> bool {
        self.0[0] & 0x01 != 0
    }

    /// Locally administered bit: usually a randomized (private) address.
    pub fn is_local(&self) -> bool {
        self.0[0] & 0x02 != 0
    }
}

impl fmt::Display for Mac {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let m = self.0;
        write!(f, "{:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}", m[0], m[1], m[2], m[3], m[4], m[5])
    }
}

impl fmt::Debug for Mac {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dir {
    /// Station -> AP (ToDS)
    ToAp,
    /// AP -> station (FromDS)
    FromAp,
    /// Neither bit set (direct / IBSS)
    Direct,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DataHeader {
    pub dir: Dir,
    pub bssid: Mac,
    pub src: Mac,
    pub dst: Mac,
}

fn mac_at(b: &[u8], off: usize) -> Mac {
    let mut m = [0u8; 6];
    m.copy_from_slice(&b[off..off + 6]);
    Mac(m)
}

/// Parse the MAC header of an 802.11 data frame. Returns `None` for
/// management/control frames, WDS (4-address) frames and short buffers.
pub fn parse_data_header(frame: &[u8]) -> Option<DataHeader> {
    if frame.len() < 24 {
        return None;
    }
    let ftype = (frame[0] >> 2) & 0x3;
    if ftype != 2 {
        return None;
    }
    let (a1, a2, a3) = (mac_at(frame, 4), mac_at(frame, 10), mac_at(frame, 16));
    let (dir, bssid, src, dst) = match frame[1] & 0x3 {
        0b01 => (Dir::ToAp, a1, a2, a3),
        0b10 => (Dir::FromAp, a2, a3, a1),
        0b00 => (Dir::Direct, a3, a2, a1),
        _ => return None,
    };
    Some(DataHeader { dir, bssid, src, dst })
}

#[derive(Debug, Clone)]
pub struct HostStats {
    pub mac: Mac,
    pub tx_bytes: u64,
    pub rx_bytes: u64,
    pub frames: u32,
    /// Smoothed RSSI of frames this host transmitted over the air.
    pub rssi: Option<i8>,
    /// Seen as the station side of the BSS (vs. only behind the AP).
    pub wireless: bool,
    pub first_seen_ms: u64,
    pub last_seen_ms: u64,
    prev_total: u64,
}

impl HostStats {
    fn new(mac: Mac, now_ms: u64) -> Self {
        Self {
            mac,
            tx_bytes: 0,
            rx_bytes: 0,
            frames: 0,
            rssi: None,
            wireless: false,
            first_seen_ms: now_ms,
            last_seen_ms: now_ms,
            prev_total: 0,
        }
    }

    pub fn total_bytes(&self) -> u64 {
        self.tx_bytes + self.rx_bytes
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Me,
    Ap,
    Wireless,
    Wired,
}

/// One row of a snapshot, ready for display.
#[derive(Debug, Clone)]
pub struct HostView {
    pub mac: Mac,
    pub role: Role,
    pub random_mac: bool,
    pub rssi: Option<i8>,
    pub rate_bps: u32,
    pub tx_bytes: u64,
    pub rx_bytes: u64,
    pub frames: u32,
    pub idle_s: u32,
}

#[derive(Debug, Clone, Default)]
pub struct Totals {
    pub frames: u64,
    pub bytes: u64,
    pub broadcast_frames: u64,
    pub foreign_frames: u64,
}

pub struct Table {
    bssid: Mac,
    me: Mac,
    max_hosts: usize,
    hosts: Vec<HostStats>,
    totals: Totals,
    last_snapshot_ms: u64,
}

impl Table {
    pub fn new(bssid: Mac, me: Mac, max_hosts: usize) -> Self {
        Self {
            bssid,
            me,
            max_hosts,
            hosts: Vec::with_capacity(max_hosts),
            totals: Totals::default(),
            last_snapshot_ms: 0,
        }
    }

    fn host(&mut self, mac: Mac, now_ms: u64) -> &mut HostStats {
        let idx = match self.hosts.iter().position(|h| h.mac == mac) {
            Some(i) => i,
            None => {
                if self.hosts.len() >= self.max_hosts {
                    let oldest = (0..self.hosts.len())
                        .min_by_key(|&i| self.hosts[i].last_seen_ms)
                        .unwrap();
                    self.hosts.swap_remove(oldest);
                }
                self.hosts.push(HostStats::new(mac, now_ms));
                self.hosts.len() - 1
            }
        };
        let h = &mut self.hosts[idx];
        h.last_seen_ms = now_ms;
        h
    }

    /// Account one received frame. `len` is the over-the-air length without FCS.
    pub fn record(&mut self, frame: &[u8], len: u32, rssi: i8, now_ms: u64) {
        let Some(hdr) = parse_data_header(frame) else { return };
        if hdr.bssid != self.bssid {
            self.totals.foreign_frames += 1;
            return;
        }
        self.totals.frames += 1;
        self.totals.bytes += len as u64;

        let src = self.host(hdr.src, now_ms);
        src.tx_bytes += len as u64;
        src.frames += 1;
        if hdr.dir != Dir::FromAp {
            // The source itself put this frame on the air.
            src.wireless = true;
            src.rssi = Some(match src.rssi {
                Some(prev) => ((prev as i16 * 3 + rssi as i16) / 4) as i8,
                None => rssi,
            });
        }

        if hdr.dst.is_multicast() {
            self.totals.broadcast_frames += 1;
        } else {
            let dst = self.host(hdr.dst, now_ms);
            dst.rx_bytes += len as u64;
            if hdr.dir == Dir::FromAp {
                dst.wireless = true;
            }
        }
    }

    pub fn totals(&self) -> &Totals {
        &self.totals
    }

    /// Rows sorted by current rate, then recency. Resets the rate window.
    pub fn snapshot(&mut self, now_ms: u64) -> Vec<HostView> {
        let dt_ms = now_ms.saturating_sub(self.last_snapshot_ms).max(1);
        self.last_snapshot_ms = now_ms;
        let mut rows: Vec<HostView> = self
            .hosts
            .iter_mut()
            .map(|h| {
                let total = h.total_bytes();
                let rate = (total - h.prev_total) * 1000 / dt_ms;
                h.prev_total = total;
                let role = if h.mac == self.me {
                    Role::Me
                } else if h.mac == self.bssid {
                    Role::Ap
                } else if h.wireless {
                    Role::Wireless
                } else {
                    Role::Wired
                };
                HostView {
                    mac: h.mac,
                    role,
                    random_mac: h.mac.is_local(),
                    rssi: h.rssi,
                    rate_bps: rate.min(u32::MAX as u64) as u32,
                    tx_bytes: h.tx_bytes,
                    rx_bytes: h.rx_bytes,
                    frames: h.frames,
                    idle_s: (now_ms.saturating_sub(h.last_seen_ms) / 1000) as u32,
                }
            })
            .collect();
        rows.sort_by(|a, b| b.rate_bps.cmp(&a.rate_bps).then(a.idle_s.cmp(&b.idle_s)));
        rows
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const AP: Mac = Mac([0x10, 0, 0, 0, 0, 1]);
    const STA: Mac = Mac([0x20, 0, 0, 0, 0, 2]);
    const WIRED: Mac = Mac([0x30, 0, 0, 0, 0, 3]);
    const ME: Mac = Mac([0x40, 0, 0, 0, 0, 4]);

    fn frame(flags: u8, a1: Mac, a2: Mac, a3: Mac) -> Vec<u8> {
        let mut f = vec![0x08, flags, 0, 0];
        f.extend_from_slice(&a1.0);
        f.extend_from_slice(&a2.0);
        f.extend_from_slice(&a3.0);
        f.extend_from_slice(&[0, 0]);
        f
    }

    #[test]
    fn parses_directions() {
        let up = parse_data_header(&frame(0x01, AP, STA, WIRED)).unwrap();
        assert_eq!((up.dir, up.bssid, up.src, up.dst), (Dir::ToAp, AP, STA, WIRED));
        let down = parse_data_header(&frame(0x02, STA, AP, WIRED)).unwrap();
        assert_eq!((down.dir, down.bssid, down.src, down.dst), (Dir::FromAp, AP, WIRED, STA));
        assert!(parse_data_header(&frame(0x03, AP, STA, WIRED)).is_none());
        let mut mgmt = frame(0x00, AP, STA, AP);
        mgmt[0] = 0x80; // beacon
        assert!(parse_data_header(&mgmt).is_none());
    }

    #[test]
    fn accounts_tx_rx_and_roles() {
        let mut t = Table::new(AP, ME, 8);
        t.record(&frame(0x01, AP, STA, WIRED), 100, -40, 0);
        t.record(&frame(0x02, STA, AP, WIRED), 1000, -30, 500);
        let rows = t.snapshot(1000);
        let sta = rows.iter().find(|r| r.mac == STA).unwrap();
        let wired = rows.iter().find(|r| r.mac == WIRED).unwrap();
        assert_eq!((sta.tx_bytes, sta.rx_bytes, sta.role, sta.rssi), (100, 1000, Role::Wireless, Some(-40)));
        assert_eq!((wired.tx_bytes, wired.rx_bytes, wired.role, wired.rssi), (1000, 100, Role::Wired, None));
        assert_eq!(sta.rate_bps, 1100);
        // Next window with no traffic: rate drops to zero.
        assert_eq!(t.snapshot(2000)[0].rate_bps, 0);
    }

    #[test]
    fn ignores_other_bss_and_counts_broadcast() {
        let mut t = Table::new(AP, ME, 8);
        let other = Mac([0x50, 0, 0, 0, 0, 5]);
        t.record(&frame(0x01, other, STA, WIRED), 100, -40, 0);
        t.record(&frame(0x02, Mac([0xff; 6]), AP, WIRED), 60, -30, 0);
        assert_eq!(t.totals().foreign_frames, 1);
        assert_eq!(t.totals().broadcast_frames, 1);
        assert_eq!(t.snapshot(1).len(), 1); // only WIRED, no broadcast entry
    }

    #[test]
    fn evicts_least_recently_seen() {
        let mut t = Table::new(AP, ME, 2);
        for (i, now) in [(1u8, 0u64), (2, 10), (3, 20)] {
            t.record(&frame(0x01, AP, Mac([0x20, 0, 0, 0, 0, i]), Mac([0xff; 6])), 10, -50, now);
        }
        let macs: Vec<u8> = t.snapshot(30).iter().map(|r| r.mac.0[5]).collect();
        assert!(!macs.contains(&1) && macs.len() == 2);
    }
}
