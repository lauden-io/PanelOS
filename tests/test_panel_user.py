"""Who may act as a panel (custom_components/panelos/panel.py: may_act and the reported user).

Runs with the standard library only: python3 -m unittest discover -s tests

panel.py is loaded against small stand-ins for the Home Assistant modules it imports (config entries, the state
machine, the entity registry, the dispatcher, the users); sys.modules is put back afterwards, so a real Home
Assistant in the same run is not touched.
"""

from __future__ import annotations

import asyncio
import importlib
from pathlib import Path
import sys
import types
import unittest

PACKAGE = "panelos_under_test"
PACKAGE_DIR = Path(__file__).resolve().parents[1] / "custom_components" / "panelos"
SIGNAL_PANEL_USER = "panelos_panel_user"


def _identity(func):
    return func


def _stub_modules() -> dict[str, types.ModuleType]:
    """The Home Assistant modules panel.py imports, with what it uses at import time and in the tested functions."""

    def module(name: str, package: bool = False, **attrs) -> types.ModuleType:
        m = types.ModuleType(name)
        m.__dict__.update(attrs)
        if package:
            m.__path__ = []
        return m

    class Placeholder:
        def __init__(self, *args, **kwargs) -> None:
            pass

    def dispatcher_send(hass, signal, *args) -> None:
        hass.sent.append((signal, args))

    entity_registry = module(
        "homeassistant.helpers.entity_registry",
        async_get=lambda hass: hass.entity_registry,
        async_entries_for_config_entry=lambda registry, entry_id: [e for e in registry if e.config_entry_id == entry_id],
        EVENT_ENTITY_REGISTRY_UPDATED="entity_registry_updated",
    )
    helpers = module(
        "homeassistant.helpers",
        package=True,
        area_registry=module("homeassistant.helpers.area_registry"),
        device_registry=module("homeassistant.helpers.device_registry"),
        entity_registry=entity_registry,
    )
    modules = [
        module("homeassistant", package=True),
        module("homeassistant.auth", package=True),
        module("homeassistant.auth.models", User=Placeholder),
        module("homeassistant.config_entries", ConfigEntry=Placeholder),
        module("homeassistant.core", Event=Placeholder, HomeAssistant=Placeholder, callback=_identity),
        module("homeassistant.exceptions", HomeAssistantError=Exception),
        module("homeassistant.components", package=True),
        module("homeassistant.components.intent", package=True),
        module("homeassistant.components.intent.const", TIMER_DATA="intent_timers"),
        helpers,
        helpers.area_registry,
        helpers.device_registry,
        entity_registry,
        module("homeassistant.helpers.dispatcher", async_dispatcher_connect=Placeholder, async_dispatcher_send=dispatcher_send),
        module("homeassistant.helpers.event", async_call_later=Placeholder, async_track_state_change_event=Placeholder),
        module("homeassistant.helpers.entity", Entity=Placeholder, entity_sources=Placeholder),
        module("homeassistant.helpers.entity_platform", AddEntitiesCallback=Placeholder),
        module("homeassistant.helpers.start", async_at_started=Placeholder),
        module("homeassistant.helpers.storage", Store=Placeholder),
        module("homeassistant.helpers.typing", UNDEFINED=object()),
    ]
    return {m.__name__: m for m in modules}


def _load_panel() -> types.ModuleType:
    stubs = _stub_modules()
    names = [*stubs, PACKAGE, f"{PACKAGE}.panel", f"{PACKAGE}.const", f"{PACKAGE}.resources"]
    saved = {name: sys.modules.get(name) for name in names}
    write_bytecode = sys.dont_write_bytecode
    try:
        sys.dont_write_bytecode = True
        sys.modules.update(stubs)
        package = types.ModuleType(PACKAGE)
        package.__path__ = [str(PACKAGE_DIR)]
        sys.modules[PACKAGE] = package
        return importlib.import_module(f"{PACKAGE}.panel")
    finally:
        sys.dont_write_bytecode = write_bytecode
        for name, old in saved.items():
            if old is None:
                sys.modules.pop(name, None)
            else:
                sys.modules[name] = old


panel = _load_panel()

USER_A = "0123456789abcdef0123456789abcdef"
USER_B = "fedcba9876543210fedcba9876543210"
ADMIN = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
MAC = "AA:BB:CC:DD:EE:FF"


