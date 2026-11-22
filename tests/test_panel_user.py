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
        self.unique_id: str | None = None
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


# ---------------------------------------------------------------- the MAC address a report carries

OTHER_MAC = "11:22:33:44:55:66"
NEW_MAC = "AA:BB:CC:DD:EE:02"
NEW_SENSOR = "sensor.new_voice_panel_user"


def voice_of(hass: Hass) -> Entry:
    return next(e for e in hass.config_entries.entries if e.domain == "esphome")


def with_mac() -> tuple[Hass, Entry, dict[str, User]]:
    """`setup()` with the ESPHome entry's unique id as Home Assistant's ESPHome integration writes it (the MAC)."""
    hass, entry, u = setup()
    voice_of(hass).unique_id = MAC.lower()
    return hass, entry, u


def kept_mac(entry: Entry) -> str | None:
    return (entry.data.get("device") or {}).get("mac")


def report_device(hass: Hass, entry: Entry, user: User, mac: str | None) -> None:
    """A device report of the connection that holds the panel's subscription (Panel.apply_report), without the device
    registry."""
    p = object.__new__(panel.Panel)
    p.hass, p.entry, p.panel_id = hass, entry, "nspanel-pro"
    p.states, p.options, p.pending_alarms = {}, {}, None
    p._register_device = lambda info: None
    p._link_voice_device = lambda voice: None
    p.apply_report({"device": {"name": "NSPanel Pro", "model": "NSPanel Pro", **({"mac": mac} if mac else {})}}, user)


def add_new_panel(hass: Hass) -> Entry:
    """The ESPHome entry of another panel with the same Panel ID (the panel was replaced), with its Panel user sensor."""
    new = Entry("esphome", "new-voice-entry", {"device_name": "nspanel-pro", "noise_psk": "new key"})
    new.unique_id = NEW_MAC.lower()
    hass.config_entries.entries.append(new)
    hass.entity_registry.append(RegistryEntry(NEW_SENSOR, "esphome", f"{NEW_MAC}/0/text_sensor/Panel user", new.entry_id))
    return new


class FirstToConnect(unittest.TestCase):
    """A user that is not the panel's connects first while the panel names nobody (it is restarting) and reports a MAC
    address of its choice: the panel's ESPHome entry stays the panel's, and once the panel names its user the binding
    moves to it."""

    def test_a_made_up_mac_is_not_kept(self) -> None:
        hass, entry, u = with_mac()
        report(hass, "unavailable")
        self.assertTrue(panel.may_act(hass, entry, u[USER_B]))
        report_device(hass, entry, u[USER_B], OTHER_MAC)
        self.assertIsNone(kept_mac(entry))
        # the rest of the report is taken as before
        self.assertEqual(entry.data["device"], {"name": "NSPanel Pro", "model": "NSPanel Pro"})
        self.assertIs(panel.voice_entry(hass, "nspanel-pro", panel.panel_mac(entry)), voice_of(hass))
        # the panel is back and names its user: the binding moves, the first user's links end
        report(hass, USER_A)
        panel.user_reported(hass, entry)
        run_tasks(entry)
        self.assertEqual(kept_id(entry), USER_A)
        self.assertEqual(hass.sent[-1], (SIGNAL_PANEL_USER, (entry.entry_id, USER_A)))
        self.assertFalse(panel.may_act(hass, entry, u[USER_B]))
        self.assertTrue(panel.may_act(hass, entry, u[USER_A]))

    def test_nor_the_mac_of_another_esphome_entry_with_the_name(self) -> None:
        hass, entry, u = with_mac()
        voice = voice_of(hass)
        # listed first, with a key, naming nobody
        other = Entry("esphome", "other-entry", {"device_name": "nspanel-pro", "noise_psk": "key"})
        other.unique_id = OTHER_MAC
        hass.config_entries.entries.insert(1, other)
        panel.may_act(hass, entry, u[USER_B])
        report_device(hass, entry, u[USER_B], OTHER_MAC)
        self.assertIsNone(kept_mac(entry))
        report(hass, USER_A)
        self.assertIs(panel.voice_entry(hass, "nspanel-pro", panel.panel_mac(entry)), voice)
        self.assertEqual(panel.reported_user(hass, entry), USER_A)
        self.assertTrue(panel.may_act(hass, entry, u[USER_A]))
        self.assertEqual(kept_id(entry), USER_A)

    def test_nor_can_it_take_the_kept_one_away(self) -> None:
        hass, entry, u = with_mac()
        entry.data["device"] = {"mac": MAC.lower()}
        panel.may_act(hass, entry, u[USER_B])
        for mac in (OTHER_MAC, None):
            with self.subTest(mac=mac):
                report_device(hass, entry, u[USER_B], mac)
                self.assertEqual(kept_mac(entry), MAC.lower())

    def test_a_mac_kept_before_this_check_does_not_hide_the_panel(self) -> None:
        hass, entry, u = with_mac()
        entry.data["device"] = {"mac": OTHER_MAC}
        panel.may_act(hass, entry, u[USER_B])
        report(hass, USER_A)
        self.assertEqual(panel.reported_user(hass, entry), USER_A)
        self.assertFalse(panel.may_act(hass, entry, u[USER_B]))
        self.assertTrue(panel.may_act(hass, entry, u[USER_A]))
        # the panel's next report puts its own MAC address back
        report_device(hass, entry, u[USER_A], MAC.lower())
        self.assertEqual(kept_mac(entry), MAC.lower())


