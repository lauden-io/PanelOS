"""Panel IDs and two panels side by side (custom_components/panelos/panel.py and llm.py).

Runs with the standard library only: python3 -m unittest discover -s tests

A panel's ID is the short name given to it at installation (`nspanel-pro` for a panel without one). These tests cover
the name rule, which entry, ESPHome device and user each of two panels gets, which requests reach which panel, the
panels waiting for an entry, and which panel the voice tools pick by room or name. panel.py comes from
test_panel_user (loaded against small stand-ins for Home Assistant); llm.py is loaded here the same way.
"""

from __future__ import annotations

import importlib
import sys
import time
import types
import unittest

from test_panel_user import PACKAGE, PACKAGE_DIR, Entry, Hass, RegistryEntry, State, User, panel

USER_A = "0123456789abcdef0123456789abcdef"
USER_B = "fedcba9876543210fedcba9876543210"
ADMIN = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
MAC_A = "aa:bb:cc:dd:ee:01"
MAC_B = "aa:bb:cc:dd:ee:02"
MAC_OTHER = "11:22:33:44:55:66"


def panel_entry(entry_id: str, panel_id: str, mac: str | None = None, title: str | None = None) -> Entry:
    data = {"panel_id": panel_id, **({"device": {"mac": mac}} if mac else {})}
    entry = Entry("panelos", entry_id, data)
    entry.title = title or f"PanelOS ({panel_id})"
    return entry


def esphome_entry(entry_id: str, device_name: str, mac: str, key: bool = True) -> Entry:
    entry = Entry("esphome", entry_id, {"device_name": device_name, **({"noise_psk": "key"} if key else {})})
    entry.unique_id = mac
    return entry


class PanelIdRule(unittest.TestCase):
    """The name rule: the same as on the panel (app/src/panel_id.rs)."""

    def test_names(self) -> None:
        for name in ("kitchen", "nspanel-pro", "living-room", "hall2", "2nd", "a", "a" * 31):
            self.assertEqual(panel.checked_panel_id(name), name)

    def test_written_loosely(self) -> None:
        self.assertEqual(panel.checked_panel_id("  Kitchen "), "kitchen")
        self.assertEqual(panel.checked_panel_id("HALL"), "hall")

    def test_not_names(self) -> None:
        for bad in ("", "   ", "living room", "kitchen_1", "küche", "kitchen.local", "-kitchen", "kitchen-", "-", "a" * 32, "a/b"):
            self.assertIsNone(panel.checked_panel_id(bad), bad)
        for bad in (None, 7, ["kitchen"]):
            self.assertIsNone(panel.checked_panel_id(bad))


class FindEntry(unittest.TestCase):
    def test_each_panel_its_own_entry(self) -> None:
        old, kitchen = panel_entry("e1", "nspanel-pro"), panel_entry("e2", "kitchen")
        hass = Hass([old, kitchen], [], [])
        self.assertIs(panel.find_entry(hass, "nspanel-pro"), old)
        self.assertIs(panel.find_entry(hass, "kitchen"), kitchen)
        self.assertIsNone(panel.find_entry(hass, "hall"))
        self.assertIsNone(panel.find_entry(hass, ""))

    def test_a_named_panel_never_takes_another_panels_entry(self) -> None:
        hass = Hass([panel_entry("e1", "nspanel-pro")], [], [])
        self.assertIsNone(panel.find_entry(hass, "kitchen"))

    def test_a_panel_without_a_name_keeps_its_single_entry(self) -> None:
        # an entry set up with another Panel ID before panels had names: the panel (nspanel-pro) keeps finding it
        only = panel_entry("e1", "wallpanel")
        hass = Hass([only], [], [])
        self.assertIs(panel.find_entry(hass, "nspanel-pro"), only)
        self.assertIs(panel.find_entry(hass, ""), only)
        # not once there are two entries
        hass = Hass([only, panel_entry("e2", "kitchen")], [], [])
        self.assertIsNone(panel.find_entry(hass, "nspanel-pro"))