class User:
    def __init__(self, user_id: str, name: str, is_admin: bool = False) -> None:
        self.id, self.name, self.is_admin = user_id, name, is_admin


class Entry:
    def __init__(self, domain: str, entry_id: str, data: dict) -> None:
        self.domain, self.entry_id, self.data, self.options, self.title = domain, entry_id, dict(data), {}, "Panel"
        self.tasks: list = []

    def async_create_task(self, hass, coro) -> None:
        self.tasks.append(coro)


class ConfigEntries:
    def __init__(self, entries: list[Entry]) -> None:
        self.entries = entries

    def async_entries(self, domain: str) -> list[Entry]:
        return [e for e in self.entries if e.domain == domain]

    def async_update_entry(self, entry: Entry, data: dict | None = None, options: dict | None = None) -> None:
        if data is not None:
            entry.data = dict(data)
        if options is not None:
            entry.options = dict(options)


class State:
    def __init__(self, state: str) -> None:
        self.state = state


class States:
    def __init__(self) -> None:
        self.states: dict[str, State] = {}

    def get(self, entity_id: str) -> State | None:
        return self.states.get(entity_id)


class Auth:
    def __init__(self, users: list[User]) -> None:
        self.users = {u.id: u for u in users}

    async def async_get_user(self, user_id: str) -> User | None:
        return self.users.get(user_id)


class RegistryEntry:
    def __init__(self, entity_id: str, platform: str, unique_id: str, config_entry_id: str) -> None:
        self.entity_id, self.platform, self.unique_id, self.config_entry_id = entity_id, platform, unique_id, config_entry_id
        self.domain = entity_id.split(".", 1)[0]


class Hass:
    def __init__(self, entries: list[Entry], users: list[User], registry: list[RegistryEntry]) -> None:
        self.config_entries = ConfigEntries(entries)
        self.states = States()
        self.auth = Auth(users)
        self.entity_registry = registry
        self.sent: list[tuple[str, tuple]] = []


SENSOR = "sensor.voice_panel_user"


def setup(key: bool = True, sensor: bool = True, unique_id: str = f"{MAC}/0/text_sensor/Panel user"):
    """A panel entry, its ESPHome entry (with or without the API key) and the Panel user sensor (or none)."""
    users = [User(USER_A, "Panel"), User(USER_B, "Guest"), User(ADMIN, "Admin", is_admin=True)]
    entry = Entry("panelos", "panel-entry", {"panel_id": "nspanel-pro"})
    voice = Entry("esphome", "voice-entry", {"device_name": "nspanel-pro", **({"noise_psk": "key"} if key else {})})
    registry = [
        RegistryEntry("sensor.voice_wake_level", "esphome", f"{MAC}/0/sensor/Wake level", voice.entry_id),
        *([RegistryEntry(SENSOR, "esphome", unique_id, voice.entry_id)] if sensor else []),
    ]
    hass = Hass([entry, voice], users, registry)
    return hass, entry, {u.id: u for u in users}


def report(hass: Hass, state: str) -> None:
    hass.states.states[SENSOR] = State(state)


def run_tasks(entry: Entry) -> None:
    tasks, entry.tasks = entry.tasks, []
    for coro in tasks:
        asyncio.run(coro)


def kept_id(entry: Entry) -> str | None:
    return (panel.panel_user(entry) or {}).get("id")


