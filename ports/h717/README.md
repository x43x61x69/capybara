# H717 Paradise emulator prototype

Goal: run the user's Paradise ROM locally on a LILYGO T-Display-S3 AMOLED
1.91-inch touch / H717, with a physical UART link to a real Tama. This is an
experimental port, not a verified playable release. It reuses Capybara's ARM
Thumb-2 and Sonix peripheral models; it does not replace the ROM with a game
reimplementation. No ROM, key or save is distributed.

## Current validation

- The independent `no_std` + `alloc` core has no desktop/GUI dependencies.
  Its 11 default host tests pass, covering instructions, combined buttons,
  wake, UART FIFO bytes, copy-on-write flash, save restore and corrupt data
  rejection. The optional recompiler adds two execution and fallback tests.
- The Python packer's 9 tests pass. A private 16 MiB Tropical ROM was exported,
  compressed to **8,478,523 bytes**, completely decoded and verified, and booted
  through the embedded core's public C ABI. The probe executed 600 million
  instructions and generated 178 frames, reaching the language selection menu.
  This is a host test, not an ESP32 benchmark or proof of playability.
- The esp-rs 1.98.1.0 Apple Silicon compiler and matching rust-src are installed
  in `local/xtensa-rust`, with both official archive SHA-256 values verified.
  The build script automatically selects this compiler and its local Cargo cache.
- The real Rust core cross-compiles for `xtensa-esp32s3-none-elf` and links with
  the Xtensa C++ screen/touch/UART frontend. PlatformIO produced `firmware.bin`,
  using 415,985 bytes of application flash and 19,568 bytes of static RAM.
  Heap allocations, including the PSRAM page cache, are additional at runtime.
- Hardware flashing and startup were verified on the H717 by USB serial
  number. A USB product ID alone cannot distinguish it from the K191.

Still required: verify screen/touch orientation on H717, measure sustained game speed
and play through the game, then validate an actual UART transaction with a
physical Tama. Software UART tests do not establish real-device compatibility.

## Build and ROM preparation

Follow the [official Espressif Rust toolchain instructions](https://docs.espressif.com/projects/rust/book/getting-started/toolchain.html)
to install rustup/espup and the Xtensa `esp` toolchain, including `rust-src`.
Source the generated `export-esp.sh`. Install PlatformIO and then, from the
repository root:

```sh
bash ports/h717/build.sh
cargo run --release --locked --example h717_export -- /path/to/own-dump.bin ports/h717/local/prepared
python3 ports/h717/tools/pack_rom.py ports/h717/local/prepared ports/h717/local/tamarom.bin --verify
```

The exporter accepts the same key sidecar/environment variable as the desktop
loader. All output in `local/` is private and ignored by Git. Output includes
the game's existing flash save, so use a copy if a clean game is desired.

The final application is `firmware/.pio/build/h717/firmware.bin`. PlatformIO
upload installs bootloader, partition table and application; the private
`tamarom.bin` must separately be written at **0x400000** using the selected S3's
esptool. Do not use `erase_flash`, which would also remove saves. Confirm that
the connected board is H717 before any upload, and disconnect Tama signal
wires while flashing/resetting. See [frontend instructions](firmware/README.md)
for the full partition layout, controls and crossed TX43/RX44 wiring.

Host validation without the Xtensa Rust compiler:

```sh
cargo test --offline --locked --manifest-path ports/h717/core/Cargo.toml
python3 -m unittest discover -s ports/h717/tools -p 'test_*.py'
cargo run --release --locked --manifest-path ports/h717/core/Cargo.toml --bin host_probe -- ports/h717/local/tamarom.bin 600000000 ports/h717/local/boot.ppm
```

The optional hybrid recompiler builds selected PRAM basic blocks into native
code. Set `H717_RECOMP_PRAM` to the absolute path of the prepared `pram.bin`
and `H717_RECOMP_RANGES` to space-separated hexadecimal `start:end` ranges
obtained by profiling that image. Use instruction-aligned starts and exclusive
ends. Generated code lives in Cargo's build directory. The interpreter handles
unsupported instructions and supplies the fallback when PRAM changes, when
the stack is outside guest SRAM, or when a block would cross a peripheral
update. The USB serial status reports how many guest instructions per second
ran as native blocks.

## Memory and storage

The board has 16 MiB flash and 8 MiB PSRAM. The partition layout reserves 12 MiB
flash for ROM, 1 MiB for saves and the remaining space for the app/system. The
packer preserves all 16 MiB of the guest ROM using per-page LZ4 compression and
deduplication; oversized packages are rejected, never truncated. PRAM, mailbox
and every decoded page have CRC checks.

The emulator reads through a callback and keeps a **1 MiB page cache**. It never
allocates a 16 MiB ROM or a second full reference copy. Guest flash writes use
copy-on-write overlays, capped at 64 pages (256 KiB) so current, backup and
temporary saves fit the save partition. Exceeding the cap stops with an error
rather than silently losing writes. This cap may need adjustment for unusually
large downloaded content. ROM and overlay identity prevent applying one game's
save to another ROM image.

The current frontend uses internal flash, with mmap when available and a
partition-read fallback. The same core callback can read an SD file in a later
board backend. H717 needs an external SPI SD module and four available signal
pins; its UART and I2C connectors alone are not a drop-in SD interface. SD
expands storage, not PSRAM or ARM emulation speed. Avoid per-instruction SD
reads; retain the decompressed-page cache and service UART independently.

## Time and other limits

The guest RTC follows powered-on wall time; save files preserve its value.
Powered-off elapsed time is not yet restored. An always-powered external RTC
or network time at startup can supply that delta; simply adding a battery
without RTC/time integration does not implement it. H717's internal ESP32 RTC
domain can retain time in deep sleep while supplied, not across total power
loss. Boards with an RTC still need its supply rail maintained when the main
MCU is off; check the exact revision's schematic and backup-power arrangement.

No speaker/buzzer output is assigned. UART uses 460800 8N1 and has bounded
buffers with overrun reporting. If CPU emulation is too slow, extra SD space
does not fix protocol deadlines. The frontend displays measured speed so that
an on-screen image is not mistaken for real-time playability.
