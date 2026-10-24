# M5Stack Tab5 / M5Paper firmware (Rust)

Rust (std, ESP-IDF v5.5 via `esp-idf-svc`) firmware for two boards:

| BOARD   | Device        | MCU                  | Rust target                | Toolchain          |
|---------|---------------|----------------------|----------------------------|--------------------|
| `tab5`  | M5Stack Tab5  | ESP32-P4 (RISC-V)    | `riscv32imafc-esp-espidf`  | `nightly`          |
| `paper` | M5Paper       | ESP32 (Xtensa)       | `xtensa-esp32-espidf`      | `esp` (from espup) |

## Layout

```
Makefile            build / flash entry point
partitions.csv      shared 16MB partition table (6MB app + 9.9MB spiffs)
common/             board-independent logic (host-testable, no ESP-IDF deps)
boards/tab5/        Tab5 binary crate   (.cargo/config.toml, sdkconfig.defaults)
boards/paper/       M5Paper binary crate
```

Each board is its own cargo project because the two targets need different
toolchains. Shared code goes in `common/`.

## Setup (once)

```sh
make setup
```

Installs `espup`, `espflash`, `ldproxy`, the Xtensa `esp` toolchain and
`nightly` + `rust-src`. The first `make build` per board also downloads
ESP-IDF into `~/.espressif` (several minutes, shared by both boards).

Serial access: `sudo usermod -aG uucp $USER` (Arch) and log in again.

## Flash

```sh
make fwup BOARD=tab5                      # build + flash + monitor
make fwup BOARD=paper PORT=/dev/ttyUSB0
make fwup-tab5                            # shorthand
make fwup BOARD=tab5 MONITOR=0            # don't open monitor
make monitor BOARD=paper
make image BOARD=tab5                     # merged .bin for esptool / web flasher
make help
```

`PORT` is optional; espflash auto-detects / asks when several are attached.

- **Tab5**: USB-C goes to the P4's USB Serial/JTAG (`/dev/ttyACM*`). If it
  doesn't enter download mode, hold BOOT while pressing RESET.
- **M5Paper**: CH9102 USB-UART (`/dev/ttyUSB*` or `/dev/ttyACM*`). On battery,
  the board stays on only while firmware holds GPIO2 high (done in `main`).

## Notes

- Tab5 uses pre-v3 ESP32-P4 silicon, so `CONFIG_ESP32P4_SELECTS_REV_LESS_V3=y`
  is set and the IDF-built bootloader (not espflash's bundled one) is flashed.
- Change ESP-IDF version in `boards/*/.cargo/config.toml` (`ESP_IDF_VERSION`).
- After editing `sdkconfig.defaults`, run `make clean BOARD=...` so it is reapplied.