class WithoutReport(unittest.TestCase):
    """A panel that does not report its user (an older panel, no sensor): the first user that connects is kept."""

    def test_first_user_is_kept_and_others_refused(self) -> None:
        hass, entry, u = setup(sensor=False)
        self.assertIsNone(panel.reported_user(hass, entry))
        self.assertTrue(panel.may_act(hass, entry, u[USER_B]))
        self.assertEqual(panel.panel_user(entry), {"id": USER_B, "name": "Guest"})
        self.assertIn((SIGNAL_PANEL_USER, (entry.entry_id, USER_B)), hass.sent)
        self.assertFalse(panel.may_act(hass, entry, u[USER_A]))
        self.assertTrue(panel.may_act(hass, entry, u[USER_B]))

    def test_administrators_pass_and_are_not_kept(self) -> None:
        hass, entry, u = setup(sensor=False)
        self.assertTrue(panel.may_act(hass, entry, u[ADMIN]))
        self.assertIsNone(panel.panel_user(entry))

    def test_renamed_user_is_shown_with_the_new_name(self) -> None:
        hass, entry, u = setup(sensor=False)
        panel.may_act(hass, entry, u[USER_A])
        u[USER_A].name = "Hall panel"
        self.assertTrue(panel.may_act(hass, entry, u[USER_A]))
        self.assertEqual(panel.panel_user(entry), {"id": USER_A, "name": "Hall panel"})

    def test_forget_lets_the_next_user_be_kept(self) -> None:
        hass, entry, u = setup(sensor=False)
        panel.may_act(hass, entry, u[USER_B])
        self.assertTrue(panel.forget_panel_user(hass, entry))
        self.assertIsNone(panel.panel_user(entry))
        self.assertIn((SIGNAL_PANEL_USER, (entry.entry_id, None)), hass.sent)
        self.assertTrue(panel.may_act(hass, entry, u[USER_A]))
        self.assertEqual(kept_id(entry), USER_A)

    def test_unknown_unavailable_or_odd_sensor_counts_as_no_report(self) -> None:
        for state in ("unknown", "unavailable", "", "not a user id", USER_A.upper(), USER_A + "0"):
            with self.subTest(state=state):
                hass, entry, u = setup()
                report(hass, state)
                self.assertIsNone(panel.reported_user(hass, entry))
                self.assertTrue(panel.may_act(hass, entry, u[USER_B]))
                self.assertEqual(kept_id(entry), USER_B)

    def test_esphome_entry_without_a_key_does_not_count(self) -> None:
        hass, entry, u = setup(key=False)
        report(hass, USER_A)
        self.assertIsNone(panel.reported_user(hass, entry))
        self.assertTrue(panel.may_act(hass, entry, u[USER_B]))
        self.assertEqual(kept_id(entry), USER_B)

    def test_no_esphome_entry(self) -> None:
        hass, entry, u = setup()
        hass.config_entries.entries = [entry]
        self.assertIsNone(panel.user_sensor(hass, entry))
        self.assertIsNone(panel.reported_user(hass, entry))
        self.assertTrue(panel.may_act(hass, entry, u[USER_B]))

    def test_no_report_changes_nothing(self) -> None:
        hass, entry, u = setup()
        panel.may_act(hass, entry, u[USER_B])
        report(hass, "unavailable")
        panel.user_reported(hass, entry)
        self.assertEqual(entry.tasks, [])
        self.assertEqual(kept_id(entry), USER_B)


class WithReport(unittest.TestCase):
    """A panel that reports its user: only that user (and administrators) may act as it."""

    def test_only_the_reported_user_may_act(self) -> None:
        hass, entry, u = setup()
        report(hass, USER_A)
        self.assertEqual(panel.user_sensor(hass, entry), SENSOR)
        self.assertEqual(panel.reported_user(hass, entry), USER_A)
        # another user that connects first is not kept
        self.assertFalse(panel.may_act(hass, entry, u[USER_B]))
        self.assertIsNone(panel.panel_user(entry))
        self.assertTrue(panel.may_act(hass, entry, u[USER_A]))
        self.assertEqual(panel.panel_user(entry), {"id": USER_A, "name": "Panel"})
        self.assertFalse(panel.may_act(hass, entry, u[USER_B]))
        self.assertTrue(panel.may_act(hass, entry, u[ADMIN]))

    def test_older_unique_id_is_found_too(self) -> None:
        hass, entry, u = setup(unique_id=f"{MAC.replace(':', '').lower()}-text_sensor-panel_user")
        report(hass, USER_A)
        self.assertEqual(panel.reported_user(hass, entry), USER_A)
        self.assertFalse(panel.may_act(hass, entry, u[USER_B]))

    def test_other_sensors_do_not_count(self) -> None:
        hass, entry, u = setup(unique_id=f"{MAC}/0/text_sensor/IP address")
        report(hass, USER_A)
        self.assertIsNone(panel.reported_user(hass, entry))

    def test_a_user_kept_first_gives_way_to_the_reported_one(self) -> None:
        hass, entry, u = setup()
        # kept before the panel reported (an older app, or the sensor still unavailable)
        self.assertTrue(panel.may_act(hass, entry, u[USER_B]))
        report(hass, USER_A)
        self.assertFalse(panel.may_act(hass, entry, u[USER_B]))
        self.assertTrue(panel.may_act(hass, entry, u[USER_A]))
        self.assertEqual(kept_id(entry), USER_A)
        self.assertEqual(hass.sent[-1], (SIGNAL_PANEL_USER, (entry.entry_id, USER_A)))

    def test_the_report_alone_moves_the_binding(self) -> None:
        hass, entry, u = setup()
        panel.may_act(hass, entry, u[USER_B])
        report(hass, USER_A)
        panel.user_reported(hass, entry)
        run_tasks(entry)
        self.assertEqual(panel.panel_user(entry), {"id": USER_A, "name": "Panel"})
        # the links of the user kept before end (SIGNAL_PANEL_USER with the new user)
        self.assertEqual(hass.sent[-1], (SIGNAL_PANEL_USER, (entry.entry_id, USER_A)))

    def test_forget_keeps_the_reported_user(self) -> None:
        hass, entry, u = setup()
        report(hass, USER_A)
        panel.may_act(hass, entry, u[USER_A])
        sent = len(hass.sent)
        self.assertFalse(panel.forget_panel_user(hass, entry))
        self.assertEqual(kept_id(entry), USER_A)
        self.assertEqual(len(hass.sent), sent)

    def test_while_the_sensor_is_away_the_kept_user_still_holds(self) -> None:
        hass, entry, u = setup()
        report(hass, USER_A)
        panel.may_act(hass, entry, u[USER_A])
        report(hass, "unavailable")
        self.assertFalse(panel.may_act(hass, entry, u[USER_B]))
        self.assertTrue(panel.may_act(hass, entry, u[USER_A]))


