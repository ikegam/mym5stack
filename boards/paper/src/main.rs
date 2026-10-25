//! M5Paper LAN monitor.
//!
//! Joins the home WiFi, then counts every data frame of that BSS the radio
//! hears (promiscuous mode) and shows per-host traffic on the e-paper.
//! The ACTIVE switch (tap it, or push the side wheel; on at boot) adds a
//! periodic ICMP ping sweep of the local /24. Rolling the wheel (G37 up,
//! G39 down) scrolls the host list. PORTS runs a one-shot connect scan of
//! ports 1-1023 on the hosts the sweep found; the button stays locked
//! (showing progress) until it finishes.

mod epd;
mod net;
mod portscan;
mod scan;
mod touch;

use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::Result;
use embedded_graphics::mono_font::ascii::FONT_10X20;
use embedded_graphics::mono_font::MonoTextStyle;
use embedded_graphics::pixelcolor::{Gray4, GrayColor};
use embedded_graphics::prelude::*;
use embedded_graphics::text::{Alignment, Text};
use esp_idf_svc::eventloop::EspSystemEventLoop;
use esp_idf_svc::hal::adc::attenuation::DB_12;
use esp_idf_svc::hal::adc::oneshot::config::{AdcChannelConfig, Calibration};
use esp_idf_svc::hal::adc::oneshot::{AdcChannelDriver, AdcDriver};
use esp_idf_svc::hal::delay::FreeRtos;
use esp_idf_svc::hal::gpio::{AnyIOPin, Input, PinDriver, Pull};
use esp_idf_svc::hal::i2c::{I2cConfig, I2cDriver};
use esp_idf_svc::hal::peripherals::Peripherals;
use esp_idf_svc::hal::units::FromValueType;
use esp_idf_svc::hal::spi::{self, Dma, SpiDeviceDriver, SpiDriver, SpiDriverConfig};
use esp_idf_svc::nvs::EspDefaultNvsPartition;
use esp_idf_svc::sntp::EspSntp;
use esp_idf_svc::sys;
use m5_common::fb::Framebuffer;
use m5_common::ui;

use epd::{It8951, Mode};
use portscan::PortScanner;
use scan::ActiveScan;

const WIFI_SSID: Option<&str> = option_env!("WIFI_SSID");
const WIFI_PASS: &str = match option_env!("WIFI_PASS") {
    Some(p) => p,
    None => "",
};
/// Local time offset from UTC (JST).
const TZ_OFFSET_S: u64 = 9 * 3600;
const REFRESH: Duration = Duration::from_secs(30);
/// Every Nth periodic refresh uses the flashing GC16 mode to clear ghosting.
const FULL_REFRESH_EVERY: u32 = 10;
/// Button redraw interval while a port scan runs (progress display).
const PROGRESS_EVERY: Duration = Duration::from_secs(5);
/// Host rows moved per wheel click.
const SCROLL_STEP: i32 = 3;
/// Wait this long after the last wheel click before redrawing, so a quick
/// roll becomes one e-paper refresh.
const SCROLL_SETTLE: Duration = Duration::from_millis(300);

#[derive(Clone)]
pub enum Event {
    Tap(i32, i32),
    WheelPush,
    /// Wheel rolled: -1 = up (G37), +1 = down (G39).
    Scroll(i32),
    SweepDone,
    PortScanDone,
}

