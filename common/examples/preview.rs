//! Render the LAN monitor screen with fake data: `make preview`
use std::net::Ipv4Addr;

use m5_common::active::ActiveTable;
use m5_common::fb::Framebuffer;
use m5_common::names::HostNames;
use m5_common::ports::PortScan;
use m5_common::sniff::{Mac, Table};
use m5_common::ui;

fn main() {
    let ap = Mac([0x10, 0x6f, 0x3f, 0, 0, 1]);
    let me = Mac([0x24, 0x0a, 0xc4, 0, 0, 2]);
    let mut t = Table::new(ap, me, 64);
    for i in 0..18u8 {
        let sta = Mac([if i % 4 == 0 { 0x3a } else { 0x8c }, 0x85, 0x90, 0x12, 0x34, i]);
        let wired = Mac([0x00, 0x1b, 0x21, 0, 0, 0xfe]);
        let n = (18 - i as u32).pow(3);
        for k in 0..n.min(3000) {
            // ToDS frame from station, FromDS reply back to it
            let up = [&[0x88u8, 0x01, 0, 0][..], &ap.0, &sta.0, &wired.0, &[0, 0]].concat();
            let down = [&[0x88u8, 0x02, 0, 0][..], &sta.0, &ap.0, &wired.0, &[0, 0]].concat();
            t.record(&up, 120, -40 - i as i8 * 3, k as u64);
            t.record(&down, 1400, -30, k as u64);
        }
    }
    t.record(&[&[0x88u8, 0x01, 0, 0][..], &ap.0, &me.0, &ap.0, &[0, 0]].concat(), 80, -35, 1000);
    let hosts = t.snapshot(30_000);

    let mut a = ActiveTable::default();
    for last in [1u8, 2, 10, 11, 20, 23, 40, 41, 42, 100, 101, 150, 200, 201, 250] {
        a.record(Ipv4Addr::new(192, 168, 1, last), if last % 7 == 0 { None } else { Some(last as u32 % 37 + 1) }, 0);
    }
    a.end_sweep(7_400);

    let mut names = HostNames::default();
    names.set(Mac([0x8c, 0x85, 0x90, 0x12, 0x34, 1]), "living-room-tv".into());
    names.set(Mac([0x8c, 0x85, 0x90, 0x12, 0x34, 2]), "ikegam-macbook-pro-14".into());
    names.set(me, "m5paper-h".into());
    let mut ports = PortScan::default();
    ports.begin(15);
    for i in 0..18u8 {
        let ip = Ipv4Addr::new(192, 168, 1, 100 + i);
        names.set_ip(Mac([if i % 4 == 0 { 0x3a } else { 0x8c }, 0x85, 0x90, 0x12, 0x34, i]), ip);
        for p in [22u16, 80, 139, 443, 445, 548, 631] {
            ports.record(ip, p, (p as u8 ^ i) % 3 == 0);
        }
    }
    if std::env::var("SCANNING").is_err() {
        ports.finish();
    }

    let mut fb = Framebuffer::new(ui::WIDTH, ui::HEIGHT);
    let screen = ui::Screen {
        ssid: "home-net",
        channel: 6,
        ip: Some(Ipv4Addr::new(192, 168, 1, 23)),
        ap_rssi: Some(-48),
        clock: Some((12, 34, 56)),
        uptime_s: 4_000,
        battery: Some(100),
        totals: t.totals(),
        hosts: &hosts,
        active: std::env::args().any(|a| a == "--active"),
        active_view: &a.view(),
        names: &names,
        ports: &ports,
        scroll: std::env::var("SCROLL").ok().and_then(|v| v.parse().ok()).unwrap_or(0),
    };
    ui::draw(&mut fb, &screen).unwrap();
    let out = std::env::args().nth(1).filter(|a| !a.starts_with("--")).unwrap_or("preview.pgm".into());
    std::fs::write(&out, fb.to_pgm()).unwrap();
    println!("wrote {out}");
}
