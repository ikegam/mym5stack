//! LAN monitor screen for a 960x540 4-bit grayscale e-paper (M5Paper, landscape).

use std::fmt::Write as _;
use std::net::Ipv4Addr;

use embedded_graphics::mono_font::ascii::FONT_10X20;
use embedded_graphics::mono_font::MonoTextStyle;
use embedded_graphics::pixelcolor::{Gray4, GrayColor};
use embedded_graphics::prelude::*;
use embedded_graphics::primitives::{Line, PrimitiveStyle, PrimitiveStyleBuilder, Rectangle};
use embedded_graphics::text::{Alignment, Baseline, Text, TextStyleBuilder};

use crate::active::ActiveView;
use crate::names::HostNames;
use crate::ports::{fmt_ports, PortScan};
use crate::sniff::{HostView, Role, Totals};

pub const WIDTH: u32 = 960;
pub const HEIGHT: u32 = 540;

const MARGIN: i32 = 10;
const ROW_H: i32 = 22;
const CHAR_W: i32 = 10;
const TABLE_TOP: i32 = 64;
const ACTIVE_ROWS: i32 = 1;
const ACTIVE_COLS: i32 = 8;
// Host table columns, in characters.
const COL_HOST: i32 = 0;
const COL_ROLE: i32 = 26;
const COL_IP: i32 = 32;
const COL_PORTS: i32 = 48;
const COL_TX: i32 = 68;
/// Header width taken by the battery gauge (icon + "100%").
const BATTERY_W: i32 = 96;

/// Top-right button pair. x and width are multiples of 4 so the area can
/// be refreshed on its own on the IT8951.
pub const BUTTONS: Rectangle = Rectangle::new(Point::new(800, 8), Size::new(152, 48));
/// ACTIVE ping sweep on/off switch (left half).
pub const ACTIVE_BUTTON: Rectangle = Rectangle::new(Point::new(800, 8), Size::new(72, 48));
/// One-shot port scan (right half); locked while a scan runs.
pub const PORTS_BUTTON: Rectangle = Rectangle::new(Point::new(880, 8), Size::new(72, 48));

const GRAY: Gray4 = Gray4::new(0x8);
const LIGHT: Gray4 = Gray4::new(0xc);

pub struct Screen<'a> {
    pub ssid: &'a str,
    pub channel: u8,
    pub ip: Option<Ipv4Addr>,
    pub ap_rssi: Option<i8>,
    /// Local wall-clock time, once SNTP has synced.
    pub clock: Option<(u8, u8, u8)>,
    pub uptime_s: u64,
    /// Battery charge in percent, if it could be measured.
    pub battery: Option<u8>,
    pub totals: &'a Totals,
    pub hosts: &'a [HostView],
    pub active: bool,
    pub active_view: &'a ActiveView,
    /// Index of the first host row shown (clamped when drawing).
    pub scroll: usize,
    /// Host names to show instead of MAC addresses where known.
    pub names: &'a HostNames,
    pub ports: &'a PortScan,
}

fn table_bottom(active: bool) -> i32 {
    if active { HEIGHT as i32 - (ACTIVE_ROWS + 1) * ROW_H - 12 } else { HEIGHT as i32 - 4 }
}

/// Largest useful `Screen::scroll` for `hosts` rows.
pub fn max_scroll(hosts: usize, active: bool) -> usize {
    let max_rows = ((table_bottom(active) - TABLE_TOP - ROW_H - 4) / ROW_H).max(0) as usize;
    if hosts > max_rows { hosts - max_rows.saturating_sub(1) } else { 0 }
}

fn text<D: DrawTarget<Color = Gray4>>(d: &mut D, s: &str, x: i32, y: i32, c: Gray4) -> Result<(), D::Error> {
    Text::with_baseline(s, Point::new(x, y), MonoTextStyle::new(&FONT_10X20, c), Baseline::Top).draw(d)?;
    Ok(())
}

