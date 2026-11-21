"""A timer's name takes at most 30 characters (custom_components/panelos/const.py: timer_label).

Runs with the standard library only: python3 -m unittest discover -s tests

const.py imports nothing from Home Assistant, so it is loaded by its path, outside the package.
"""

from __future__ import annotations

import importlib.util
from pathlib import Path
import sys
import unittest

CONST = Path(__file__).resolve().parents[1] / "custom_components" / "panelos" / "const.py"


def _load_const():
    spec = importlib.util.spec_from_file_location("panelos_const_under_test", CONST)
    module = importlib.util.module_from_spec(spec)
    write_bytecode = sys.dont_write_bytecode
    try:
        sys.dont_write_bytecode = True
        spec.loader.exec_module(module)
    finally:
        sys.dont_write_bytecode = write_bytecode
    return module


const = _load_const()


class TimerLabel(unittest.TestCase):
    def test_the_limit_is_thirty(self) -> None:
        self.assertEqual(const.TIMER_LABEL_MAX, 30)

    def test_a_short_name_stays_trimmed(self) -> None:
        self.assertEqual(const.timer_label("  Pasta "), "Pasta")

    def test_no_name(self) -> None:
        self.assertIsNone(const.timer_label(""))
        self.assertIsNone(const.timer_label("   "))
        self.assertIsNone(const.timer_label(None))
        self.assertIsNone(const.timer_label(42))

    def test_a_long_name_is_cut_not_refused(self) -> None:
        thirty = "a" * 30
        self.assertEqual(const.timer_label(thirty), thirty)
        self.assertEqual(const.timer_label(thirty + "bcd"), thirty)
        # characters, not bytes; a cut that ends on a space leaves none at the end
        self.assertEqual(const.timer_label("árvíztűrő tükörfúrógép és mégis több"), "árvíztűrő tükörfúrógép és mégi")
        self.assertEqual(const.timer_label("b" * 29 + " tail"), "b" * 29)

    def test_the_panel_cuts_the_same_way(self) -> None:
        # app/src/pages.rs timer_label and label_limit(true) use the same rule and number
        pages = (CONST.parents[2] / "app" / "src" / "pages.rs").read_text(encoding="utf-8")
        self.assertIn("pub fn timer_label(", pages)
        limit = pages.split("pub fn label_limit(timer: bool) -> usize {", 1)[1].split("}", 1)[0]
        self.assertIn(str(const.TIMER_LABEL_MAX), limit)
        slint = (CONST.parents[2] / "app" / "ui" / "timers.slint").read_text(encoding="utf-8")
        self.assertIn(f"property <int> limit: root.edit-timer ? {const.TIMER_LABEL_MAX} : 32;", slint)


if __name__ == "__main__":
    unittest.main()
