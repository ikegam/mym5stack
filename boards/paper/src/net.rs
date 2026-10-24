//! WiFi station connection plus promiscuous capture of our own BSS.

use std::ffi::{c_void, CString};
use std::sync::{Mutex, OnceLock};
use std::net::{Ipv4Addr, SocketAddrV4, UdpSocket};
use std::time::{Duration, Instant};

use anyhow::{anyhow, Result};
use esp_idf_svc::eventloop::EspSystemEventLoop;
use esp_idf_svc::handle::RawHandle;
use esp_idf_svc::hal::modem::Modem;
use esp_idf_svc::mdns::EspMdns;
use esp_idf_svc::nvs::EspDefaultNvsPartition;
use esp_idf_svc::sys::{self, esp};
use esp_idf_svc::wifi::{AuthMethod, BlockingWifi, ClientConfiguration, Configuration, EspWifi};
use m5_common::dns;
use m5_common::sniff::{Mac, Table};

/// Used for DHCP and mDNS (`m5paper-h.local`).
pub const HOSTNAME: &str = "m5paper-h";
const MAX_HOSTS: usize = 96;
const FCS_LEN: u32 = 4;

pub struct ApInfo {
    pub bssid: Mac,
    pub channel: u8,
    pub rssi: i8,
}

pub fn connect(
    modem: Modem<'static>,
    sysloop: EspSystemEventLoop,
    nvs: EspDefaultNvsPartition,
    ssid: &str,
    pass: &str,
) -> Result<BlockingWifi<EspWifi<'static>>> {
    let mut wifi = BlockingWifi::wrap(EspWifi::new(modem, sysloop.clone(), Some(nvs))?, sysloop)?;
    wifi.set_configuration(&Configuration::Client(ClientConfiguration {
        ssid: ssid.try_into().map_err(|_| anyhow!("SSID too long"))?,
        password: pass.try_into().map_err(|_| anyhow!("password too long"))?,
        auth_method: if pass.is_empty() { AuthMethod::None } else { AuthMethod::WPA2Personal },
        ..Default::default()
    }))?;
    // DHCP option 12; must be set before the DHCP client starts.
    let host = CString::new(HOSTNAME)?;
    esp!(unsafe { sys::esp_netif_set_hostname(wifi.wifi().sta_netif().handle(), host.as_ptr()) })?;
    wifi.start()?;
    log::info!("connecting to {ssid}...");
    wifi.connect()?;
    wifi.wait_netif_up()?;
    // Modem sleep would make the radio miss most frames.
    esp!(unsafe { sys::esp_wifi_set_ps(sys::wifi_ps_type_t_WIFI_PS_NONE) })?;
    Ok(wifi)
}

/// Answer `<HOSTNAME>.local` queries. The returned handle must be kept alive.
pub fn start_mdns() -> Result<EspMdns> {
    let mut mdns = EspMdns::take()?;
    mdns.set_hostname(HOSTNAME)?;
    mdns.set_instance_name("M5Paper LAN monitor")?;
    Ok(mdns)
}

pub fn ap_info() -> Result<ApInfo> {
    let mut rec = sys::wifi_ap_record_t::default();
    esp!(unsafe { sys::esp_wifi_sta_get_ap_info(&mut rec) })?;
    Ok(ApInfo { bssid: Mac(rec.bssid), channel: rec.primary, rssi: rec.rssi })
}

pub fn own_mac() -> Result<Mac> {
    let mut m = [0u8; 6];
    esp!(unsafe { sys::esp_wifi_get_mac(sys::wifi_interface_t_WIFI_IF_STA, m.as_mut_ptr()) })?;
    Ok(Mac(m))
}

struct Capture {
    table: Mutex<Table>,
    epoch: Instant,
}

static CAPTURE: OnceLock<Capture> = OnceLock::new();

pub fn now_ms() -> u64 {
    CAPTURE.get().map(|c| c.epoch.elapsed().as_millis() as u64).unwrap_or(0)
}

/// Run `f` with the traffic table locked. Keep it short: frames captured
/// while the lock is held are dropped rather than stalling the WiFi task.
pub fn with_table<R>(f: impl FnOnce(&mut Table) -> R) -> Option<R> {
    CAPTURE.get().map(|c| f(&mut c.table.lock().unwrap()))
}

