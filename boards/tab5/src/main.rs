use esp_idf_svc::hal::delay::FreeRtos;
use esp_idf_svc::sys;
use m5_common::{BoardInfo, Display};

const BOARD: BoardInfo = BoardInfo {
    name: "M5Stack Tab5",
    mcu: "ESP32-P4",
    display: Display { width: 720, height: 1280, kind: "MIPI-DSI LCD" },
};

fn main() -> anyhow::Result<()> {
    sys::link_patches();
    esp_idf_svc::log::EspLogger::initialize_default();

    log::info!("{}", m5_common::banner(&BOARD));
    log_chip_info();

    let mut tick: u32 = 0;
    loop {
        log::info!("alive: tick={tick} free_heap={}", unsafe { sys::esp_get_free_heap_size() });
        tick = tick.wrapping_add(1);
        FreeRtos::delay_ms(1000);
    }
}

fn log_chip_info() {
    let mut info = sys::esp_chip_info_t::default();
    unsafe { sys::esp_chip_info(&mut info) };
    log::info!(
        "chip: model={} cores={} revision=v{}.{}",
        info.model,
        info.cores,
        info.revision / 100,
        info.revision % 100
    );
}
