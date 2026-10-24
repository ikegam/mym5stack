# M5Stack Tab5 / M5Paper firmware
#
#   make fwup BOARD=tab5            build + flash + serial monitor
#   make fwup BOARD=paper PORT=/dev/ttyUSB0
#   make fwup-tab5 / fwup-paper     shorthand
#   make help                       list all targets

# WIFI_SSID / WIFI_PASS etc. (see .env.example)
-include .env
export WIFI_SSID WIFI_PASS

# espup / espflash / ldproxy live here after `make setup`
export PATH := $(HOME)/.cargo/bin:$(PATH)

# Fetch ESP-IDF tools from Espressif's mirror (GitHub release downloads can be very slow)
export IDF_GITHUB_ASSETS ?= dl.espressif.com/github_assets

BOARD   ?= tab5
PROFILE ?= release
PORT    ?=
BAUD    ?=
MONITOR ?= 1

ifeq ($(BOARD),tab5)
  TARGET := riscv32imafc-esp-espidf
  CHIP   := esp32p4
  CRATE  := m5-tab5
else ifeq ($(BOARD),paper)
  TARGET := xtensa-esp32-espidf
  CHIP   := esp32
  CRATE  := m5-paper
else
  $(error unknown BOARD '$(BOARD)' (expected: tab5 | paper))
endif

BOARD_DIR  := boards/$(BOARD)
OUT_DIR    := $(BOARD_DIR)/target/$(TARGET)/$(PROFILE)
ELF        := $(OUT_DIR)/$(CRATE)
PARTITIONS := partitions.csv
FLASH_SIZE := 16mb
IMAGE      := $(OUT_DIR)/$(CRATE)-$(BOARD).bin
# Bootloader built by esp-idf-sys for this exact sdkconfig (chip revision etc.)
BOOTLOADER  = $(shell ls -t $(OUT_DIR)/build/esp-idf-sys-*/out/build/bootloader/bootloader.bin 2>/dev/null | head -1)

CARGO_FLAGS := $(if $(filter release,$(PROFILE)),--release,)
PORT_ARG    := $(if $(PORT),--port $(PORT),)
BAUD_ARG    := $(if $(BAUD),--baud $(BAUD),)
MONITOR_ARG := $(if $(filter 1,$(MONITOR)),--monitor,)

# espup's export script provides LIBCLANG_PATH (bindgen) and the Xtensa toolchain
ESP_ENV := $(wildcard $(HOME)/export-esp.sh)
WITH_ENV = $(if $(ESP_ENV),. $(ESP_ENV) && ,)

FLASH_OPTS = --chip $(CHIP) --partition-table $(PARTITIONS) --flash-size $(FLASH_SIZE) \
             $(if $(BOOTLOADER),--bootloader $(BOOTLOADER),)

.PHONY: help setup build fwup flash monitor image erase info test preview clean distclean \
        fwup-tab5 fwup-paper build-tab5 build-paper build-all

help:
	@echo "Usage: make <target> [BOARD=tab5|paper] [PORT=/dev/ttyACM0] [PROFILE=release|debug]"
	@echo ""
	@echo "  setup        install espup/espflash/ldproxy and Rust toolchains (once)"
	@echo "  build        build firmware for BOARD"
	@echo "  fwup         build + flash + monitor (MONITOR=0 to skip monitor)"
	@echo "  flash        flash the last build without rebuilding"
	@echo "  monitor      open serial monitor"
	@echo "  image        write a merged .bin (for web flashers / esptool)"
	@echo "  erase        erase the whole flash"
	@echo "  info         show chip/board info"
	@echo "  test         run host unit tests for common/"
	@echo "  preview      render the M5Paper screen with fake data to preview.png"
	@echo "  clean        cargo clean for BOARD"
	@echo ""
	@echo "  fwup-tab5 / fwup-paper / build-tab5 / build-paper / build-all"

setup:
	cargo install --locked espup espflash ldproxy
	espup install --targets esp32
	rustup toolchain install nightly --component rust-src
	@echo ""
	@echo "Done. Add your user to the serial group if flashing fails with EACCES:"
	@echo "  sudo usermod -aG uucp $$USER   (Arch)   /   dialout (Debian/Ubuntu)"

build:
	cd $(BOARD_DIR) && $(WITH_ENV) cargo build $(CARGO_FLAGS)

fwup: build flash

flash:
	@test -f $(ELF) || { echo "no firmware at $(ELF); run 'make build BOARD=$(BOARD)' first"; exit 1; }
	espflash flash $(PORT_ARG) $(BAUD_ARG) $(FLASH_OPTS) $(MONITOR_ARG) $(ELF)

monitor:
	espflash monitor $(PORT_ARG) --chip $(CHIP) $(if $(wildcard $(ELF)),--elf $(ELF),)

image: build
	espflash save-image --merge $(FLASH_OPTS) $(ELF) $(IMAGE)
	@echo "wrote $(IMAGE)"

erase:
	espflash erase-flash $(PORT_ARG) --chip $(CHIP)

info:
	espflash board-info $(PORT_ARG) --chip $(CHIP)

test:
	cd common && cargo test

preview:
	cd common && cargo run -q --example preview -- ../preview.pgm --active
	@if command -v magick >/dev/null; then magick preview.pgm preview.png && rm preview.pgm && echo "wrote preview.png"; fi

clean:
	cd $(BOARD_DIR) && cargo clean

distclean:
	rm -rf boards/*/target boards/*/.embuild common/target

fwup-%:
	$(MAKE) fwup BOARD=$*

build-%:
	$(MAKE) build BOARD=$*

build-all: build-tab5 build-paper
