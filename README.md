# mym5stack — Rust + ESP32 app patterns, ready for AI agents

Working Rust + ESP32 code plus notes on everything that broke along the way.
Starting a Rust + ESP32 project with an AI agent? Just show it this repo.

Rust + ESP32 でアプリを作るときのつまずきどころと動くコードの集まりです。AI エージェントにこの repo を見せてください。

## Boards

Rust (std, ESP-IDF v5.5 via `esp-idf-svc`) firmware for two boards:

| BOARD   | Device        | MCU                  | Rust target                | Toolchain          |
|---------|---------------|----------------------|----------------------------|--------------------|
| `tab5`  | M5Stack Tab5  | ESP32-P4 (RISC-V)    | `riscv32imafc-esp-espidf`  | `esp` (from espup) |
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

## Tab5: TinyStories LLM

The Tab5 firmware runs Karpathy's [llama2.c](https://github.com/karpathy/llama2.c)
`stories15M` model (15M params, trained on the TinyStories dataset) with int8
weights in PSRAM, and shows a short story built from today's weather on the
screen with a small animated scene. Japanese stories use
[opsbr/stories15M-ja](https://huggingface.co/opsbr/stories15M-ja) when its
files are on the card.

```sh
make llm-model          # -> llm/stories15M{,-ja}.q8 + llm/tokenizer{,-ja}.bin
```

Copy the files to the root of a FAT32 microSD card, insert it and
`make fwup BOARD=tab5`. Try the model on the host first with
`cd common && cargo run --release --example llm -- run ../llm/stories15M.q8 ../llm/tokenizer.bin "Once upon a time"`.

Stories open with today's JMA forecast. Tap the region in the top-right
corner to pick another one on a map of Japan; the choice is saved to
`region.txt` on the card. The map (`common/src/japan_map.bin`) is built from
[dataofjapan/land](https://github.com/dataofjapan/land) `japan.geojson`
(source: 地球地図日本, GSI) with
`cargo run --release --example japan_map_gen -- japan.geojson src/japan_map.bin`.

## Notes

- Tab5 uses pre-v3 ESP32-P4 silicon, so `CONFIG_ESP32P4_SELECTS_REV_LESS_V3=y`
  is set and the IDF-built bootloader (not espflash's bundled one) is flashed.
- Change ESP-IDF version in `boards/*/.cargo/config.toml` (`ESP_IDF_VERSION`).
- After editing `sdkconfig.defaults`, run `make clean BOARD=...` so it is reapplied.

## License

BSD 4-clause, see [LICENSE](LICENSE). Files derived from Espressif's BSP stay
under Apache-2.0, and the Japan map data comes from 地球地図日本 (GSI).