fn main() -> Result<()> {
    sys::link_patches();
    esp_idf_svc::log::EspLogger::initialize_default();
    // One INFO line per ping step would flood the UART during sweeps.
    unsafe { sys::esp_log_level_set(c"esp_idf_svc::ping".as_ptr(), sys::esp_log_level_t_ESP_LOG_WARN) };

    let p = Peripherals::take()?;
    let sysloop = EspSystemEventLoop::take()?;
    let nvs = EspDefaultNvsPartition::take()?;

    // GPIO2 latches the main power rail (stays on after the button is released
    // on battery); GPIO23 powers the e-paper panel / IT8951.
    let mut main_pwr = PinDriver::output(p.pins.gpio2)?;
    main_pwr.set_high()?;
    let mut epd_pwr = PinDriver::output(p.pins.gpio23)?;
    epd_pwr.set_high()?;
    FreeRtos::delay_ms(100);

    // Battery voltage through a 1:2 divider on GPIO35 (ADC1 ch7).
    let mut bat_adc = AdcChannelDriver::new(
        AdcDriver::new(p.adc1)?,
        p.pins.gpio35,
        &AdcChannelConfig { attenuation: DB_12, calibration: Calibration::Line, ..Default::default() },
    )?;
    let mut battery = move || -> Option<u8> {
        const SAMPLES: u32 = 16;
        let mut sum = 0;
        for _ in 0..SAMPLES {
            sum += bat_adc.read().ok()? as u32;
        }
        let mv = sum / SAMPLES * 2;
        log::info!("battery {mv}mV");
        Some(m5_common::battery::percent(mv))
    };

    let spi_bus = SpiDriver::new(
        p.spi2,
        p.pins.gpio14,
        p.pins.gpio12,
        Some(p.pins.gpio13),
        &SpiDriverConfig::new().dma(Dma::Auto(4096)),
    )?;
    let spi_dev = |mhz: u32| {
        SpiDeviceDriver::new(&spi_bus, Option::<AnyIOPin>::None, &spi::config::Config::new().baudrate(mhz.MHz().into()))
    };
    let mut epd = It8951::new(
        spi_dev(10)?,
        spi_dev(2)?,
        PinDriver::output(p.pins.gpio15)?,
        PinDriver::input(p.pins.gpio27, Pull::Floating)?,
    )?;
    let mut fb = Framebuffer::new(epd.width, epd.height);

    let Some(ssid) = WIFI_SSID.filter(|s| !s.is_empty()) else {
        message(&mut epd, &mut fb, &["WIFI_SSID is not set.", "", "Put WIFI_SSID / WIFI_PASS in .env and reflash."])?;
        loop {
            FreeRtos::delay_ms(60_000);
        }
    };
    message(&mut epd, &mut fb, &["LAN monitor", "", &format!("connecting to {ssid} ...")])?;

    let mut wifi = match net::connect(p.modem, sysloop, nvs, ssid, WIFI_PASS) {
        Ok(w) => w,
        Err(e) => {
            log::error!("wifi: {e:?}");
            message(&mut epd, &mut fb, &[&format!("WiFi connect to {ssid} failed:"), &format!("{e}"), "", "restarting in 30s"])?;
            FreeRtos::delay_ms(30_000);
            unsafe { sys::esp_restart() };
        }
    };
    let ip_info = wifi.wifi().sta_netif().get_ip_info()?;
    log::info!("ip {}/{} gw {}", ip_info.ip, ip_info.subnet.mask.0, ip_info.subnet.gateway);
    let _sntp = EspSntp::new_default()?;
    let _mdns = net::start_mdns().map_err(|e| log::warn!("mdns: {e}")).ok();

    let ap = net::ap_info()?;
    let own_mac = net::own_mac()?;
    net::start_capture(&ap, own_mac)?;

    let (tx, rx) = mpsc::channel();
    let scan = ActiveScan::start(ip_info.ip, ip_info.subnet.mask.0, ip_info.subnet.gateway, tx.clone());
    scan.names.lock().unwrap().set(own_mac, net::HOSTNAME.into());
    scan.set_enabled(true);

    let i2c = I2cDriver::new(p.i2c0, p.pins.gpio21, p.pins.gpio22, &I2cConfig::new().baudrate(400.kHz().into()))?;
    // Side wheel: external pull-ups, active low.
    let buttons = [
        (PinDriver::input(p.pins.gpio37, Pull::Floating)?, Event::Scroll(-1)),
        (PinDriver::input(p.pins.gpio38, Pull::Floating)?, Event::WheelPush),
        (PinDriver::input(p.pins.gpio39, Pull::Floating)?, Event::Scroll(1)),
    ];
    let input_tx = tx.clone();
    std::thread::Builder::new()
        .name("input".into())
        .stack_size(6144)
        .spawn(move || input_loop(i2c, buttons, input_tx))?;

    let ports = PortScanner::default();
    let boot = Instant::now();
    let mut refreshes: u32 = 0;
    let mut next_refresh = Instant::now();
    let mut scroll: usize = 0;
    let mut max_scroll: usize = 0;
    let mut next_progress = Instant::now();
    loop {
        let scanning = ports.state.lock().unwrap().running();
        let wake = if scanning { next_refresh.min(next_progress) } else { next_refresh };
        let mut buttons_changed = false;
        let toggle = match rx.recv_timeout(wake.saturating_duration_since(Instant::now())) {
            Ok(Event::Tap(x, y)) if ui::PORTS_BUTTON.contains(Point::new(x, y)) => {
                if !scanning {
                    let up = scan.table.lock().unwrap().view().hosts.iter().filter(|h| h.1.is_some()).map(|h| h.0).collect();
                    scan.set_paused(true);
                    ports.start(up, tx.clone());
                    buttons_changed = true;
                }
                false
            }
            Ok(Event::Tap(x, y)) => ui::ACTIVE_BUTTON.contains(Point::new(x, y)),
            Ok(Event::WheelPush) => true,
            Ok(Event::Scroll(dir)) => {
                scroll = (scroll as i32 + dir * SCROLL_STEP).clamp(0, max_scroll as i32) as usize;
                next_refresh = Instant::now() + SCROLL_SETTLE;
                false
            }
            Ok(Event::PortScanDone) => {
                scan.set_paused(false);
                next_refresh = Instant::now();
                false
            }
            Ok(Event::SweepDone) => {
                next_refresh = Instant::now();
                false
            }
            Err(RecvTimeoutError::Timeout) => false,
            Err(RecvTimeoutError::Disconnected) => anyhow::bail!("event channel closed"),
        };

        if toggle {
            scan.set_enabled(!scan.enabled());
            next_refresh = Instant::now() + Duration::from_secs(2);
            buttons_changed = true;
        }
        if buttons_changed || (scanning && Instant::now() >= next_progress) {
            // Quick refresh of the buttons only; the full screen follows on its own schedule.
            fb.clear_white();
            ui::draw_buttons(&mut fb, scan.enabled(), &ports.state.lock().unwrap())?;
            let b = ui::BUTTONS;
            epd.update(&fb, b.top_left.x as u32, b.top_left.y as u32, b.size.width, b.size.height, Mode::Du)?;
            next_progress = Instant::now() + PROGRESS_EVERY;
            continue;
        }
        if Instant::now() < next_refresh {
            continue;
        }

        if !wifi.is_connected().unwrap_or(false) {
            log::warn!("wifi disconnected, reconnecting");
            if let Err(e) = wifi.connect() {
                log::warn!("reconnect failed: {e}");
            }
        }

        let now = net::now_ms();
        let (hosts, totals) = net::with_table(|t| (t.snapshot(now), t.totals().clone())).unwrap_or_default();
        let active_view = scan.table.lock().unwrap().view();
        let names = scan.names.lock().unwrap();
        let port_state = ports.state.lock().unwrap().clone();
        max_scroll = ui::max_scroll(hosts.len(), scan.enabled());
        scroll = scroll.min(max_scroll);
        let screen = ui::Screen {
            ssid,
            channel: ap.channel,
            ip: Some(ip_info.ip),
            ap_rssi: net::ap_info().ok().map(|a| a.rssi),
            clock: local_clock(),
            uptime_s: boot.elapsed().as_secs(),
            battery: battery(),
            totals: &totals,
            hosts: &hosts,
            active: scan.enabled(),
            active_view: &active_view,
            scroll,
            names: &names,
            ports: &port_state,
        };
        ui::draw(&mut fb, &screen)?;
        drop(names);
        let mode = if refreshes % FULL_REFRESH_EVERY == 0 { Mode::Gc16 } else { Mode::Gl16 };
        epd.update_full(&fb, mode)?;
        refreshes += 1;
        let (free, block) = unsafe {
            (
                sys::heap_caps_get_free_size(sys::MALLOC_CAP_INTERNAL),
                sys::heap_caps_get_largest_free_block(sys::MALLOC_CAP_INTERNAL),
            )
        };
        log::info!(
            "refresh #{refreshes}: {} hosts, {} frames, internal heap {free} free / {block} largest",
            hosts.len(),
            totals.frames
        );
        next_refresh = Instant::now() + REFRESH;
    }
}