fn hline<D: DrawTarget<Color = Gray4>>(d: &mut D, y: i32, c: Gray4) -> Result<(), D::Error> {
    Line::new(Point::new(MARGIN, y), Point::new(WIDTH as i32 - MARGIN, y))
        .into_styled(PrimitiveStyle::with_stroke(c, 1))
        .draw(d)
}

pub fn fmt_bytes(b: u64) -> String {
    match b {
        0..=999 => format!("{b}"),
        1_000..=999_999 => format!("{:.1}k", b as f64 / 1e3),
        1_000_000..=999_999_999 => format!("{:.1}M", b as f64 / 1e6),
        _ => format!("{:.2}G", b as f64 / 1e9),
    }
}

pub fn fmt_duration(s: u64) -> String {
    match s {
        0..=59 => format!("{s}s"),
        60..=3599 => format!("{}m", s / 60),
        3600..=86_399 => format!("{}h{:02}m", s / 3600, s % 3600 / 60),
        _ => format!("{}d{}h", s / 86_400, s % 86_400 / 3600),
    }
}

fn button<D: DrawTarget<Color = Gray4>>(d: &mut D, r: Rectangle, label: &str, on: bool) -> Result<(), D::Error> {
    let (fill, fg) = if on { (Gray4::BLACK, Gray4::WHITE) } else { (Gray4::WHITE, Gray4::BLACK) };
    r.into_styled(PrimitiveStyleBuilder::new().fill_color(fill).stroke_color(Gray4::BLACK).stroke_width(3).build())
        .draw(d)?;
    Text::with_text_style(
        label,
        r.center(),
        MonoTextStyle::new(&FONT_10X20, fg),
        TextStyleBuilder::new().alignment(Alignment::Center).baseline(Baseline::Middle).build(),
    )
    .draw(d)?;
    Ok(())
}

/// ACTIVE (inverted while on) and PORTS (inverted with progress while running).
pub fn draw_buttons<D: DrawTarget<Color = Gray4>>(d: &mut D, active: bool, ports: &PortScan) -> Result<(), D::Error> {
    button(d, ACTIVE_BUTTON, "ACTIVE", active)?;
    match ports.percent() {
        Some(p) => button(d, PORTS_BUTTON, &format!("{p}%"), true),
        None => button(d, PORTS_BUTTON, "PORTS", false),
    }
}

/// Battery icon with fill level and percentage, top-left of the header.
fn draw_battery<D: DrawTarget<Color = Gray4>>(d: &mut D, pct: Option<u8>) -> Result<(), D::Error> {
    let (x, y, w, h) = (MARGIN, 11, 26, 14);
    Rectangle::new(Point::new(x, y), Size::new(w, h))
        .into_styled(PrimitiveStyle::with_stroke(Gray4::BLACK, 2))
        .draw(d)?;
    Rectangle::new(Point::new(x + w as i32, y + 4), Size::new(3, 6))
        .into_styled(PrimitiveStyle::with_fill(Gray4::BLACK))
        .draw(d)?;
    let label = match pct {
        Some(p) => {
            let fill = (w - 6) * p.min(100) as u32 / 100;
            if fill > 0 {
                Rectangle::new(Point::new(x + 3, y + 3), Size::new(fill, h - 6))
                    .into_styled(PrimitiveStyle::with_fill(Gray4::BLACK))
                    .draw(d)?;
            }
            format!("{p}%")
        }
        None => "--%".into(),
    };
    text(d, &label, x + w as i32 + 7, 8, Gray4::BLACK)
}

/// Name if known (cut to its column), else the MAC address.
fn host_label(names: &HostNames, h: &HostView) -> String {
    match names.get(&h.mac) {
        Some(n) => n.chars().take((COL_ROLE - COL_HOST - 1) as usize).collect(),
        None => h.mac.to_string(),
    }
}