class Serves(unittest.TestCase):
    def test_requests_reach_their_panel_only(self) -> None:
        hass = Hass([panel_entry("e1", "nspanel-pro"), panel_entry("e2", "kitchen")], [], [])
        self.assertTrue(panel.serves(hass, "kitchen", "kitchen"))
        self.assertTrue(panel.serves(hass, "kitchen", ""))
        self.assertFalse(panel.serves(hass, "kitchen", "nspanel-pro"))
        self.assertFalse(panel.serves(hass, "nspanel-pro", "kitchen"))
        self.assertTrue(panel.serves(hass, "nspanel-pro", "nspanel-pro"))

    def test_a_panel_on_an_entry_with_another_id(self) -> None:
        hass = Hass([panel_entry("e1", "wallpanel")], [], [])
        self.assertTrue(panel.serves(hass, "nspanel-pro", "wallpanel"))
        self.assertTrue(panel.serves(hass, "nspanel-pro", "nspanel-pro"))
        self.assertFalse(panel.serves(hass, "nspanel-pro", "kitchen"))


def two_panels(mac_known: bool = True):
    """Two panels, each with its ESPHome entry (with a key), its Panel user sensor and its user."""
    users = [User(USER_A, "Panel A"), User(USER_B, "Panel B"), User(ADMIN, "Admin", is_admin=True)]
    a = panel_entry("pa", "nspanel-pro", MAC_A if mac_known else None)
    b = panel_entry("pb", "kitchen", MAC_B if mac_known else None)
    voice_a, voice_b = esphome_entry("va", "nspanel-pro", MAC_A), esphome_entry("vb", "kitchen", MAC_B)
    registry = [
        RegistryEntry("sensor.panel_a_user", "esphome", f"{MAC_A.upper()}/0/text_sensor/Panel user", "va"),
        RegistryEntry("sensor.panel_b_user", "esphome", f"{MAC_B.upper()}/0/text_sensor/Panel user", "vb"),
    ]
    hass = Hass([a, b, voice_a, voice_b], users, registry)
    hass.states.states["sensor.panel_a_user"] = State(USER_A)
    hass.states.states["sensor.panel_b_user"] = State(USER_B)
    return hass, a, b, {u.id: u for u in users}


class TwoPanels(unittest.TestCase):
    def test_each_panel_its_voice_satellite(self) -> None:
        for mac_known in (True, False):
            hass, a, b, _ = two_panels(mac_known)
            self.assertEqual(panel.voice_entry(hass, "nspanel-pro", panel.panel_mac(a)).entry_id, "va")
            self.assertEqual(panel.voice_entry(hass, "kitchen", panel.panel_mac(b)).entry_id, "vb")
            self.assertEqual(panel.user_sensor(hass, a), "sensor.panel_a_user")
            self.assertEqual(panel.user_sensor(hass, b), "sensor.panel_b_user")

    def test_each_panel_its_user(self) -> None:
        hass, a, b, users = two_panels()
        self.assertEqual(panel.reported_user(hass, a), USER_A)
        self.assertEqual(panel.reported_user(hass, b), USER_B)
        self.assertTrue(panel.may_act(hass, a, users[USER_A]))
        self.assertFalse(panel.may_act(hass, a, users[USER_B]))
        self.assertTrue(panel.may_act(hass, b, users[USER_B]))
        self.assertFalse(panel.may_act(hass, b, users[USER_A]))
        self.assertTrue(panel.may_act(hass, a, users[ADMIN]))
        self.assertTrue(panel.may_act(hass, b, users[ADMIN]))

    def test_another_esphome_device_with_the_name_is_not_the_panel(self) -> None:
        # an ESPHome device of another MAC address that happens to be called like the panel, listed first
        hass, _a, b, _ = two_panels()
        hass.config_entries.entries.insert(0, esphome_entry("other", "kitchen", MAC_OTHER))
        self.assertEqual(panel.voice_entry(hass, "kitchen", panel.panel_mac(b)).entry_id, "vb")
        # before the panel's MAC address is kept: the entry whose device names its user, else the first by name
        self.assertEqual(panel.voice_entry(hass, "kitchen").entry_id, "vb")
        hass.states.states["sensor.panel_b_user"] = State("unavailable")
        self.assertEqual(panel.voice_entry(hass, "kitchen").entry_id, "other")
        self.assertEqual(panel.voice_entry(hass, "kitchen", panel.panel_mac(b)).entry_id, "vb")

    def test_an_esphome_entry_without_a_mac_unique_id_still_counts(self) -> None:
        hass = Hass([panel_entry("pa", "nspanel-pro", MAC_A), esphome_entry("va", "nspanel-pro", "nspanel-pro")], [], [])
        self.assertEqual(panel.voice_entry(hass, "nspanel-pro", MAC_A).entry_id, "va")