class ReportedMac(unittest.TestCase):
    def test_the_panel_confirms_its_own(self) -> None:
        hass, entry, u = with_mac()
        report(hass, USER_A)
        self.assertTrue(panel.may_act(hass, entry, u[USER_A]))
        report_device(hass, entry, u[USER_A], MAC)
        self.assertEqual(kept_mac(entry), MAC.lower())

    def test_an_administrators_report_is_kept(self) -> None:
        hass, entry, u = setup(sensor=False)
        report_device(hass, entry, u[ADMIN], MAC)
        self.assertEqual(kept_mac(entry), MAC.lower())

    def test_an_older_panel_without_the_sensor(self) -> None:
        # nobody named: the first user is kept as before, its MAC address is not; the ESPHome entry is found by its name
        hass, entry, u = setup(sensor=False)
        voice_of(hass).unique_id = MAC.lower()
        self.assertTrue(panel.may_act(hass, entry, u[USER_B]))
        report_device(hass, entry, u[USER_B], MAC)
        self.assertIsNone(kept_mac(entry))
        self.assertIs(panel.voice_entry(hass, "nspanel-pro", panel.panel_mac(entry)), voice_of(hass))


class ReplacedPanel(unittest.TestCase):
    """The panel replaced by another one with the same Panel ID, logged in with another user."""

    def _old_panel(self) -> tuple[Hass, Entry, dict[str, User]]:
        hass, entry, u = with_mac()
        entry.data["device"] = {"mac": MAC.lower()}
        report(hass, USER_A)
        panel.may_act(hass, entry, u[USER_A])
        # the old panel is gone
        report(hass, "unavailable")
        return hass, entry, u

    def test_while_the_old_entry_is_there_it_stays_the_panels(self) -> None:
        hass, entry, u = self._old_panel()
        old = voice_of(hass)
        add_new_panel(hass)
        hass.states.states[NEW_SENSOR] = State(USER_B)
        self.assertEqual(panel.user_sensors(hass, entry), [SENSOR, NEW_SENSOR])
        self.assertIs(panel.voice_entry(hass, "nspanel-pro", panel.panel_mac(entry)), old)
        self.assertIsNone(panel.reported_user(hass, entry))
        self.assertFalse(panel.may_act(hass, entry, u[USER_B]))
        # deleting the old panel's ESPHome entry moves the binding by itself
        hass.config_entries.entries.remove(old)
        self.assertEqual(panel.reported_user(hass, entry), USER_B)
        panel.user_reported(hass, entry)
        run_tasks(entry)
        self.assertEqual(kept_id(entry), USER_B)
        report_device(hass, entry, u[USER_B], NEW_MAC)
        self.assertEqual(kept_mac(entry), NEW_MAC.lower())

    def test_the_same_user_on_the_new_panel(self) -> None:
        hass, entry, u = self._old_panel()
        old = voice_of(hass)
        add_new_panel(hass)
        hass.states.states[NEW_SENSOR] = State(USER_A)
        self.assertTrue(panel.may_act(hass, entry, u[USER_A]))
        # its MAC address waits for the old panel's entry to go
        report_device(hass, entry, u[USER_A], NEW_MAC)
        self.assertEqual(kept_mac(entry), MAC.lower())
        hass.config_entries.entries.remove(old)
        report_device(hass, entry, u[USER_A], NEW_MAC)
        self.assertEqual(kept_mac(entry), NEW_MAC.lower())
        # the old panel back on with its entry added again: the kept MAC address settles it
        hass.config_entries.entries.insert(1, old)
        report(hass, USER_B)
        self.assertEqual(panel.reported_user(hass, entry), USER_A)

    def test_the_old_entry_deleted(self) -> None:
        hass, entry, u = self._old_panel()
        hass.config_entries.entries.remove(voice_of(hass))
        add_new_panel(hass)
        # another device's entry is not taken while it names nobody
        self.assertIsNone(panel.voice_entry(hass, "nspanel-pro", panel.panel_mac(entry)))
        self.assertIsNone(panel.reported_user(hass, entry))
        hass.states.states[NEW_SENSOR] = State(USER_B)
        self.assertEqual(panel.reported_user(hass, entry), USER_B)
        self.assertTrue(panel.may_act(hass, entry, u[USER_B]))
        self.assertEqual(kept_id(entry), USER_B)


class DeviceGivenThePanelsName(unittest.TestCase):
    """A user's own ESPHome device, added with a key, renamed on the device to the panel's ID (Home Assistant takes the
    new name), whose Panel user sensor names that user."""

    def _with_impostor(self) -> tuple[Hass, Entry, dict[str, User]]:
        hass, entry, u = with_mac()
        entry.data["device"] = {"mac": MAC.lower()}
        impostor = add_new_panel(hass)
        hass.config_entries.entries.remove(impostor)
        hass.config_entries.entries.insert(1, impostor)
        hass.states.states[NEW_SENSOR] = State(USER_B)
        return hass, entry, u

    def test_it_never_counts_while_the_panels_entry_is_there(self) -> None:
        hass, entry, u = self._with_impostor()
        report(hass, USER_A)
        panel.may_act(hass, entry, u[USER_A])
        # the panel offline
        report(hass, "unavailable")
        self.assertIsNone(panel.reported_user(hass, entry))
        self.assertFalse(panel.may_act(hass, entry, u[USER_B]))
        self.assertEqual(kept_id(entry), USER_A)

    def test_kept_for_connecting_first_it_cannot_move_the_mac(self) -> None:
        hass, entry, u = self._with_impostor()
        report(hass, "unavailable")
        self.assertTrue(panel.may_act(hass, entry, u[USER_B]))
        report_device(hass, entry, u[USER_B], NEW_MAC)
        self.assertEqual(kept_mac(entry), MAC.lower())
        report(hass, USER_A)
        panel.user_reported(hass, entry)
        run_tasks(entry)
        self.assertEqual(kept_id(entry), USER_A)
        self.assertFalse(panel.may_act(hass, entry, u[USER_B]))


if __name__ == "__main__":
    unittest.main()
