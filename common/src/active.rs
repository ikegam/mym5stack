//! Results of the optional active (ICMP ping sweep) scan.

use std::collections::BTreeMap;
use std::net::Ipv4Addr;

/// Hosts to probe: the /24 containing `me` (wider subnets are clamped to /24
/// to keep a sweep short), excluding network, broadcast and `me` itself.
pub fn sweep_targets(me: Ipv4Addr, prefix: u8) -> Vec<Ipv4Addr> {
    let prefix = prefix.clamp(24, 30);
    let mask = u32::MAX << (32 - prefix);
    let net = u32::from(me) & mask;
    let bcast = net | !mask;
    (net + 1..bcast).map(Ipv4Addr::from).filter(|&ip| ip != me).collect()
}

#[derive(Debug, Clone, Copy)]
struct Entry {
    rtt_ms: Option<u32>,
    last_up_ms: u64,
}

#[derive(Default)]
pub struct ActiveTable {
    entries: BTreeMap<Ipv4Addr, Entry>,
    sweeps: u32,
    scanning: bool,
    last_sweep_ms: u32,
}

#[derive(Debug, Clone, Default)]
pub struct ActiveView {
    /// Hosts that answered at least once; `None` = no reply in the last sweep.
    pub hosts: Vec<(Ipv4Addr, Option<u32>)>,
    pub up: usize,
    pub sweeps: u32,
    pub scanning: bool,
    pub last_sweep_ms: u32,
}

impl ActiveTable {
    pub fn record(&mut self, ip: Ipv4Addr, rtt_ms: Option<u32>, now_ms: u64) {
        match (self.entries.get_mut(&ip), rtt_ms) {
            (Some(e), rtt) => {
                e.rtt_ms = rtt;
                if rtt.is_some() {
                    e.last_up_ms = now_ms;
                }
            }
            (None, Some(_)) => {
                self.entries.insert(ip, Entry { rtt_ms, last_up_ms: now_ms });
            }
            // Never answered: not worth listing.
            (None, None) => {}
        }
    }

    pub fn begin_sweep(&mut self) {
        self.scanning = true;
    }

    pub fn end_sweep(&mut self, took_ms: u32) {
        self.scanning = false;
        self.sweeps += 1;
        self.last_sweep_ms = took_ms;
    }

    pub fn clear(&mut self) {
        *self = Self::default();
    }

    pub fn view(&self) -> ActiveView {
        let hosts: Vec<_> = self.entries.iter().map(|(ip, e)| (*ip, e.rtt_ms)).collect();
        ActiveView {
            up: hosts.iter().filter(|(_, r)| r.is_some()).count(),
            hosts,
            sweeps: self.sweeps,
            scanning: self.scanning,
            last_sweep_ms: self.last_sweep_ms,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn targets_cover_slash24_without_self() {
        let me = Ipv4Addr::new(192, 168, 1, 23);
        let t = sweep_targets(me, 24);
        assert_eq!(t.len(), 253);
        assert_eq!(t[0], Ipv4Addr::new(192, 168, 1, 1));
        assert_eq!(*t.last().unwrap(), Ipv4Addr::new(192, 168, 1, 254));
        assert!(!t.contains(&me));
        assert_eq!(sweep_targets(me, 16).len(), 253); // clamped
    }

    #[test]
    fn only_lists_hosts_that_answered() {
        let mut a = ActiveTable::default();
        let ip1 = Ipv4Addr::new(10, 0, 0, 1);
        let ip2 = Ipv4Addr::new(10, 0, 0, 2);
        a.record(ip1, Some(3), 0);
        a.record(ip2, None, 0);
        a.record(ip1, None, 10);
        let v = a.view();
        assert_eq!(v.hosts, vec![(ip1, None)]);
        assert_eq!(v.up, 0);
    }
}
