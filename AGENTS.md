# AGENTS.md — notes for AI coding agents (and humans)

Rust `std` firmware on ESP-IDF for M5Stack Tab5 (ESP32-P4) and M5Paper
(ESP32). Everything below was learned the hard way on real hardware; read it
before changing build config, drivers or performance code. It is meant to
save you (and your user's tokens) from re-discovering it.

## Workflow

- `make test` — host unit tests for `common/` (no ESP-IDF). Put logic there.
- `make build BOARD=tab5` — incremental build is ~1 min; a clean build ~4–5 min.
  Avoid `make clean` unless sdkconfig changes are not being picked up.
- `make fwup BOARD=tab5 MONITOR=0` — flash without the interactive monitor
  (the monitor fails with "Failed to initialize input reader" when not on a TTY).
  Read logs with `espflash monitor --non-interactive --chip esp32p4`
  (it resets the board).
- Serial permissions: user must be in `uucp` (Arch). Without re-login,
  run commands through `newgrp uucp`.
- Check UI changes on the host first: `cd common && cargo run --example
  tab5_preview -- out.ppm` renders the Tab5 screen (`--map <region>` for the
  region picker). Convert with ImageMagick and look at it before flashing.
- After changing panel timing or anything visual, ask the user to confirm
  the screen; the log says nothing about a black panel.
- Secrets: WiFi credentials come from `.env` (git-ignored) via `option_env!`.
  Never print or commit them.

## Build system gotchas

- Use the `esp` toolchain (espup) for **both** boards. Upstream nightly's
  `std` for `*-espidf` failed to build (libc `stat` field mismatch).
- esp-idf-sys does not watch files in `extra_components` dirs. The Makefile
  touches `sdkconfig.defaults` when C components are newer, which triggers an
  incremental ESP-IDF rebuild. Without it, C changes are silently ignored.
- Kconfig options marked experimental (e.g. `CONFIG_SPIRAM_SPEED_200M`) are
  **silently dropped** unless `CONFIG_IDF_EXPERIMENTAL_FEATURES=y`. PSRAM fell
  back to 20 MHz with no error. Always confirm speeds in the boot log.
- Tab5 boards ship pre-v3 P4 silicon: `CONFIG_ESP32P4_SELECTS_REV_LESS_V3=y`
  and flash the IDF-built bootloader (the Makefile does this).
- Managed components: pin versions in `[package.metadata.esp-idf-sys]`.
  `esp_wifi_remote 1.1.6` + `esp_hosted 2.0.17` work with IDF v5.5.1;
  older pairs (0.8.x / 1.4.x) fail to compile against it.
- `opt-level = "s"` for the firmware but `opt-level = 3` for `m5-common`
  (hot loops live there) via `[profile.release.package.m5-common]`.

## Runtime gotchas (ESP-IDF + Rust std)

- `std::thread::spawn` stacks come from internal RAM. Spawning a thread per
  operation ran out of memory quickly; use a persistent worker
  (`common/src/llm/helper.rs`). Pin threads with `esp_pthread_set_cfg`
  (`pin_to_core`) before spawning, then restore the default config.
- Long compute loops on the main task trip the task watchdog; yield with
  `FreeRtos::delay_ms(1)` periodically.
- RISC-V32 (P4) has no `AtomicU64`; use `AtomicU32`/`AtomicI32`.
- HTTPS: `crt_bundle_attach: Some(esp_crt_bundle_attach)` in the HTTP client
  config is enough for public sites.

## ESP32-P4 performance

- rustc's LLVM does not accept the PIE SIMD mnemonics (`xespv` features did
  not match). Compile the instruction with IDF's GCC
  (`-march=..._xesppie`), `objdump` it, and emit `.word` encodings with
  fixed registers in `asm!`. See `simd` in `common/src/llm/model.rs`
  (`esp.vld.128.ip` needs 16-byte aligned data — use an `align(16)` type).
- PSRAM at 200 MHz + 256 KB L2 cache with 128-byte lines: ~140–160 MB/s
  reads; int8 matmul ~100 M MAC/s from PSRAM, ~270 M MAC/s from internal RAM.
  Memory bandwidth, not compute, bounds LLM token rate.
- GDMA prefetch into internal RAM reached 148 MB/s but made inference
  *slower*; it was removed. Splitting rows across both cores helps.

## Tab5 hardware

- Display: MIPI-DSI, bring-up in C (`boards/tab5/components/tab5_display`,
  trimmed from Espressif's BSP). Panel variant is detected from the touch
  controller (ILI9881C+GT911, ST7123, or ST7121+ST712x). DPI clock must stay
  at **70 MHz** for ST712x — 40 MHz gave a black screen.
- Draw animations into an off-screen buffer and blit (`display.rs`
  `Offscreen`); drawing straight into the live frame buffer flickers. Cache
  static backgrounds and restore them per frame.
- WiFi goes through the ESP32-C6 over SDIO (esp_hosted):
  - Power the C6 via IO expander 0x44 pin P0, and clear its high-Z register
    (0x07) or the pin never drives.
  - `CONFIG_ESP_HOSTED_SDIO_RESET_ACTIVE_HIGH=y` — hosted 2.x ends its reset
    sequence at the "active" level, so ACTIVE_LOW leaves the C6 held in reset
    (symptom: `send_op_cond` 0x107 and boot loops).
  - SDIO pin options in 2.x are `CONFIG_ESP_HOSTED_PRIV_SDIO_PIN_*_SLOT_1`.
  - Bring WiFi up **before** mounting the microSD card; both use the SDMMC host.
- microSD: enable LDO channel 4 at 3.3 V first; SDMMC0 4-bit
  (CMD 44, CLK 43, D0–D3 39–42).
- Internal I2C bus: port 1, SDA 31 / SCL 32 (touch, IO expanders).
- Tab5 Keyboard (A164, not used yet): I2C 0x6D on Ext.Port1 (SDA G0, SCL G1,
  INT G50); reg 0x10 = 2 selects character mode, then read length at 0x40 and
  characters from 0x50. Reference: m5stack/M5Unit-KEYBOARD.

## M5Paper hardware

- Hold GPIO2 high in `main` or the board powers off on battery.
- IT8951 e-paper: manual CS around each SPI exchange, wait for HRDY between
  words; reads need a second, slower SPI device (MISO samples as 0 at 10 MHz).

## Japanese text and the LLM

- Japanese text: `u8g2-fonts` `u8g2_font_b16_t_japanese3` scaled 2x. It lacks
  some symbols (e.g. ℃ — write 度); `common/examples/ja_glyphs.rs` lists gaps.
- LLM: llama2.c-format models quantized to a custom Q8_0 `.q8`
  (`common/examples/llm.rs quantize`), loaded from microSD into PSRAM.
  `stories15M` runs at ~4–5 tok/s on the P4 with the UI running.
