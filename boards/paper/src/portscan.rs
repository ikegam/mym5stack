//! One-shot TCP connect scan of the well-known ports (1-1023) on the hosts
//! that answered the ping sweep. Runs in the background for a few minutes.

use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use m5_common::ports::{PortScan, WELL_KNOWN};

use crate::Event;

/// Parallel connects; lwIP has only a handful of sockets to spare.
const WORKERS: usize = 4;
const CONNECT_TIMEOUT: Duration = Duration::from_millis(250);

#[derive(Default)]
pub struct PortScanner {
    pub state: Arc<Mutex<PortScan>>,
}

impl PortScanner {
    /// Start scanning `hosts` unless a scan is already running.
    pub fn start(&self, hosts: Vec<Ipv4Addr>, events: Sender<Event>) {
        {
            let mut s = self.state.lock().unwrap();
            if s.running() {
                return;
            }
            s.begin(hosts.len());
        }
        log::info!("port scan of {} hosts started", hosts.len());
        let state = self.state.clone();
        std::thread::Builder::new()
            .name("portscan".into())
            .stack_size(4096)
            .spawn(move || run(hosts, state, events))
            .expect("spawn port scan thread");
    }
}

fn run(hosts: Vec<Ipv4Addr>, state: Arc<Mutex<PortScan>>, events: Sender<Event>) {
    let start = Instant::now();
    let ports: Vec<u16> = WELL_KNOWN.collect();
    let total = hosts.len() * ports.len();
    let (hosts, ports) = (Arc::new(hosts), Arc::new(ports));
    let next = Arc::new(AtomicUsize::new(0));
    let workers: Vec<_> = (0..WORKERS)
        .map(|_| {
            let (hosts, ports, next, state) = (hosts.clone(), ports.clone(), next.clone(), state.clone());
            std::thread::Builder::new()
                .stack_size(6144)
                .spawn(move || loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    if i >= total {
                        break;
                    }
                    let (ip, port) = (hosts[i / ports.len()], ports[i % ports.len()]);
                    let open = TcpStream::connect_timeout(&SocketAddr::from((ip, port)), CONNECT_TIMEOUT).is_ok();
                    if open {
                        log::info!("open {ip}:{port}");
                    }
                    state.lock().unwrap().record(ip, port, open);
                })
                .expect("spawn port scan worker")
        })
        .collect();
    for w in workers {
        let _ = w.join();
    }
    state.lock().unwrap().finish();
    log::info!("port scan done in {:?}", start.elapsed());
    let _ = events.send(Event::PortScanDone);
}