fn role_label(h: &HostView) -> String {
    let r = match h.role {
        Role::Me => "me",
        Role::Ap => "AP",
        Role::Wireless => "wifi",
        Role::Wired => "lan",
    };
    format!("{r}{}", if h.random_mac { "*" } else { "" })
}

pub fn draw<D: DrawTarget<Color = Gray4>>(d: &mut D, s: &Screen) -> Result<(), D::Error> {
    d.clear(Gray4::WHITE)?;

    // Header
    draw_battery(d, s.battery)?;
    let mut l1 = format!("LAN monitor  {} ch{}", s.ssid, s.channel);
    if let Some(ip) = s.ip {
        let _ = write!(l1, "  {ip}");
    }
    if let Some(r) = s.ap_rssi {
        let _ = write!(l1, "  AP {r}dBm");
    }
    text(d, &l1, MARGIN + BATTERY_W, 8, Gray4::BLACK)?;

    let recent = s.hosts.iter().filter(|h| h.idle_s < 300).count();
    let mut l2 = match s.clock {
        Some((h, m, sec)) => format!("{h:02}:{m:02}:{sec:02}"),
        None => "--:--:--".into(),
    };
    let _ = write!(
        l2,
        "  up {}  hosts {} (5m:{})  {} frm  bc {}  other {}",
        fmt_duration(s.uptime_s),
        s.hosts.len(),
        recent,
        fmt_bytes(s.totals.frames),
        fmt_bytes(s.totals.broadcast_frames),
        fmt_bytes(s.totals.foreign_frames),
    );
    text(d, &l2, MARGIN, 32, GRAY)?;
    draw_buttons(d, s.active, s.ports)?;

    // Passive table
    let cols: [(&str, i32); 8] = [
        ("HOST", COL_HOST),
        ("ROLE", COL_ROLE),
        ("IP", COL_IP),
        ("PORTS", COL_PORTS),
        ("TX", COL_TX),
        ("RX", COL_TX + 7),
        ("FRAMES", COL_TX + 14),
        ("IDLE", COL_TX + 21),
    ];
    for (name, c) in cols {
        text(d, name, MARGIN + c * CHAR_W, TABLE_TOP, GRAY)?;
    }
    hline(d, TABLE_TOP + ROW_H, Gray4::BLACK)?;

    let table_bottom = table_bottom(s.active);
    let first_row = TABLE_TOP + ROW_H + 4;
    let max_rows = ((table_bottom - first_row) / ROW_H).max(0) as usize;

    // When the list overflows, the last row becomes a position indicator.
    let overflow = s.hosts.len() > max_rows;
    let page = if overflow { max_rows.saturating_sub(1) } else { max_rows };
    let start = s.scroll.min(s.hosts.len().saturating_sub(page));
    let shown = page.min(s.hosts.len() - start);
    for (i, h) in s.hosts.iter().skip(start).take(shown).enumerate() {
        let y = first_row + i as i32 * ROW_H;
        let fg = if h.idle_s >= 300 { GRAY } else { Gray4::BLACK };
        if i % 2 == 1 {
            Rectangle::new(Point::new(MARGIN, y - 1), Size::new(WIDTH - 2 * MARGIN as u32, ROW_H as u32))
                .into_styled(PrimitiveStyle::with_fill(Gray4::new(0xe)))
                .draw(d)?;
        }
        let ip = s.names.ip(&h.mac);
        let ports = ip.and_then(|ip| s.ports.open.get(&ip)).map(|p| fmt_ports(p, (COL_TX - COL_PORTS - 1) as usize));
        let cells = [
            (host_label(s.names, h), COL_HOST),
            (role_label(h), COL_ROLE),
            (ip.map(|ip| ip.to_string()).unwrap_or_default(), COL_IP),
            (ports.unwrap_or_default(), COL_PORTS),
            (fmt_bytes(h.tx_bytes), COL_TX),
            (fmt_bytes(h.rx_bytes), COL_TX + 7),
            (fmt_bytes(h.frames as u64), COL_TX + 14),
            (fmt_duration(h.idle_s as u64), COL_TX + 21),
        ];
        for (txt, c) in cells {
            text(d, &txt, MARGIN + c * CHAR_W, y, fg)?;
        }
    }
    if overflow {
        let y = first_row + shown as i32 * ROW_H;
        let (above, below) = (start, s.hosts.len() - start - shown);
        let msg = format!(
            "{}-{} of {}   {} above / {} below   wheel to scroll",
            start + 1,
            start + shown,
            s.hosts.len(),
            above,
            below
        );
        text(d, &msg, MARGIN, y, GRAY)?;
    }
    if s.hosts.is_empty() {
        text(d, "listening for traffic...", MARGIN, first_row, GRAY)?;
    }

    if s.active {
        draw_active_panel(d, s, table_bottom + 4)?;
    }
    Ok(())
}

