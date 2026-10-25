//! microSD slot: SDMMC slot 0, 4-bit bus on GPIO39-44, mounted at /sd.
//!
//! The card's IO pins are powered from the P4's internal LDO channel 4
//! (LDO_VO4), which has to be switched on before the bus is usable.

use esp_idf_svc::fs::fatfs::Fatfs;
use esp_idf_svc::hal::gpio::{AnyIOPin, Pins};
use esp_idf_svc::hal::sd::mmc::{self, SdMmcHostDriver, SDMMC0};
use esp_idf_svc::hal::sd::{config, SdCardDriver};
use esp_idf_svc::io::vfs::MountedFatfs;
use esp_idf_svc::sys;

pub type MountedSd = MountedFatfs<Fatfs<SdCardDriver<SdMmcHostDriver<'static>>>>;

const LDO_CHAN: i32 = 4;
const LDO_MV: i32 = 3300;
const SPEED_KHZ: u32 = 40_000;

pub fn mount(slot: SDMMC0<'static>, pins: Pins) -> anyhow::Result<MountedSd> {
    power_on()?;
    let host = SdMmcHostDriver::new_4bits(
        slot,
        pins.gpio44, // CMD
        pins.gpio43, // CLK
        pins.gpio39,
        pins.gpio40,
        pins.gpio41,
        pins.gpio42,
        None::<AnyIOPin>,
        None::<AnyIOPin>,
        &mmc::config::Configuration::new(),
    )?;
    let mut cfg = config::Configuration::new();
    cfg.speed_khz = SPEED_KHZ;
    let card = SdCardDriver::new_mmc(host, &cfg)?;
    let c = card.card();
    let mb = c.csd.capacity as u64 * c.csd.sector_size as u64 / (1024 * 1024);
    log::info!("sd: {mb} MB card at {} kHz", c.real_freq_khz);
    Ok(MountedFatfs::mount(Fatfs::new_sdcard(0, card)?, "/sd", 4)?)
}

fn power_on() -> anyhow::Result<()> {
    let cfg = sys::esp_ldo_channel_config_t { chan_id: LDO_CHAN, voltage_mv: LDO_MV, ..Default::default() };
    let mut handle = core::ptr::null_mut();
    sys::esp!(unsafe { sys::esp_ldo_acquire_channel(&cfg, &mut handle) })?;
    // Never released: the card stays powered for the life of the firmware.
    Ok(())
}
