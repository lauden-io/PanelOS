"""Home Assistant's time zone for the panels (custom_components/panelos/timezone.py).

Runs with the standard library only: python3 -m unittest discover -s tests
"""

from __future__ import annotations

from datetime import datetime, timezone
import importlib.util
from pathlib import Path
import struct
import sys
import unittest
from zoneinfo import ZoneInfo

MODULE = Path(__file__).resolve().parents[1] / "custom_components" / "panelos" / "timezone.py"
_spec = importlib.util.spec_from_file_location("panelos_timezone_under_test", MODULE)
tz = importlib.util.module_from_spec(_spec)
sys.modules[_spec.name] = tz
_spec.loader.exec_module(tz)


def tzif(footer: bytes | None, version: bytes = b"2", times: int = 0, types: int = 1, chars: int = 4, leap: int = 0,
         isstd: int = 0, isut: int = 0) -> bytes:
    """A TZif file with the given counts (zero-filled data blocks) and footer line."""

    def header() -> bytes:
        return struct.pack(">4sc15x6L", b"TZif", version, isut, isstd, leap, times, types, chars)

    v1 = header() + bytes(times * 5 + types * 6 + chars + leap * 8 + isstd + isut)
    if version == b"\0":
        return v1
    v2 = header() + bytes(times * 9 + types * 6 + chars + leap * 12 + isstd + isut)
    return v1 + v2 + (b"\n" + footer + b"\n" if footer is not None else b"")


class FooterRule(unittest.TestCase):
    def test_reads_the_rule(self) -> None:
        self.assertEqual(tz.footer_rule(tzif(b"CET-1CEST,M3.5.0,M10.5.0/3")), "CET-1CEST,M3.5.0,M10.5.0/3")

    def test_skips_both_data_blocks(self) -> None:
        # counts in every field: the footer is found behind both blocks
        data = tzif(b"EST5EDT,M3.2.0,M11.1.0", times=236, types=6, chars=20, leap=27, isstd=6, isut=6)
        self.assertEqual(tz.footer_rule(data), "EST5EDT,M3.2.0,M11.1.0")

    def test_quoted_names_and_extensions(self) -> None:
        for rule in (b"<+0530>-5:30", b"<-02>2<-01>,M3.5.0/-1,M10.5.0/0", b"IST-2IDT,M3.4.4/26,M10.5.0", b"UTC0"):
            with self.subTest(rule=rule):
                self.assertEqual(tz.footer_rule(tzif(rule, version=b"3")), rule.decode())

    def test_later_versions(self) -> None:
        self.assertEqual(tz.footer_rule(tzif(b"JST-9", version=b"4")), "JST-9")

    def test_no_rule(self) -> None:
        # an empty footer: no rule for the times after the last transition
        self.assertIsNone(tz.footer_rule(tzif(b"")))
        # version 1: no footer at all
        self.assertIsNone(tz.footer_rule(tzif(None, version=b"\0")))
        # version 2 without its footer
        self.assertIsNone(tz.footer_rule(tzif(None)))

    def test_not_a_zone_file(self) -> None:
        self.assertIsNone(tz.footer_rule(b""))
        self.assertIsNone(tz.footer_rule(b"TZif"))
        self.assertIsNone(tz.footer_rule(b"XXXX" + tzif(b"UTC0")[4:]))
        # cut short inside the second block, or the second header damaged
        data = tzif(b"UTC0", times=10)
        self.assertIsNone(tz.footer_rule(data[:-20]))
        first = 44 + 10 * 5 + 6 + 4
        self.assertIsNone(tz.footer_rule(data[:first] + b"XXXX" + data[first + 4 :]))

    def test_footer_without_its_closing_newline(self) -> None:
        self.assertIsNone(tz.footer_rule(tzif(b"UTC0")[:-1]))

    def test_odd_footers(self) -> None:
        self.assertIsNone(tz.footer_rule(tzif("CET-1CÉST".encode())))
        self.assertIsNone(tz.footer_rule(tzif(b"CET -1")))
        self.assertIsNone(tz.footer_rule(tzif(b"CET\t-1")))


class ZoneMessage(unittest.TestCase):
    def test_message(self) -> None:
        self.assertEqual(
            tz.zone_message("America/New_York", "EST5EDT,M3.2.0,M11.1.0"),
            {"name": "America/New_York", "posix": "EST5EDT,M3.2.0,M11.1.0"},
        )

    def test_none_without_a_rule(self) -> None:
        self.assertIsNone(tz.zone_message("Europe/Berlin", None))
        self.assertIsNone(tz.zone_message("Europe/Berlin", ""))

    def test_names_outside_the_zone_folders(self) -> None:
        for name in ("", "/etc/passwd", "../etc/passwd", "Europe/../../etc/passwd", "Europe//Berlin", "Europe/Berlin/",
                     "Europe\\Berlin", "Europe/Ber lin"):
            with self.subTest(name=name):
                self.assertIsNone(tz.zone_file(name))
                self.assertIsNone(tz.time_zone(name))


