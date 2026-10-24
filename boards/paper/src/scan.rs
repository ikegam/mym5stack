//! Optional active scan: ICMP ping sweep of the local /24, run while enabled.

use std::net::Ipv4Addr;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use esp_idf_svc::ping::{self, EspPing};
use m5_common::active::{sweep_targets, ActiveTable};
use m5_common::names::HostNames;
use m5_common::sniff::Mac;

use crate::{net, Event};

const WORKERS: usize = 8;
const PING_TIMEOUT: Duration = Duration::from_millis(600);
const SWEEP_INTERVAL: Duration = Duration::from_secs(60);

pub struct ActiveScan {
    enabled: Arc<AtomicBool>,
    pub table: Arc<Mutex<ActiveTable>>,
    /// Kept across ON/OFF so names survive a toggle.
    pub names: Arc<Mutex<HostNames>>,
}

impl ActiveScan {
    /// `dns` is asked for reverse (PTR) names of hosts that answer.
    pub fn start(me: Ipv4Addr, prefix: u8, dns: Ipv4Addr, events: Sender<Event>) -> Self {
        let enabled = Arc::new(AtomicBool::new(false));
        let table = Arc::new(Mutex::new(ActiveTable::default()));
        let names = Arc::new(Mutex::new(HostNames::default()));
        let (en, tb, nm) = (enabled.clone(), table.clone(), names.clone());
        std::thread::Builder::new()
            .name("sweep".into())
            .stack_size(8192)
            .spawn(move || run(me, prefix, dns, en, tb, nm, events))
            .expect("spawn sweep thread");
        Self { enabled, table, names }
    }

    pub fn enabled(&self) -> bool {
        self.enabled.load(Ordering::Relaxed)
    }

    pub fn set_enabled(&self, on: bool) {
        self.enabled.store(on, Ordering::Relaxed);
        if !on {
            self.table.lock().unwrap().clear();
        }
        log::info!("active scan {}", if on { "ON" } else { "OFF" });
    }
}

fn ping_once(ip: Ipv4Addr) -> Option<u32> {
    let conf = ping::Configuration { count: 1, timeout: PING_TIMEOUT, ..Default::default() };
    let mut rtt = None;
    let res = EspPing::default().ping_details(ip, &conf, |_summary: &ping::Summary, reply: &ping::Reply| {
        if let ping::Reply::Success(info) = reply {
            rtt = Some(info.elapsed_time.as_millis() as u32);
        }
    });
    if let Err(e) = res {
        log::warn!("ping {ip}: {e}");
    }
    rtt
}

fn run(
    me: Ipv4Addr,
    prefix: u8,
    dns: Ipv4Addr,
    enabled: Arc<AtomicBool>,
    table: Arc<Mutex<ActiveTable>>,
    names: Arc<Mutex<HostNames>>,
    events: Sender<Event>,
) {
    let targets = Arc::new(sweep_targets(me, prefix));
    let epoch = Instant::now();
    let mut sweep: u32 = 0;
    loop {
        while !enabled.load(Ordering::Relaxed) {
            std::thread::sleep(Duration::from_millis(200));
        }
        let start = Instant::now();
        table.lock().unwrap().begin_sweep();
        let next = Arc::new(AtomicUsize::new(0));
        let found: Arc<Mutex<Vec<(Ipv4Addr, Mac)>>> = Arc::default();
        let workers: Vec<_> = (0..WORKERS)
            .map(|_| {
                let (targets, next, table, enabled) = (targets.clone(), next.clone(), table.clone(), enabled.clone());
                let found = found.clone();
                std::thread::Builder::new()
                    .stack_size(6144)
                    .spawn(move || loop {
                        let i = next.fetch_add(1, Ordering::Relaxed);
                        if i >= targets.len() || !enabled.load(Ordering::Relaxed) {
                            break;
                        }
                        let rtt = ping_once(targets[i]);
                        if let Some(mac) = rtt.and_then(|_| net::arp_lookup(targets[i])) {
                            found.lock().unwrap().push((targets[i], mac));
                        }
                        let now = epoch.elapsed().as_millis() as u64;
                        table.lock().unwrap().record(targets[i], rtt, now);
                    })
                    .expect("spawn ping worker")
            })
            .collect();
        for w in workers {
            let _ = w.join();
        }
        if !enabled.load(Ordering::Relaxed) {
            continue;
        }
        {
            let mut n = names.lock().unwrap();
            for &(ip, mac) in found.lock().unwrap().iter() {
                n.set_ip(mac, ip);
            }
        }
        resolve_names(&found.lock().unwrap(), dns, &names, sweep);
        sweep += 1;
        let took = start.elapsed();
        table.lock().unwrap().end_sweep(took.as_millis() as u32);
        log::info!("sweep done in {took:?}");
        let _ = events.send(Event::SweepDone);

        let wait_until = Instant::now() + SWEEP_INTERVAL.saturating_sub(took);
        while Instant::now() < wait_until && enabled.load(Ordering::Relaxed) {
            std::thread::sleep(Duration::from_millis(200));
        }
    }
}

/// Look up names for hosts not tried yet (or unnamed for a while).
fn resolve_names(found: &[(Ipv4Addr, Mac)], dns: Ipv4Addr, names: &Mutex<HostNames>, sweep: u32) {
    for &(ip, mac) in found {
        if !names.lock().unwrap().should_lookup(ip, &mac, sweep) {
            continue;
        }
        match net::resolve_name(ip, dns) {
            Some(name) => {
                log::info!("name {ip} {mac} = {name}");
                names.lock().unwrap().set(mac, name);
            }
            None => log::info!("name {ip} {mac}: unknown"),
        }
    }
}
