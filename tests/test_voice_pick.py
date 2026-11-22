"""Which ESPHome entry is a panel's, and which reported MAC address its entry keeps
(custom_components/panelos/voice_pick.py).

Runs with the standard library only: python3 -m unittest discover -s tests

voice_pick.py imports nothing from Home Assistant, so it is loaded by its path, outside the package.
"""

from __future__ import annotations

import importlib.util
from pathlib import Path
import sys
import unittest

PICK = Path(__file__).resolve().parents[1] / "custom_components" / "panelos" / "voice_pick.py"


def _load():
    spec = importlib.util.spec_from_file_location("panelos_voice_pick_under_test", PICK)
    module = importlib.util.module_from_spec(spec)
    write_bytecode = sys.dont_write_bytecode
    try:
        sys.dont_write_bytecode = True
        sys.modules[spec.name] = module
        spec.loader.exec_module(module)
    finally:
        sys.dont_write_bytecode = write_bytecode
    return module


m = _load()
C = m.Candidate
USER_A = "0123456789abcdef0123456789abcdef"
USER_B = "fedcba9876543210fedcba9876543210"
ADMIN_ID = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
MAC_OLD = "aabbccddee01"
MAC_NEW = "aabbccddee02"
MAC_OTHER = "112233445566"


class PlainMac(unittest.TestCase):
    def test_forms(self) -> None:
        for value in ("aa:bb:cc:dd:ee:01", "AA:BB:CC:DD:EE:01", "aa-bb-cc-dd-ee-01", "aabbccddee01"):
            with self.subTest(value=value):
                self.assertEqual(m.plain_mac(value), MAC_OLD)
        self.assertEqual(m.written_mac(MAC_OLD), "aa:bb:cc:dd:ee:01")

    def test_not_a_mac(self) -> None:
        for value in (None, 12, "", "nspanel-pro", "aa:bb:cc:dd:ee", "aa:bb:cc:dd:ee:0g", "aa:bb:cc:dd:ee:01:02"):
            with self.subTest(value=value):
                self.assertIsNone(m.plain_mac(value))


class Pick(unittest.TestCase):
    def test_none_or_one(self) -> None:
        self.assertIsNone(m.pick([], None))
        self.assertIsNone(m.pick([], MAC_OLD))
        self.assertEqual(m.pick([C(MAC_OLD, True, None)], None), 0)
        self.assertEqual(m.pick([C(MAC_OLD, True, USER_A)], MAC_OLD), 0)

    def test_by_name_alone_the_first(self) -> None:
        self.assertEqual(m.pick([C(MAC_OTHER, False, None), C(MAC_OLD, True, None)], None), 0)

    def test_the_kept_mac_picks_while_nobody_is_named(self) -> None:
        self.assertEqual(m.pick([C(MAC_OTHER, True, None), C(MAC_OLD, True, None)], MAC_OLD), 1)
        # an entry without a MAC address as its unique id fits any
        self.assertEqual(m.pick([C(MAC_OTHER, True, None), C(None, True, None)], MAC_OLD), 1)

    def test_another_device_is_never_taken_while_nobody_is_named(self) -> None:
        self.assertIsNone(m.pick([C(MAC_OTHER, True, None)], MAC_OLD))
        self.assertIsNone(m.pick([C(MAC_OTHER, False, USER_A)], MAC_OLD))

    def test_an_entry_that_names_its_user_comes_first(self) -> None:
        self.assertEqual(m.pick([C(MAC_OTHER, True, None), C(MAC_OLD, True, USER_A)], None), 1)

    def test_only_an_entry_with_a_key_names_a_user(self) -> None:
        self.assertEqual(m.pick([C(MAC_OLD, True, None), C(MAC_OTHER, False, USER_B)], None), 0)
        self.assertIsNone(C(MAC_OTHER, False, USER_B).named)
        self.assertEqual(C(MAC_OTHER, True, USER_B).named, USER_B)

    def test_the_entry_of_the_kept_mac_alone_while_it_is_there(self) -> None:
        # another device given the panel's name (Home Assistant takes a new name from the device) that names a user
        # does not count while the panel's own entry is there, even while the panel names nobody
        panel, other = C(MAC_OLD, True, None), C(MAC_OTHER, True, USER_B)
        self.assertEqual(m.pick([other, panel], MAC_OLD), 1)
        self.assertEqual(m.pick([other, C(MAC_OLD, False, None)], MAC_OLD), 1)

    def test_a_replaced_panel(self) -> None:
        old, new = C(MAC_OLD, True, None), C(MAC_NEW, True, USER_B)
        # while the old panel's entry is there it stays the panel's
        self.assertEqual(m.pick([old, new], MAC_OLD), 0)
        # deleted: the new one, once it names its user
        self.assertEqual(m.pick([new], MAC_OLD), 0)
        self.assertIsNone(m.pick([C(MAC_NEW, True, None)], MAC_OLD))

    def test_two_that_name_a_user(self) -> None:
        both = [C(MAC_OLD, True, USER_A), C(MAC_NEW, True, USER_B)]
        self.assertEqual(m.pick(both, MAC_NEW), 1)
        self.assertEqual(m.pick(both, MAC_OLD), 0)
        self.assertEqual(m.pick(both, MAC_OTHER), 0)
        self.assertEqual(m.pick(both, None), 0)


