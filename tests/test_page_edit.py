"""A page's widgets come from the panel only while editing on the panel is allowed (custom_components/panelos/const.py:
panel_edit, checked by panelos/page_set).

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
    spec = importlib.util.spec_from_file_location("panelos_const_page_edit_under_test", CONST)
    module = importlib.util.module_from_spec(spec)
    write_bytecode = sys.dont_write_bytecode
    try:
        sys.dont_write_bytecode = True
        spec.loader.exec_module(module)
    finally:
        sys.dont_write_bytecode = write_bytecode
    return module


const = _load_const()


class PanelEdit(unittest.TestCase):
    def test_the_widgets_are_an_edit(self) -> None:
        self.assertTrue(const.panel_edit({"items": []}))
        self.assertTrue(const.panel_edit({"items": [{"kind": "clock"}], "scene": 1}))
        # removing them too
        self.assertTrue(const.panel_edit({"items": None}))

    def test_menus_and_looks_are_not(self) -> None:
        # what the panel's menus set on any page, editing allowed or not
        for options in (
            {"last_view": 3},
            {"hidden_calendars": ["calendar.films"]},
            {"scene": 2},
            {"cars": False},
            {"functions": ["timer", "alarm"]},
            {"fit": "cover"},
            {},
        ):
            with self.subTest(options=options):
                self.assertFalse(const.panel_edit(options))

    def test_no_options(self) -> None:
        for options in (None, [], "items"):
            with self.subTest(options=options):
                self.assertFalse(const.panel_edit(options))


if __name__ == "__main__":
    unittest.main()
