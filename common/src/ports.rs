//! One-shot TCP connect scan of the well-known ports on hosts found by the
//! ping sweep.

use std::collections::BTreeMap;
use std::net::Ipv4Addr;
use std::ops::RangeInclusive;

pub const WELL_KNOWN: RangeInclusive<u16> = 1..=1023;

#[derive(Debug, Clone, Default)]
pub struct PortScan {
    /// `Some((done, total))` probes while a scan runs.
    pub progress: Option<(u32, u32)>,
    /// Open ports per host from the last (or running) scan.
    pub open: BTreeMap<Ipv4Addr, Vec<u16>>,
    /// Hosts covered by the last finished scan.
    pub scanned: usize,
}

impl PortScan {
    pub fn running(&self) -> bool {
        self.progress.is_some()
    }

    pub fn percent(&self) -> Option<u32> {
        self.progress.map(|(done, total)| if total == 0 { 100 } else { done * 100 / total })
    }

    pub fn begin(&mut self, hosts: usize) {
        self.open.clear();
        self.scanned = hosts;
        self.progress = Some((0, hosts as u32 * WELL_KNOWN.len() as u32));
    }

    pub fn record(&mut self, ip: Ipv4Addr, port: u16, open: bool) {
        if let Some((done, _)) = self.progress.as_mut() {
            *done += 1;
        }
        if open {
            let ports = self.open.entry(ip).or_default();
            ports.push(port);
            ports.sort_unstable();
        }
    }

    pub fn finish(&mut self) {
        self.progress = None;
    }
}

/// `22,80,443` cut to `width` chars, with `+N` for ports that did not fit.
pub fn fmt_ports(ports: &[u16], width: usize) -> String {
    let mut out = String::new();
    for (i, p) in ports.iter().enumerate() {
        let item = if out.is_empty() { p.to_string() } else { format!(",{p}") };
        let rest = ports.len() - i;
        let more = if rest > 1 { format!(" +{}", rest - 1) } else { String::new() };
        if out.len() + item.len() + more.len() > width {
            out.push_str(&format!(" +{rest}"));
            return out;
        }
        out.push_str(&item);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn progress_and_results() {
        let mut s = PortScan::default();
        assert!(!s.running());
        s.begin(2);
        assert_eq!(s.percent(), Some(0));
        let ip = Ipv4Addr::new(10, 0, 0, 2);
        s.record(ip, 443, true);
        s.record(ip, 22, true);
        s.record(ip, 23, false);
        assert_eq!(s.open[&ip], vec![22, 443]);
        assert_eq!(s.progress, Some((3, 2046)));
        s.finish();
        assert!(!s.running());
    }

    #[test]
    fn formats_ports() {
        assert_eq!(fmt_ports(&[], 10), "");
        assert_eq!(fmt_ports(&[22, 80, 443], 20), "22,80,443");
        assert_eq!(fmt_ports(&[22, 80, 443, 445, 548], 12), "22,80,443 +2");
        assert!(fmt_ports(&[1, 2, 3, 4, 5, 6, 7, 8, 9, 10], 12).len() <= 12);
    }
}
