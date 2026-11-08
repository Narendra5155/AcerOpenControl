import unittest

from acerfan import ec as E
from acerfan.acpi import AcpiError, encode_buffer, parse_reply, u32_buffer


class ParseReply(unittest.TestCase):
    def test_int(self):
        self.assertEqual(parse_reply("0x1f\x00"), 0x1F)

    def test_buffer(self):
        self.assertEqual(parse_reply("{0x01, 0xff, 0x00, 0x10}"), b"\x01\xff\x00\x10")

    def test_empty_buffer(self):
        self.assertEqual(parse_reply("{}"), b"")

    def test_error(self):
        with self.assertRaises(AcpiError):
            parse_reply("Error: AE_NOT_FOUND")

    def test_not_called(self):
        self.assertIsNone(parse_reply("not called"))


class Encoding(unittest.TestCase):
    def test_u32_le(self):
        self.assertEqual(encode_buffer(u32_buffer(0x8040)), "b40800000")

    def test_pct(self):
        self.assertEqual(E.pct_to_raw(100), 255)
        self.assertEqual(E.pct_to_raw(0), 0)
        self.assertEqual(E.pct_to_raw(150), 255)
        self.assertEqual(E.raw_to_pct(255), 100)

    def test_rpm(self):
        self.assertEqual(E.decode_rpm(b"\x00\x00"), 0)
        self.assertEqual(E.decode_rpm(b"\xff\xff"), 0)
        self.assertEqual(E.decode_rpm(b"\x01\xa4", big_endian=True), int(2156220 / 0x01A4))


class FakeAcpi:
    def __init__(self):
        self.calls = []
        self.ram = bytearray(range(256))
        self.indx = 0

    def wmbb(self, cmd, arg=0):
        self.calls.append((cmd, arg))
        if cmd == E.CMD_SET_INDEX:
            self.indx = arg
            return cmd
        if cmd == E.CMD_READ_EC:
            off = self.indx & 0xFFFF
            size = 1 << ((self.indx >> 16) & 0xFFFF)
            return bytes(self.ram[off:off + size]) + b"\x00" * (4 - size)
        return cmd


class ClevoEC(unittest.TestCase):
    def test_read_ec(self):
        fa = FakeAcpi()
        ec = E.ClevoEC(fa)
        self.assertEqual(ec.read_ec(0xD0, 2), bytes([0xD0, 0xD1]))
        self.assertEqual(fa.calls[0], (E.CMD_SET_INDEX, (1 << 16) | 0xD0))

    def test_set_duty_packs_bytes(self):
        fa = FakeAcpi()
        E.ClevoEC(fa).set_duty(100, 0)
        self.assertEqual(fa.calls[-1], (E.CMD_SET_FAN_DUTY, 0x00FF))

    def test_set_auto(self):
        fa = FakeAcpi()
        E.ClevoEC(fa).set_auto()
        self.assertEqual(fa.calls[-1], (E.CMD_SET_FAN_AUTO, 0x0F))

    def test_range_checks(self):
        ec = E.ClevoEC(FakeAcpi())
        with self.assertRaises(ValueError):
            ec.read_ec(0xFF, 2)
        with self.assertRaises(ValueError):
            ec.read_ec(0, 3)


if __name__ == "__main__":
    unittest.main()
