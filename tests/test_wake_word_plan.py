"""Home Assistant's wake word selects follow the panel's wake words (custom_components/panelos/wake_word_plan.py).

Runs with the standard library only: python3 -m unittest discover -s tests

wake_word_plan.py imports nothing from Home Assistant, so it is loaded by its path, outside the package.
"""

from __future__ import annotations

import importlib.util
from pathlib import Path
import sys
import unittest

PLAN = Path(__file__).resolve().parents[1] / "custom_components" / "panelos" / "wake_word_plan.py"


def _load():
    spec = importlib.util.spec_from_file_location("panelos_wake_word_plan_under_test", PLAN)
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
NAMES = {"hey_mycroft": "Hey Mycroft", "hey_jarvis": "Hey Jarvis", "okay_nabu": "Okay Nabu", "alexa": "Alexa"}
NONE = m.NO_WAKE_WORD


class WakeWordPlan(unittest.TestCase):
    def test_in_line_nothing_to_do(self) -> None:
        p = m.plan(["hey_mycroft", "hey_jarvis"], ["hey_mycroft", "hey_jarvis"], NAMES, 2, ["Hey Mycroft", "Hey Jarvis"])
        self.assertTrue(p.nothing)

    def test_two_words_restored_as_none(self) -> None:
        # what Home Assistant leaves after a restart: both selects at "No wake word"
        p = m.plan(["hey_mycroft", "hey_jarvis"], ["hey_mycroft", "hey_jarvis"], NAMES, 2, [NONE, NONE])
        self.assertFalse(p.push)
        self.assertEqual(p.selects, [(0, "Hey Mycroft"), (1, "Hey Jarvis")])

    def test_second_word_changed_on_the_page(self) -> None:
        # Home Assistant set the second select back after the page changed the word
        p = m.plan(["hey_mycroft", "okay_nabu"], ["hey_mycroft", "okay_nabu"], NAMES, 2, ["Hey Mycroft", NONE])
        self.assertEqual(p.selects, [(1, "Okay Nabu")])

    def test_changed_on_the_panel_itself(self) -> None:
        # Home Assistant's configuration does not know it yet: it takes the panel's words first
        p = m.plan(["alexa", "hey_jarvis"], ["hey_mycroft", "hey_jarvis"], NAMES, 2, ["Hey Mycroft", "Hey Jarvis"])
        self.assertTrue(p.push)
        self.assertEqual(p.target, ["alexa", "hey_jarvis"])
        self.assertEqual(p.selects, [(0, "Alexa")])

    def test_one_word(self) -> None:
        p = m.plan(["okay_nabu"], ["okay_nabu"], NAMES, 2, ["Okay Nabu", "Hey Jarvis"])
        self.assertEqual(p.selects, [(1, NONE)])

    def test_no_word(self) -> None:
        p = m.plan([], [], NAMES, 2, [NONE, NONE])
        self.assertTrue(p.nothing)

    def test_an_older_panel_follows_home_assistant(self) -> None:
        # no report: Home Assistant's own configuration counts
        p = m.plan(None, ["hey_mycroft", "hey_jarvis"], NAMES, 2, [NONE, NONE])
        self.assertFalse(p.push)
        self.assertEqual(p.selects, [(0, "Hey Mycroft"), (1, "Hey Jarvis")])

    def test_unknown_and_twice_left_out(self) -> None:
        p = m.plan(["gone_model", "hey_jarvis", "hey_jarvis"], ["hey_jarvis"], NAMES, 2, ["Hey Jarvis", NONE])
        self.assertEqual(p.target, ["hey_jarvis"])
        self.assertTrue(p.nothing)

    def test_no_more_than_the_selects_and_the_satellite_take(self) -> None:
        p = m.plan(["hey_mycroft", "hey_jarvis", "alexa"], ["hey_mycroft"], NAMES, 3, ["Hey Mycroft"])
        self.assertEqual(p.target, ["hey_mycroft"])
        self.assertTrue(p.nothing)
        p = m.plan(["hey_mycroft", "hey_jarvis"], ["hey_mycroft"], NAMES, 1, ["Hey Mycroft", NONE])
        self.assertEqual(p.target, ["hey_mycroft"])
        self.assertTrue(p.nothing)


if __name__ == "__main__":
    unittest.main()
