//! WiFi through the ESP32-C6 co-processor (esp_hosted over SDIO) and HTTPS GET.

use anyhow::{anyhow, Result};
use esp_idf_svc::eventloop::EspSystemEventLoop;
use esp_idf_svc::hal::modem::Modem;
use esp_idf_svc::http::client::{Configuration as HttpConfig, EspHttpConnection};
use esp_idf_svc::http::Method;
use esp_idf_svc::nvs::EspDefaultNvsPartition;
use esp_idf_svc::sys;
use esp_idf_svc::wifi::{AuthMethod, BlockingWifi, ClientConfiguration, Configuration, EspWifi};

extern "C" {
    fn tab5_wifi_power(on: bool) -> sys::esp_err_t;
}

/// Powers up the ESP32-C6. Call early: it needs time to boot before
/// esp_hosted first talks to it.
pub fn power_on() -> Result<()> {
    sys::esp!(unsafe { tab5_wifi_power(true) })?;
    std::thread::sleep(std::time::Duration::from_millis(1000));
    Ok(())
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
    wifi.start()?;
    log::info!("connecting to {ssid}...");
    wifi.connect()?;
    wifi.wait_netif_up()?;
    Ok(wifi)
}

/// GETs `url` (HTTPS with the built-in CA bundle) and returns the body.
pub fn get(url: &str) -> Result<Vec<u8>> {
    let mut conn = EspHttpConnection::new(&HttpConfig {
        crt_bundle_attach: Some(sys::esp_crt_bundle_attach),
        timeout: Some(std::time::Duration::from_secs(15)),
        ..Default::default()
    })?;
    conn.initiate_request(Method::Get, url, &[])?;
    conn.initiate_response()?;
    let status = conn.status();
    anyhow::ensure!(status == 200, "{url}: HTTP {status}");
    let mut body = Vec::new();
    let mut buf = [0u8; 1024];
    loop {
        let n = conn.read(&mut buf)?;
        if n == 0 {
            break;
        }
        body.extend_from_slice(&buf[..n]);
    }
    Ok(body)
}
