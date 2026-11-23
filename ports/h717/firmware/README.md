# H717 hardware frontend

Target: **LILYGO T-Display-S3 AMOLED 1.91-inch touch / H717**, ESP32-S3R8,
16 MiB QIO flash and 8 MiB OPI PSRAM. This is not the K191/T-QT Pro,
T4-S3, AMOLED Lite, or AMOLED Plus. The firmware checks PSRAM at startup.

This frontend drives the real emulator in `../core` through its C ABI. There
is no demo animation or replacement game linked when that core is missing.
See `../README.md` for the build prerequisites and current validation status.

## Build

Build the Xtensa Rust static library first, then:

```sh
cd ports/h717/firmware
pio run -e h717
```

The default archive is
`../core/target/xtensa-esp32s3-none-elf/release/libh717_core.a`. Set
`H717_CORE_ARCHIVE` to override the path. The linker stops with an explicit
error if the real emulator archive is absent. PlatformIO uses Arduino-ESP32
2.0.17 through `espressif32@6.12.0`; it needs no third-party display library.

The frontend alone can be checked without pretending to produce a working
firmware image:

```sh
pio run -e h717 \
  -t .pio/build/h717/src/main.cpp.o \
  -t .pio/build/h717/src/h717_board.cpp.o
```

The Xtensa Rust core, C++ frontend and Arduino framework dependencies compile
and link successfully into firmware.bin. The verified esp-rs toolchain is
installed under ../local/xtensa-rust and selected by ../build.sh automatically.
Host-side core ROM boot tests do not validate H717 speed, touch alignment or
physical UART timing.

Do not flash a display-only or stub-linked build. Do not upload to a connected
board until its model is confirmed; the USB product ID alone cannot distinguish
H717 from K191. A normal upload writes bootloader, partitions and application,
but does not install the separately prepared private ROM package. The ROM data
partition is named `tamarom`, at `0x400000`, length `0xC00000`.
The frontend first attempts a flash-cache mapping of the packed ROM, checking
the SDK result. If MMU space is insufficient, the same reader callback falls
back to bounded `esp_partition_read` calls. Neither route copies the whole
ROM into PSRAM; the Rust core maintains its bounded decompressed page cache.

## Controls

The 128×128 game screen is scaled to 224×224 with nearest-neighbor sampling.
The remaining landscape display contains touch controls:

| Left side | Right side |
| --- | --- |
| A | Dial clockwise |
| B | Dial counterclockwise |
| C | Wheel press |
| A+C together | Save |

Touch buttons remain pressed while held. The physical **BOOT button is B**, so
BOOT plus a touch on A or C produces A+B or B+C. The explicit A+C touch button
handles the Paramix shortcut on the single-touch CST816 controller. Holding a
dial button repeats at 180 ms. The RESET button resets the ESP32, not a Tama
button; save before resetting.

USB serial at 115200 accepts `1`, `2`, `3`, `4` to toggle held A, B, C and wheel;
`0` releases all USB-held buttons. `+` and `-` turn the dial, and `s` requests a
save. USB controls are useful for testing combinations and remain separate from
the physical Tama UART.

Send `p` over USB to toggle timing diagnostics. The log gives the time since
boot for USB button commands and the fingerprint of each framebuffer sent to
the AMOLED, so a benchmark can match the same screen across builds.

The lower corners show measured emulated speed and TX/RX counters. A value below
100% means the emulator is progressing slower than the original 96 MHz cycle
timeline. The USB log also reports PC, frame counter, guest baud rate, physical
UART overrun count and free PSRAM. A display appearing is not proof that the
game runs fast enough or that link deadlines are met.

## Real Tama connection

| H717 UART connector | GPIO | Tama |
| --- | --- | --- |
| TX | 43 | RX |
| RX | 44 | TX |
| GND | — | GND |
| 3V3 | — | Leave disconnected |

Use crossed TX/RX and a common ground, with independent power for Tama. The
frontend configures UART1 to **460800 baud, 8 data bits, no parity, one stop bit,
no hardware flow control**. It connects directly to the emulated Sonix UART;
it does not implement a different host transfer protocol or alter the ROM.

GPIO43 also carries ESP32 ROM boot messages before this firmware starts. Keep
the Tama signal wires disconnected while flashing/resetting, then connect
after the H717 UI appears. Application logs go to native USB Serial/JTAG and
are not sent onto the Tama UART. This follows the project's existing direct
UART adapter wiring; electrical compatibility has not been re-measured here.

The emulator owns all machine calls on CPU core 1. Display, touch and USB input
run on CPU core 0. The UART driver has a 16 KiB RX ring; partial RX/TX writes are
retained across short emulator batches. An overrun is reported rather than
treated as a successful transfer. Reliable real-device play still requires
measured near-real-time emulation and protocol testing on hardware.

## Save behavior

LittleFS occupies 1 MiB at `0x300000`. It is never automatically formatted on a
mount failure. On a fresh installation, send uppercase **`F`** over USB serial
once to initialize the unmounted save filesystem. This erases that save
partition; it does not erase the ROM partition. If an existing installation
fails to mount, preserve/recover its flash before choosing to format it.

`SAVE` queues a write; autosave queues one every two minutes. Both wait until the
guest UART is disabled and two seconds have passed without line activity,
because internal flash programming stalls the flash cache. Save files use a
temporary file plus one previous backup. On boot, a save is restored only if
the core validates its CRC and ROM identity; the backup is tried if needed.
The core limits changed pages so the temporary/current/backup copies fit.

Saves preserve the game's changed flash and emulated RTC, not a full CPU
snapshot. The H717 has no configured battery-backed real-time clock here;
power-off time does not automatically advance the in-game calendar. There is
no audio output assigned in this frontend.

## Hardware sources

- [LILYGO AMOLED Series board map and touch support](https://github.com/Xinyuan-LilyGO/LilyGo-AMOLED-Series)
- [LILYGO RM67162 reference QSPI driver](https://github.com/Xinyuan-LilyGO/T-Display-S3-AMOLED/blob/main/examples/factory/rm67162.cpp)
- [LILYGO reference pins](https://github.com/Xinyuan-LilyGO/T-Display-S3-AMOLED/blob/main/examples/factory/pins_config.h)

The direct QSPI transport uses command `0x02` for registers, `0x32` for quad
pixel writes, RGB565 format, and MADCTL `MX|MV` for landscape. CST816 is probed
at I²C address `0x15`, SDA3/SCL2/IRQ21. With MADCTL `MX|MV`, the CST816's native
coordinates match the landscape display. Pin and touch alignment still require hardware
verification on the user's particular board revision.