fn input_loop(i2c: I2cDriver<'static>, mut buttons: [(PinDriver<'static, Input>, Event); 3], tx: Sender<Event>) {
    let mut touch = touch::Gt911::new(i2c)
        .map_err(|e| log::warn!("touch disabled: {e}"))
        .ok();
    let mut was_down = [false; 3];
    loop {
        if let Some(t) = touch.as_mut() {
            match t.poll_tap() {
                Ok(Some((x, y))) => {
                    let _ = tx.send(Event::Tap(x, y));
                }
                Ok(None) => {}
                Err(e) => log::warn!("touch: {e}"),
            }
        }
        for ((pin, ev), was) in buttons.iter_mut().zip(was_down.iter_mut()) {
            let down = pin.is_low();
            if down && !*was {
                let _ = tx.send(ev.clone());
            }
            *was = down;
        }
        FreeRtos::delay_ms(30);
    }
}

fn message(epd: &mut It8951, fb: &mut Framebuffer, lines: &[&str]) -> Result<()> {
    fb.clear_white();
    let style = MonoTextStyle::new(&FONT_10X20, Gray4::BLACK);
    let top = fb.height() as i32 / 2 - lines.len() as i32 * 12;
    for (i, l) in lines.iter().enumerate() {
        let pos = Point::new(fb.width() as i32 / 2, top + i as i32 * 24);
        Text::with_alignment(l, pos, style, Alignment::Center).draw(fb)?;
    }
    epd.update_full(fb, Mode::Gc16)
}

fn local_clock() -> Option<(u8, u8, u8)> {
    let secs = SystemTime::now().duration_since(UNIX_EPOCH).ok()?.as_secs();
    if secs < 1_700_000_000 {
        return None; // SNTP not synced yet
    }
    let t = (secs + TZ_OFFSET_S) % 86_400;
    Some(((t / 3600) as u8, (t % 3600 / 60) as u8, (t % 60) as u8))
}