class ReportChanges(unittest.TestCase):
    """The panel paired again with another user: the binding moves by itself."""

    def test_new_user_takes_over_and_old_links_end(self) -> None:
        hass, entry, u = setup()
        report(hass, USER_A)
        panel.may_act(hass, entry, u[USER_A])
        report(hass, USER_B)
        panel.user_reported(hass, entry)
        self.assertEqual(len(entry.tasks), 1)
        run_tasks(entry)
        self.assertEqual(panel.panel_user(entry), {"id": USER_B, "name": "Guest"})
        self.assertEqual(hass.sent[-1], (SIGNAL_PANEL_USER, (entry.entry_id, USER_B)))
        self.assertFalse(panel.may_act(hass, entry, u[USER_A]))
        self.assertTrue(panel.may_act(hass, entry, u[USER_B]))

    def test_new_user_subscribing_before_the_change_is_handled(self) -> None:
        hass, entry, u = setup()
        report(hass, USER_A)
        panel.may_act(hass, entry, u[USER_A])
        # the panel's new user connects before the sensor says so: refused (the panel asks again)
        self.assertFalse(panel.may_act(hass, entry, u[USER_B]))
        report(hass, USER_B)
        self.assertTrue(panel.may_act(hass, entry, u[USER_B]))
        self.assertEqual(kept_id(entry), USER_B)
        # the sensor's change then finds it kept already: nothing more to do
        panel.user_reported(hass, entry)
        self.assertEqual(entry.tasks, [])

    def test_same_user_again_changes_nothing(self) -> None:
        hass, entry, u = setup()
        report(hass, USER_A)
        panel.may_act(hass, entry, u[USER_A])
        sent = len(hass.sent)
        panel.user_reported(hass, entry)
        self.assertEqual(entry.tasks, [])
        self.assertEqual(len(hass.sent), sent)

    def test_a_user_home_assistant_does_not_know(self) -> None:
        hass, entry, u = setup()
        report(hass, USER_A)
        panel.may_act(hass, entry, u[USER_A])
        gone = "0" * 32
        report(hass, gone)
        panel.user_reported(hass, entry)
        run_tasks(entry)
        # nobody but administrators: the links end, the kept user stays as it was
        self.assertEqual(hass.sent[-1], (SIGNAL_PANEL_USER, (entry.entry_id, gone)))
        self.assertEqual(kept_id(entry), USER_A)
        self.assertFalse(panel.may_act(hass, entry, u[USER_A]))
        self.assertFalse(panel.may_act(hass, entry, u[USER_B]))
        self.assertTrue(panel.may_act(hass, entry, u[ADMIN]))

    def test_report_changed_again_before_the_lookup(self) -> None:
        hass, entry, u = setup()
        report(hass, USER_A)
        panel.may_act(hass, entry, u[USER_A])
        report(hass, USER_B)
        panel.user_reported(hass, entry)
        report(hass, USER_A)
        run_tasks(entry)
        self.assertEqual(kept_id(entry), USER_A)


if __name__ == "__main__":
    unittest.main()
