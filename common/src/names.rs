//! MAC -> host name cache, filled from ARP + reverse lookups during sweeps.

use std::collections::HashMap;
use std::net::Ipv4Addr;

use crate::sniff::Mac;

/// Sweeps to wait before asking again about an IP that had no name.
const RETRY_SWEEPS: u32 = 10;

#[derive(Default)]
pub struct HostNames {
    by_mac: HashMap<Mac, String>,
    /// Last IP seen at each MAC (from ARP during sweeps).
    ips: HashMap<Mac, Ipv4Addr>,
    /// Sweep number of the last lookup per IP.
    tried: HashMap<Ipv4Addr, u32>,
}

impl HostNames {
    pub fn get(&self, mac: &Mac) -> Option<&str> {
        self.by_mac.get(mac).map(String::as_str)
    }

    pub fn ip(&self, mac: &Mac) -> Option<Ipv4Addr> {
        self.ips.get(mac).copied()
    }

    pub fn set_ip(&mut self, mac: Mac, ip: Ipv4Addr) {
        self.ips.insert(mac, ip);
    }

    pub fn set(&mut self, mac: Mac, name: String) {
        self.by_mac.insert(mac, name);
    }

    /// Whether `ip` (currently at `mac`) should be looked up in sweep `sweep`;
    /// marks it as tried if so.
    pub fn should_lookup(&mut self, ip: Ipv4Addr, mac: &Mac, sweep: u32) -> bool {
        let due = match self.tried.get(&ip) {
            None => true,
            Some(&last) => !self.by_mac.contains_key(mac) && sweep >= last + RETRY_SWEEPS,
        };
        if due {
            self.tried.insert(ip, sweep);
        }
        due
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn looks_up_once_then_retries_unknown() {
        let mut n = HostNames::default();
        let (ip, mac) = (Ipv4Addr::new(10, 0, 0, 5), Mac([2, 0, 0, 0, 0, 5]));
        assert!(n.should_lookup(ip, &mac, 0));
        assert!(!n.should_lookup(ip, &mac, 1));
        assert!(n.should_lookup(ip, &mac, RETRY_SWEEPS));
        n.set(mac, "nas".into());
        assert!(!n.should_lookup(ip, &mac, 100));
        assert_eq!(n.get(&mac), Some("nas"));
    }
}