unsafe extern "C" fn on_frame(buf: *mut c_void, ty: sys::wifi_promiscuous_pkt_type_t) {
    if ty != sys::wifi_promiscuous_pkt_type_t_WIFI_PKT_DATA {
        return;
    }
    let Some(cap) = CAPTURE.get() else { return };
    let pkt = &*(buf as *const sys::wifi_promiscuous_pkt_t);
    let len = pkt.rx_ctrl.sig_len();
    if len <= FCS_LEN {
        return;
    }
    let len = len - FCS_LEN;
    let rssi = pkt.rx_ctrl.rssi() as i8;
    let frame = core::slice::from_raw_parts(pkt.payload.as_ptr(), len.min(32) as usize);
    let now = cap.epoch.elapsed().as_millis() as u64;
    if let Ok(mut t) = cap.table.try_lock() {
        t.record(frame, len, rssi, now);
    }
}

/// Start counting every data frame of `ap`'s BSS that the radio receives.
pub fn start_capture(ap: &ApInfo, me: Mac) -> Result<()> {
    CAPTURE
        .set(Capture { table: Mutex::new(Table::new(ap.bssid, me, MAX_HOSTS)), epoch: Instant::now() })
        .map_err(|_| anyhow!("capture already started"))?;
    let filter = sys::wifi_promiscuous_filter_t { filter_mask: sys::WIFI_PROMIS_FILTER_MASK_DATA };
    esp!(unsafe { sys::esp_wifi_set_promiscuous_filter(&filter) })?;
    esp!(unsafe { sys::esp_wifi_set_promiscuous_rx_cb(Some(on_frame)) })?;
    esp!(unsafe { sys::esp_wifi_set_promiscuous(true) })?;
    log::info!("capturing BSS {} on ch{}", ap.bssid, ap.channel);
    Ok(())
}

unsafe extern "C" {
    // lwIP (netif/etharp.h); not in the generated bindings.
    fn etharp_find_addr(
        netif: *mut c_void,
        ipaddr: *const u32,
        eth_ret: *mut *const [u8; 6],
        ip_ret: *mut *const u32,
    ) -> i32;
}

struct ArpQuery {
    ip: u32,
    mac: Option<[u8; 6]>,
}

unsafe extern "C" fn arp_find(ctx: *mut c_void) -> sys::esp_err_t {
    let q = &mut *(ctx as *mut ArpQuery);
    let sta = sys::esp_netif_get_handle_from_ifkey(c"WIFI_STA_DEF".as_ptr());
    if sta.is_null() {
        return sys::ESP_FAIL;
    }
    let (mut eth, mut ip_ret) = (core::ptr::null(), core::ptr::null());
    if etharp_find_addr(sys::esp_netif_get_netif_impl(sta), &q.ip, &mut eth, &mut ip_ret) >= 0 && !eth.is_null() {
        q.mac = Some(*eth);
    }
    sys::ESP_OK
}

/// MAC address of `ip` from lwIP's ARP cache (fresh right after a ping reply).
pub fn arp_lookup(ip: Ipv4Addr) -> Option<Mac> {
    let mut q = ArpQuery { ip: u32::from_ne_bytes(ip.octets()), mac: None };
    // The ARP table belongs to the TCP/IP task; query it from there.
    esp!(unsafe { sys::esp_netif_tcpip_exec(Some(arp_find), &mut q as *mut _ as *mut c_void) }).ok()?;
    q.mac.map(Mac)
}

/// Reverse-resolve `ip`: ask the host itself over (legacy unicast) mDNS, then
/// the router's DNS, which usually knows DHCP client names.
pub fn resolve_name(ip: Ipv4Addr, dns: Ipv4Addr) -> Option<String> {
    let sock = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).ok()?;
    sock.set_read_timeout(Some(Duration::from_millis(400))).ok()?;
    let mut buf = [0u8; 512];
    for (i, server) in [SocketAddrV4::new(ip, 5353), SocketAddrV4::new(dns, 53)].into_iter().enumerate() {
        let id = u16::from_be_bytes([ip.octets()[3], i as u8]) ^ (now_ms() as u16);
        if sock.send_to(&dns::ptr_query(id, ip), server).is_err() {
            continue;
        }
        while let Ok((n, _)) = sock.recv_from(&mut buf) {
            if let Some(name) = dns::parse_ptr_response(id, &buf[..n]).and_then(|n| dns::short_name(&n)) {
                return Some(name);
            }
        }
    }
    None
}
