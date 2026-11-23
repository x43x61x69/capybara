# Private ROM preparation

Use a dump from your own console. The exporter decrypts the boot code on the
computer, so the ESP32 does not need to search for a device key or hold a second
16 MiB copy of the ROM.

From the Capybara repository root:

```sh
cargo run --release --locked --example h717_export -- /path/to/your-dump.bin ports/h717/local/prepared
python3 ports/h717/tools/pack_rom.py ports/h717/local/prepared ports/h717/local/tamarom.bin --verify
python3 -m unittest discover -s ports/h717/tools -p 'test_*.py'
```

The exporter reads the same `.key` sidecar or `SONIX_DEVICE_KEY` environment
variable as the native loader. Keep the entire `local/` directory private: it
contains firmware, decrypted code, save data and a console key.

The package preserves all 16 MiB of guest flash, including existing game-save
pages. Each 4 KiB page is stored either as a raw LZ4 block or uncompressed, and
identical pages share storage. Each decoded page and the complete ROM carry
IEEE CRC32 checksums. The `--verify` option decodes the complete package before
writing it. A package over the 12 MiB partition budget is rejected; the tool
never truncates a ROM to make it fit.

`H717ROM1` starts with a 64-byte little-endian header. It contains version,
package size, ROM CRC32, device key, XIP base, 4096-byte page geometry, offsets
to 64 KiB PRAM, 4 KiB mailbox, page index and payload, the CRC32 of its first
52 bytes at offset 52, and PRAM/mailbox CRC32 values at offsets 56 and 60.
Each of the 4096 index entries is `(absolute offset, encoded length,
decoded CRC32)` in three `u32` fields. A length of 4096 denotes raw data;
shorter lengths denote raw LZ4 blocks.