class KeptMac(unittest.TestCase):
    PANEL = [C(MAC_OLD, True, USER_A)]

    def test_confirmed_by_the_panels_esphome_device(self) -> None:
        self.assertEqual(m.kept_mac(None, "AA:BB:CC:DD:EE:01", USER_A, False, self.PANEL), "aa:bb:cc:dd:ee:01")
        self.assertTrue(m.confirmed(self.PANEL, "aa:bb:cc:dd:ee:01", USER_A))

    def test_another_user_neither_sets_nor_takes_it(self) -> None:
        for reported in (MAC_OTHER, MAC_OLD, None, "", "not a mac"):
            with self.subTest(reported=reported):
                self.assertIsNone(m.kept_mac(None, reported, USER_B, False, self.PANEL))
                self.assertEqual(m.kept_mac("aa:bb:cc:dd:ee:01", reported, USER_B, False, self.PANEL), "aa:bb:cc:dd:ee:01")

    def test_not_confirmed(self) -> None:
        cases = {
            "no ESPHome entry": [],
            "the entry names nobody": [C(MAC_OLD, True, None)],
            "the entry has no key": [C(MAC_OLD, False, USER_A)],
            "the entry names another user": [C(MAC_OLD, True, USER_B)],
            "another entry names the user": [C(MAC_OTHER, True, USER_A)],
            "the entry has no MAC address as its unique id": [C(None, True, USER_A)],
        }
        for why, candidates in cases.items():
            with self.subTest(why=why):
                self.assertFalse(m.confirmed(candidates, MAC_OLD, USER_A))
                self.assertIsNone(m.kept_mac(None, MAC_OLD, USER_A, False, candidates))
        self.assertFalse(m.confirmed(self.PANEL, MAC_OLD, None))
        self.assertFalse(m.confirmed(self.PANEL, MAC_OLD, ""))

    def test_an_administrators_report_is_kept(self) -> None:
        self.assertEqual(m.kept_mac(None, MAC_OTHER, USER_B, True, self.PANEL), "11:22:33:44:55:66")
        # but only a MAC address
        self.assertEqual(m.kept_mac("aa:bb:cc:dd:ee:01", "not a mac", USER_B, True, self.PANEL), "aa:bb:cc:dd:ee:01")

    def test_a_report_without_one_keeps_the_old_one(self) -> None:
        self.assertEqual(m.kept_mac("aa:bb:cc:dd:ee:01", None, USER_A, False, self.PANEL), "aa:bb:cc:dd:ee:01")

    def test_not_moved_while_the_entry_of_the_kept_one_is_there(self) -> None:
        # a user kept for connecting first, with a device of its own given the panel's name: its device confirms it,
        # but the panel's entry is still there
        old = "aa:bb:cc:dd:ee:01"
        candidates = [C(MAC_OLD, True, None), C(MAC_OTHER, True, USER_B)]
        self.assertTrue(m.confirmed(candidates, MAC_OTHER, USER_B))
        self.assertEqual(m.kept_mac(old, MAC_OTHER, USER_B, False, candidates), old)
        # the same for the panel's own new hardware until the old panel's entry is deleted
        self.assertEqual(m.kept_mac(old, MAC_NEW, USER_A, False, [C(MAC_OLD, True, None), C(MAC_NEW, True, USER_A)]), old)
        self.assertEqual(m.kept_mac(old, MAC_NEW, USER_A, False, [C(MAC_NEW, True, USER_A)]), "aa:bb:cc:dd:ee:02")
        # an administrator's report moves it
        self.assertEqual(m.kept_mac(old, MAC_OTHER, ADMIN_ID, True, candidates), "11:22:33:44:55:66")


if __name__ == "__main__":
    unittest.main()