class Offsets(unittest.TestCase):
    def test_rule_offsets(self) -> None:
        cases = {
            "CET-1CEST,M3.5.0,M10.5.0/3": (3600, 7200),
            "EST5EDT,M3.2.0,M11.1.0": (-18000, -14400),
            "EST5EDT": (-18000, -14400),
            "NST3:30NDT,M3.2.0,M11.1.0": (-12600, -9000),
            "<+1030>-10:30<+11>-11,M10.1.0,M4.1.0": (37800, 39600),
            "<-02>2<-01>,M3.5.0/-1,M10.5.0/0": (-7200, -3600),
            "<+1245>-12:45<+1345>,M9.5.0/2:45,M4.1.0/3:45": (45900, 49500),
            "IST-5:30": (19800,),
            "<+01>-1": (3600,),
            "UTC0": (0,),
            "HST10": (-36000,),
        }
        for rule, offsets in cases.items():
            with self.subTest(rule=rule):
                self.assertEqual(tz.rule_offsets(rule), offsets)

    def test_unreadable_rules(self) -> None:
        for rule in ("", "Europe/Berlin", ":Europe/Berlin", "CET", "CE-1", "-1", "CET-1 CEST"):
            with self.subTest(rule=rule):
                self.assertEqual(tz.rule_offsets(rule), ())

    def test_fixed_rule(self) -> None:
        cases = {
            (3600, "+01"): "<+01>-1",
            (19800, "IST"): "IST-5:30",
            (-12600, None): "<-0330>3:30",
            (-10800, "-03"): "<-03>3",
            (0, "GMT"): "GMT0",
            (0, "+00"): "<+00>0",
            (-9000, "NDT"): "NDT2:30",
            (20700, "x"): "<+0545>-5:45",
            (-36000, ""): "<-10>10",
        }
        for (offset, abbreviation), rule in cases.items():
            with self.subTest(offset=offset, abbreviation=abbreviation):
                self.assertEqual(tz.fixed_rule(offset, abbreviation), rule)
                self.assertEqual(tz.rule_offsets(rule), (offset,))

    def test_pick_rule(self) -> None:
        cet = "CET-1CEST,M3.5.0,M10.5.0/3"
        # winter and summer: the footer
        self.assertEqual(tz.pick_rule(cet, 3600, "CET"), cet)
        self.assertEqual(tz.pick_rule(cet, 7200, "CEST"), cet)
        # the footer misses the offset in effect: that offset alone
        self.assertEqual(tz.pick_rule("<+00>0", 3600, "+01"), "<+01>-1")
        # no footer: the offset in effect; nothing known: nothing
        self.assertEqual(tz.pick_rule(None, 19800, "IST"), "IST-5:30")
        self.assertIsNone(tz.pick_rule(None, None))
        # the offset not known: the footer as it is
        self.assertEqual(tz.pick_rule(cet, None), cet)


def _have(name: str) -> bool:
    return tz.zone_file(name) is not None


class RealZones(unittest.TestCase):
    """The zones of this computer's database (system zone folders or the tzdata package), where there is one."""

    @unittest.skipUnless(_have("Europe/Berlin"), "no zone database here")
    def test_central_europe(self) -> None:
        self.assertEqual(tz.time_zone("Europe/Berlin"), {"name": "Europe/Berlin", "posix": "CET-1CEST,M3.5.0,M10.5.0/3"})

    @unittest.skipUnless(_have("America/New_York"), "no zone database here")
    def test_new_york(self) -> None:
        self.assertEqual(tz.time_zone("America/New_York")["posix"], "EST5EDT,M3.2.0,M11.1.0")

    @unittest.skipUnless(_have("Asia/Kolkata"), "no zone database here")
    def test_half_hour_zone(self) -> None:
        self.assertEqual(tz.time_zone("Asia/Kolkata")["posix"], "IST-5:30")

    @unittest.skipUnless(_have("UTC"), "no zone database here")
    def test_utc(self) -> None:
        self.assertEqual(tz.time_zone("UTC")["posix"], "UTC0")

    def test_unknown_zone(self) -> None:
        self.assertIsNone(tz.time_zone("Nowhere/Atlantis"))

    @unittest.skipUnless(_have("Africa/Casablanca"), "no zone database here")
    def test_the_rule_gives_the_offset_in_effect(self) -> None:
        # every month of two years: the rule sent can give the offset Python's zone database has then (Casablanca's
        # footer does not describe its years with Ramadan changes)
        zones = ("Africa/Casablanca", "Europe/Berlin", "America/New_York", "Australia/Lord_Howe", "America/Nuuk",
                 "Asia/Kolkata", "Pacific/Chatham")
        for name in zones:
            for month in range(24):
                now = datetime(2026 + month // 12, month % 12 + 1, 15, 12, tzinfo=timezone.utc)
                with self.subTest(zone=name, when=now.date()):
                    rule = tz.time_zone(name, now)["posix"]
                    offset = now.astimezone(ZoneInfo(name)).utcoffset().total_seconds()
                    self.assertIn(offset, tz.rule_offsets(rule))


if __name__ == "__main__":
    unittest.main()