class Waiting(unittest.TestCase):
    def test_panels_without_an_entry_are_offered(self) -> None:
        hass = Hass([panel_entry("e1", "nspanel-pro")], [], [])
        hass.data = {}
        panel.note_waiting(hass, "kitchen")
        panel.note_waiting(hass, "hall")
        panel.note_waiting(hass, "living room")  # not a panel's name: left out
        panel.note_waiting(hass, "")
        self.assertEqual(sorted(panel.waiting_panels(hass)), ["hall", "kitchen"])
        # the newest first (the times set here: two calls of the clock can give the same time)
        now = time.monotonic()
        panel.waiting(hass).update({"kitchen": now - 2, "hall": now - 1})
        self.assertEqual(panel.waiting_panels(hass), ["hall", "kitchen"])
        panel.waiting(hass)["kitchen"] = now
        self.assertEqual(panel.waiting_panels(hass), ["kitchen", "hall"])
        # the page is told once per new panel
        self.assertEqual([signal for signal, _ in hass.sent], [panel.SIGNAL_PAGE, panel.SIGNAL_PAGE])

    def test_added_or_old_ones_go(self) -> None:
        hass = Hass([panel_entry("e1", "nspanel-pro")], [], [])
        hass.data = {}
        panel.note_waiting(hass, "kitchen")
        panel.note_waiting(hass, "hall")
        panel.waiting(hass)["hall"] = time.monotonic() - panel.WAITING_FOR - 1
        hass.config_entries.entries.append(panel_entry("e2", "kitchen"))
        self.assertEqual(panel.waiting_panels(hass), [])

    def test_a_handful_at_most(self) -> None:
        hass = Hass([], [], [])
        hass.data = {}
        for i in range(20):
            panel.note_waiting(hass, f"panel{i}")
        self.assertEqual(len(panel.waiting_panels(hass)), 8)
        self.assertIn("panel19", panel.waiting_panels(hass))


# ---------------------------------------------------------------- the voice tools pick a panel by room or name


def _load_llm() -> types.ModuleType:
    def module(name: str, package: bool = False, **attrs) -> types.ModuleType:
        m = types.ModuleType(name)
        m.__dict__.update(attrs)
        if package:
            m.__path__ = []
        return m

    class Anything:
        def __init__(self, *args, **kwargs) -> None:
            pass

    def schema_key(key, description=None):
        return key

    area_registry = module("homeassistant.helpers.area_registry", async_get=lambda hass: hass.areas)
    device_registry = module(
        "homeassistant.helpers.device_registry",
        async_get=lambda hass: hass.devices,
        async_entries_for_config_entry=lambda devices, entry_id: [d for d in devices if d.config_entry_id == entry_id],
    )
    config_validation = module(
        "homeassistant.helpers.config_validation", string=str, positive_int=int, boolean=bool
    )
    helpers = module(
        "homeassistant.helpers",
        package=True,
        area_registry=area_registry,
        device_registry=device_registry,
        config_validation=config_validation,
    )
    stubs = [
        module("homeassistant", package=True),
        module("homeassistant.components", package=True),
        module("homeassistant.components.intent", package=True),
        module("homeassistant.components.intent.const", TIMER_DATA="intent_timers"),
        module("homeassistant.components.llm", LLMTools=Anything),
        module("homeassistant.core", HomeAssistant=Anything, callback=lambda f: f),
        helpers,
        area_registry,
        device_registry,
        config_validation,
        module("homeassistant.helpers.dispatcher", async_dispatcher_send=lambda *a: None),
        module(
            "homeassistant.helpers.llm",
            LLM_API_ASSIST="assist",
            LLMContext=Anything,
            Tool=object,
            ToolAnnotations=Anything,
            ToolInput=Anything,
            ToolResult=Anything,
        ),
        module("probatio", Schema=Anything, Required=schema_key, Optional=schema_key),
    ]
    stubs = {m.__name__: m for m in stubs}
    names = [*stubs, PACKAGE, f"{PACKAGE}.llm", f"{PACKAGE}.const"]
    saved = {name: sys.modules.get(name) for name in names}
    write_bytecode = sys.dont_write_bytecode
    try:
        sys.dont_write_bytecode = True
        sys.modules.update(stubs)
        package = types.ModuleType(PACKAGE)
        package.__path__ = [str(PACKAGE_DIR)]
        sys.modules[PACKAGE] = package
        return importlib.import_module(f"{PACKAGE}.llm")
    finally:
        sys.dont_write_bytecode = write_bytecode
        for name, old in saved.items():
            if old is None:
                sys.modules.pop(name, None)
            else:
                sys.modules[name] = old