fn draw_active_panel<D: DrawTarget<Color = Gray4>>(d: &mut D, s: &Screen, top: i32) -> Result<(), D::Error> {
    let v = s.active_view;
    hline(d, top, Gray4::BLACK)?;
    let mut title = String::from("ACTIVE ping sweep");
    if let Some(ip) = s.ip {
        let o = ip.octets();
        let _ = write!(title, " {}.{}.{}.0/24", o[0], o[1], o[2]);
    }
    let _ = write!(title, "  up {}/{}", v.up, v.hosts.len());
    if v.sweeps > 0 {
        let _ = write!(title, "  sweep #{} ({:.1}s)", v.sweeps, v.last_sweep_ms as f32 / 1000.0);
    }
    if v.scanning {
        title.push_str("  scanning...");
    }
    text(d, &title, MARGIN, top + 4, Gray4::BLACK)?;

    let cell_w = (WIDTH as i32 - 2 * MARGIN) / ACTIVE_COLS;
    let cap = (ACTIVE_ROWS * ACTIVE_COLS) as usize;
    for (i, (ip, rtt)) in v.hosts.iter().take(cap).enumerate() {
        let (r, c) = (i as i32 / ACTIVE_COLS, i as i32 % ACTIVE_COLS);
        let x = MARGIN + c * cell_w;
        let y = top + 4 + ROW_H * (r + 1);
        let last = ip.octets()[3];
        let (cell, fg) = match rtt {
            Some(ms) => (format!(".{last:<3}{:>4}ms", (*ms).min(9999)), Gray4::BLACK),
            None => (format!(".{last:<3}  down"), LIGHT),
        };
        text(d, &cell, x, y, fg)?;
    }
    if v.hosts.len() > cap {
        text(d, &format!("+{}", v.hosts.len() - cap), WIDTH as i32 - MARGIN - 5 * CHAR_W, top + 4, GRAY)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scroll_limits() {
        assert_eq!(max_scroll(3, false), 0);
        let rows = ((table_bottom(false) - TABLE_TOP - ROW_H - 4) / ROW_H) as usize;
        assert_eq!(max_scroll(rows, false), 0);
        assert_eq!(max_scroll(rows + 10, false), 11);
        assert!(max_scroll(40, true) > max_scroll(40, false));
    }

    #[test]
    fn formats() {
        assert_eq!(fmt_bytes(999), "999");
        assert_eq!(fmt_bytes(1_500), "1.5k");
        assert_eq!(fmt_bytes(2_340_000), "2.3M");
        assert_eq!(fmt_duration(59), "59s");
        assert_eq!(fmt_duration(3_725), "1h02m");
    }

    #[test]
    fn buttons_are_word_aligned() {
        for b in [BUTTONS, ACTIVE_BUTTON, PORTS_BUTTON] {
            assert_eq!(b.top_left.x % 4, 0);
            assert_eq!(b.size.width % 4, 0);
            assert!(BUTTONS.contains(b.top_left) && BUTTONS.contains(b.bottom_right().unwrap()));
        }
    }
}
