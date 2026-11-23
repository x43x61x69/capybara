import random
import struct
import unittest
from unittest.mock import patch

import pack_rom as pack


class Lz4Tests(unittest.TestCase):
    def test_literals_overlapping_matches_and_long_lengths(self):
        rng = random.Random(717)
        pages = [
            bytes(pack.PAGE_SIZE),
            b"\xff" * pack.PAGE_SIZE,
            bytes(range(256)) * 16,
            rng.randbytes(pack.PAGE_SIZE),
            (b"Paramix H717 " * 342)[:pack.PAGE_SIZE],
        ]
        for page in pages:
            self.assertEqual(pack.lz4_decompress(pack.lz4_compress(page)), page)
        self.assertEqual(pack.lz4_decompress(b"\x1fA\x01\x00\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xfb"), b"A" * 4096)

    def test_invalid_blocks_are_rejected(self):
        for bad in [b"", b"\xf0", b"\x10", b"\x00\x00\x00", b"\x00\x01\x00", b"\x1fA\x01\x00\xff"]:
            with self.subTest(block=bad), self.assertRaises(pack.PackageError):
                pack.lz4_decompress(bad)

    def test_match_and_literal_boundaries(self):
        rng = random.Random(73410)
        for period in [1, 2, 3, 4, 15, 16, 255, 256, 257, 512, 4096]:
            seed = rng.randbytes(period)
            page = (seed * (4096 // period + 1))[:4096]
            with self.subTest(period=period):
                self.assertEqual(pack.lz4_decompress(pack.lz4_compress(page)), page)


class PackageTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        # Exercise the entire 16 MiB address space and writable save pages near its end.
        flash = bytearray(b"\xff" * pack.ROM_SIZE)
        rng = random.Random(96)
        flash[:4096] = rng.randbytes(4096)
        flash[0xEFE000:0xEFF000] = b"saved-game" * 409 + b"!" * 6
        cls.flash = bytes(flash)
        cls.package = pack.build_package(cls.flash, bytes(pack.PRAM_SIZE), bytes(pack.MAILBOX_SIZE), {"device_key": None, "xip_base": 0x60011000})

    def test_full_round_trip_and_deduplication(self):
        info = pack.verify_package(self.package)
        self.assertEqual(info["unique_pages"], 3)
        self.assertEqual(info["rom_crc32"], pack.crc32(self.flash))
        page = 0xEFE000 // pack.PAGE_SIZE
        offset, length, _ = struct.unpack_from("<III", self.package, pack.INDEX_OFFSET + 12 * page)
        encoded = self.package[offset:offset + length]
        decoded = encoded if length == 4096 else pack.lz4_decompress(encoded)
        self.assertEqual(decoded, self.flash[0xEFE000:0xEFF000])

    def test_header_and_payload_corruption(self):
        for where in [8, 12, 16, 52, 63, pack.INDEX_OFFSET, len(self.package) - 1]:
            changed = bytearray(self.package)
            changed[where] ^= 0x80
            with self.subTest(offset=where), self.assertRaises(pack.PackageError):
                pack.verify_package(bytes(changed))

    def test_out_of_range_payload_and_truncation(self):
        changed = bytearray(self.package)
        struct.pack_into("<I", changed, pack.INDEX_OFFSET, len(changed))
        with self.assertRaises(pack.PackageError):
            pack.verify_package(bytes(changed))
        with self.assertRaises(pack.PackageError):
            pack.verify_package(self.package[:-1])

    def test_boot_payload_corruption(self):
        for where in [pack.PRAM_OFFSET, pack.MAILBOX_OFFSET, 56, 60]:
            changed = bytearray(self.package)
            changed[where] ^= 1
            with self.subTest(offset=where), self.assertRaisesRegex(pack.PackageError, "checksum mismatch"):
                pack.verify_package(bytes(changed))

    def test_reject_incorrect_input_size(self):
        with self.assertRaises(pack.PackageError):
            pack.build_package(b"", bytes(pack.PRAM_SIZE), bytes(pack.MAILBOX_SIZE), {"xip_base": 0})

    def test_reject_package_over_budget_and_invalid_metadata(self):
        with patch.object(pack, "MAX_PACKAGE_SIZE", len(self.package) - 1):
            with self.assertRaisesRegex(pack.PackageError, "No ROM data was truncated"):
                pack.build_package(self.flash, bytes(pack.PRAM_SIZE), bytes(pack.MAILBOX_SIZE), {"xip_base": 0})
        for field in ["device_key", "xip_base"]:
            for value in [False, -1, 0x100000000, "0x1234"]:
                metadata = {"device_key": None, "xip_base": 0}
                metadata[field] = value
                with self.subTest(field=field, value=value), self.assertRaises(pack.PackageError):
                    pack.build_package(self.flash, bytes(pack.PRAM_SIZE), bytes(pack.MAILBOX_SIZE), metadata)


if __name__ == "__main__":
    unittest.main()