llm = _load_llm()


class Area:
    def __init__(self, name: str, aliases: set[str] | None = None) -> None:
        self.name, self.aliases = name, aliases or set()


class Areas(dict):
    def async_get_area(self, area_id: str) -> Area | None:
        return self.get(area_id)


class Device:
    def __init__(self, config_entry_id: str, name: str, area_id: str | None) -> None:
        self.config_entry_id, self.name, self.name_by_user, self.area_id = config_entry_id, name, None, area_id


class FakePanel:
    def __init__(self, entry: Entry, timer_device: str | None) -> None:
        self.entry, self.panel_id, self._timer_device = entry, entry.data["panel_id"], timer_device
        entry.runtime_data = self

    def timer_device_id(self) -> str | None:
        return self._timer_device


def tool_home(areas_of: dict[str, str | None]):
    """One panel per id, its device in the given area (an area id is its name in lower case)."""
    hass = Hass([], [], [])
    hass.areas = Areas()
    hass.devices = []
    panels = {}
    for i, (panel_id, area) in enumerate(areas_of.items()):
        entry = panel_entry(f"e{i}", panel_id)
        hass.config_entries.entries.append(entry)
        label = "NSPanel Pro" if panel_id == "nspanel-pro" else f"NSPanel Pro ({panel_id})"
        if area:
            hass.areas[area.lower()] = Area(area)
        hass.devices.append(Device(entry.entry_id, label, area.lower() if area else None))
        panels[panel_id] = FakePanel(entry, f"voice-{panel_id}")
    return hass, panels


class VoiceToolsPickAPanel(unittest.TestCase):
    def test_by_room(self) -> None:
        hass, panels = tool_home({"nspanel-pro": "Hall", "kitchen": "Kitchen"})
        self.assertIs(llm._find(hass, "kitchen")[0], panels["kitchen"])
        self.assertIs(llm._find(hass, "the kitchen panel")[0], panels["kitchen"])
        self.assertIs(llm._find(hass, "Hall")[0], panels["nspanel-pro"])
        self.assertEqual(sorted(llm._find(hass, "garage")[1]), ["Hall", "Kitchen"])
        self.assertIsNone(llm._find(hass, "garage")[0])

    def test_by_name_without_a_room(self) -> None:
        hass, panels = tool_home({"nspanel-pro": None, "kitchen": None})
        self.assertIs(llm._find(hass, "kitchen")[0], panels["kitchen"])
        # "the panel" with two panels: the tool asks which one
        self.assertIsNone(llm._find(hass, "")[0])

    def test_two_panels_in_one_room_are_not_guessed(self) -> None:
        hass, panels = tool_home({"nspanel-pro": "Kitchen", "kitchen2": "Kitchen"})
        self.assertIsNone(llm._find(hass, "the kitchen panel")[0])
        self.assertIs(llm._find(hass, "kitchen2")[0], panels["kitchen2"])


if __name__ == "__main__":
    unittest.main()
