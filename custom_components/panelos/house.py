"""Critical alerts and status screens, shared by every panel.

They are stored once for the integration (``panelos.house``), not in a
panel's entry: each critical alert lists the panels it is switched off on,
each status screen the panels it appears on. This module follows the trigger
entities' states and sends show requests through the panels' subscriptions
(``panelos/subscribe``):

* status screen on: ``{"type": "status", "id", "cat", "headline", "line",
  "picto", "colour", "button", "button_label", "countdown"}`` (headline and
  line empty: the panel picks its own texts for the kind; countdown = minutes
  left, 0 = none); off: ``{"type": "status_clear", "id"}``;
* critical alert on: ``{"type": "critical", "id": entity_id, "title",
  "message", "icon", "source", "hold", "sound", "tone"}``; off (the sensor
  cleared): ``{"type": "critical_clear", "id"}``. Own critical alerts (started
  by a device or by the ``panelos.critical_alert`` action) have the id
  ``own:<id>`` and their own headline and line.

Sounds are named ``auto`` (the place's default), ``lib:<id>`` (one the panel
has) or ``own:<id>`` (uploaded, kept here as WAV, each panel keeps a copy).

A panel that (re)connects gets what is active at that moment.

Status screen triggers: an entity's state (on, off, above or below a number,
equal to a text), "after" another status screen ends (for its countdown, 10
minutes when none is set), or the ``panelos.show_status`` action. An entity
that becomes unavailable or unknown for a moment (its integration reloads, a
device reconnects) does not end a screen whose trigger held: it stays for up
to ``UNAVAILABLE_GRACE`` seconds and ends only if the entity is still not
back then (or comes back in a state that does not hold).
"""

from __future__ import annotations

from collections.abc import Callable
import logging
import math
import os
import time
from typing import Any

import voluptuous as vol

from homeassistant.core import CALLBACK_TYPE, Event, HomeAssistant, State, callback
from homeassistant.helpers import area_registry as ar, config_validation as cv, device_registry as dr, entity_registry as er
from homeassistant.helpers.dispatcher import async_dispatcher_send
from homeassistant.helpers.event import async_call_later, async_track_state_change_event
from homeassistant.helpers.storage import Store

from .const import DOMAIN, HOUSE_STORE, SIGNAL_CONFIG_UPDATED, SIGNAL_PAGE, SIGNAL_SHOW, SOUND_DIR, SOUND_LIB, STATUS_CATS
from .panel import panel_config

_LOGGER = logging.getLogger(__name__)

STORE_VERSION = 1
AFTER_DEFAULT_MINUTES = 10
# how long a status screen whose trigger entity became unavailable or unknown stays (seconds)
UNAVAILABLE_GRACE = 120

# Plain texts of a critical alert per device class: the title and the icon.
CRITICAL_TEXTS = {
    "smoke": ("Smoke detected", "mdi:smoke-detector-alert"),
    "carbon_monoxide": ("Carbon monoxide detected", "mdi:molecule-co"),
    "gas": ("Gas detected", "mdi:gas-cylinder"),
    "moisture": ("Water leak detected", "mdi:water-alert"),
    "problem": ("Problem reported", "mdi:alert-outline"),
    "tamper": ("Tamper alarm", "mdi:shield-alert-outline"),
    "safety": ("Safety alarm", "mdi:alert-octagon-outline"),
}
_CRITICAL_DEFAULT = ("Alarm", "mdi:alert-octagon-outline")

_ID = vol.All(str, vol.Match(r"^[a-z0-9_]{1,40}$"))
_COLOUR = vol.All(str, vol.Match(r"^#[0-9a-fA-F]{6}$"))
_PICTO = vol.All(str, vol.Match(r"^(mdi:)?[a-z0-9-]{1,48}$"))
_ENTRY_IDS = vol.All([vol.All(str, vol.Length(max=64))], vol.Length(max=64))
_SOUND_ID = vol.All(str, vol.Match(r"^[a-z0-9_-]{1,64}$"))


def _tone(value: Any) -> str:
    """A sound value: auto, lib:<id> or own:<id>."""
    value = vol.Coerce(str)(value)
    if value == "auto" or (value.startswith("lib:") and value[4:] in SOUND_LIB):
        return value
    if value.startswith("own:"):
        _SOUND_ID(value[4:])
        return value
    raise vol.Invalid(f"not a sound: {value}")


CRITICAL_SCHEMA = vol.Schema(
    {
        vol.Required("entity_id"): cv.entity_id,
        vol.Optional("on", default=True): bool,
        vol.Optional("hold", default=True): bool,
        # entries (panels) this alert is not shown on
        vol.Optional("off", default=list): _ENTRY_IDS,
        vol.Optional("sound", default=True): bool,
        vol.Optional("tone", default="auto"): _tone,
    }
)

OWN_TRIGGER_SCHEMA = vol.Schema(
    {
        vol.Required("kind"): vol.In(("entity", "service")),
        vol.Optional("entity", default=""): vol.Any("", cv.entity_id),
        vol.Optional("op", default="on"): vol.In(("on", "off", "above", "below")),
        vol.Optional("value", default=""): vol.All(vol.Coerce(str), vol.Length(max=60)),
    }
)

OWN_CRITICAL_SCHEMA = vol.Schema(
    {
        vol.Required("id"): _ID,
        vol.Required("trigger"): OWN_TRIGGER_SCHEMA,
        vol.Optional("headline", default=""): vol.All(str, vol.Length(max=30)),
        vol.Optional("line", default=""): vol.All(str, vol.Length(max=60)),
        vol.Optional("on", default=True): bool,
        vol.Optional("hold", default=True): bool,
        vol.Optional("off", default=list): _ENTRY_IDS,
        vol.Optional("sound", default=True): bool,
        vol.Optional("tone", default="auto"): _tone,
    }
)

SOUND_SCHEMA = vol.Schema(
    {
        vol.Required("id"): _SOUND_ID,
        vol.Required("name"): vol.All(str, vol.Length(min=1, max=40)),
        vol.Optional("bytes", default=0): vol.All(vol.Coerce(int), vol.Range(min=0)),
        vol.Optional("secs", default=0): vol.All(vol.Coerce(float), vol.Range(min=0)),
    }
)

TRIGGER_SCHEMA = vol.Schema(
    {
        vol.Required("kind"): vol.In(("entity", "after", "service")),
        vol.Optional("entity", default=""): vol.Any("", cv.entity_id),
        vol.Optional("op", default="on"): vol.In(("on", "off", "above", "below", "equals")),
        vol.Optional("value", default=""): vol.All(vol.Coerce(str), vol.Length(max=60)),
        # kind "after": the status screen whose end starts this one
        vol.Optional("after", default=""): vol.Any("", _ID),
    }
)

STATUS_SCHEMA = vol.Schema(
    {
        vol.Required("id"): _ID,
        vol.Required("cat"): vol.In(STATUS_CATS),
        vol.Required("trigger"): TRIGGER_SCHEMA,
        vol.Optional("own", default=False): bool,
        vol.Optional("headline", default=""): vol.All(str, vol.Length(max=40)),
        vol.Optional("line", default=""): vol.All(str, vol.Length(max=80)),
        vol.Optional("picto", default=""): vol.Any("", _PICTO),
        vol.Optional("colour", default="#0b7285"): _COLOUR,
        vol.Optional("button", default=True): bool,
        vol.Optional("button_label", default=""): vol.All(str, vol.Length(max=24)),
        vol.Optional("countdown", default=0): vol.All(vol.Coerce(int), vol.Range(min=0, max=240)),
        # entries (panels) it appears on
        vol.Optional("panels", default=list): _ENTRY_IDS,
        vol.Optional("on", default=True): bool,
        # the screen's name in automations (panelos.show_status / clear_status screen: …); empty = its id
        vol.Optional("alias", default=""): vol.Any("", _ID),
    }
)


def _unique_aliases(items: list[dict[str, Any]]) -> list[dict[str, Any]]:
    """No two status screens with the same name in automations (empty names do not count)."""
    _unique_ids([x for x in items if x.get("alias")], "alias")
    return items


def _unique_ids(items: list[dict[str, Any]], key: str) -> list[dict[str, Any]]:
    seen = set()
    for item in items:
        if item[key] in seen:
            raise vol.Invalid(f"{item[key]} is listed twice")
        seen.add(item[key])
    return items


HOUSE_SCHEMA = vol.Schema(
    {
        vol.Optional("critical", default=list): vol.All([CRITICAL_SCHEMA], vol.Length(max=64), lambda v: _unique_ids(v, "entity_id")),
        vol.Optional("own_critical", default=list): vol.All([OWN_CRITICAL_SCHEMA], vol.Length(max=32), lambda v: _unique_ids(v, "id")),
        vol.Optional("dismissed", default=list): vol.All([cv.entity_id], vol.Length(max=200)),
        vol.Optional("status", default=list): vol.All(
            [STATUS_SCHEMA], vol.Length(max=32), lambda v: _unique_ids(v, "id"), _unique_aliases
        ),
        vol.Optional("sounds", default=list): vol.All([SOUND_SCHEMA], vol.Length(max=50), lambda v: _unique_ids(v, "id")),
    }
)


def empty_house() -> dict[str, Any]:
    return {"critical": [], "own_critical": [], "dismissed": [], "status": [], "sounds": []}


def state_known(state: State | None) -> bool:
    """Whether an entity's state says anything (it exists and is neither unavailable nor unknown)."""
    return state is not None and state.state not in ("unavailable", "unknown")


def trigger_on(trigger: dict[str, Any], state: State | None) -> bool:
    """Whether an entity trigger holds for the entity's state."""
    if not state_known(state):
        return False
    op = trigger.get("op", "on")
    # a lock is "on" when unlocked
    on, off = ("unlocked", "locked") if state.domain == "lock" else ("on", "off")
    if op == "on":
        return state.state == on
    if op == "off":
        return state.state == off
    if op == "equals":
        return state.state == str(trigger.get("value", ""))
    try:
        value, limit = float(state.state), float(trigger.get("value", ""))
    except ValueError:
        return False
    if math.isnan(value) or math.isnan(limit):
        return False
    return value > limit if op == "above" else value < limit


class House:
    """The shared alerts and status screens: stored, evaluated, sent to the panels."""

    def __init__(self, hass: HomeAssistant, now: Callable[[], float] = time.time) -> None:
        self.hass = hass
        self.data: dict[str, Any] = empty_house()
        self._store: Store[dict[str, Any]] = Store(hass, STORE_VERSION, HOUSE_STORE)
        self._now = now
        self._unsub_states: CALLBACK_TYPE | None = None
        self._unsub_timer: CALLBACK_TYPE | None = None
        # status id -> since when its trigger holds (entity) or it was started (after / service)
        self._since: dict[str, float] = {}
        # status screens whose source ended: id -> since
        self._after: dict[str, float] = {}
        # status screens switched on by the action: id -> since
        self._manual: dict[str, float] = {}
        # status screens whose trigger held and whose entity is unavailable or unknown now: id -> since when
        self._unknown: dict[str, float] = {}
        # own critical alerts started by the action (panelos.critical_alert)
        self._crit_manual: set[str] = set()
        # what is shown where: key ("critical:<entity>" / "status:<id>") -> (request, panel ids)
        self._shown: dict[str, tuple[dict[str, Any], set[str]]] = {}
        # tests running on a panel: (key, panel id) -> cancel
        self._tests: dict[tuple[str, str], CALLBACK_TYPE] = {}
        self._base_before: set[str] = set()

    # -- storage -------------------------------------------------------------

    async def async_load(self) -> None:
        stored = await self._store.async_load()
        if isinstance(stored, dict):
            try:
                self.data = HOUSE_SCHEMA(stored)
            except vol.Invalid as err:
                _LOGGER.warning("Stored PanelOS alerts and status screens are not valid (%s); starting empty", err)
        self._track()
        self.evaluate()

    async def async_save(self, data: dict[str, Any]) -> None:
        """Store new settings (already validated) and act on them at once."""
        gone = {s["id"] for s in self.data.get("sounds", [])} - {s["id"] for s in data.get("sounds", [])}
        self.data = data
        await self._store.async_save(data)
        # uploaded sounds taken off the list: their files go too
        if gone:
            await self.hass.async_add_executor_job(self._delete_files, gone)
        self._crit_manual &= {c["id"] for c in data.get("own_critical", [])}
        known = {s["id"] for s in data["status"]}
        for table in (self._since, self._after, self._manual, self._unknown):
            for status_id in [i for i in table if i not in known]:
                table.pop(status_id)
        self._track()
        self.evaluate()
        async_dispatcher_send(self.hass, SIGNAL_PAGE)
        # every panel's settings carry the critical alerts' sounds it keeps a copy of
        async_dispatcher_send(self.hass, SIGNAL_CONFIG_UPDATED, "")

    # -- uploaded sounds ---------------------------------------------------------

    def sound_path(self, sound_id: str) -> str:
        return self.hass.config.path(SOUND_DIR, f"{sound_id}.wav")

    def _delete_files(self, ids: set[str]) -> None:
        for sound_id in ids:
            try:
                os.remove(self.sound_path(sound_id))
            except OSError:
                pass

    async def async_add_sound(self, item: dict[str, Any]) -> None:
        """An uploaded sound (its WAV is already in place) joins the list."""
        self.data = HOUSE_SCHEMA({**self.data, "sounds": [*self.data.get("sounds", []), SOUND_SCHEMA(item)]})
        await self._store.async_save(self.data)
        async_dispatcher_send(self.hass, SIGNAL_PAGE)

    def keep_sounds(self, entry_id: str) -> list[str]:
        """The uploaded sounds of the critical alerts shown on a panel (it keeps copies, so they ring offline)."""
        known = {s["id"] for s in self.data.get("sounds", [])}
        tones = {
            c["tone"]
            for c in [*self.data.get("critical", []), *self.data.get("own_critical", [])]
            if c.get("on") and c.get("sound") and entry_id not in c.get("off", [])
        }
        return sorted(t for t in tones if t.startswith("own:") and t[4:] in known)

    @callback
    def async_unload(self) -> None:
        for unsub in (self._unsub_states, self._unsub_timer, *self._tests.values()):
            if unsub:
                unsub()
        self._unsub_states = self._unsub_timer = None
        self._tests.clear()

    # -- following the trigger entities ---------------------------------------

    def _entities(self) -> set[str]:
        ids = {c["entity_id"] for c in self.data["critical"]}
        ids |= {c["trigger"]["entity"] for c in self.data.get("own_critical", []) if c["trigger"]["kind"] == "entity" and c["trigger"]["entity"]}
        ids |= {s["trigger"]["entity"] for s in self.data["status"] if s["trigger"]["kind"] == "entity" and s["trigger"]["entity"]}
        return ids

    def _track(self) -> None:
        if self._unsub_states:
            self._unsub_states()
            self._unsub_states = None
        ids = self._entities()
        if ids:
            self._unsub_states = async_track_state_change_event(self.hass, sorted(ids), self._state_changed)

    @callback
    def _state_changed(self, _event: Event) -> None:
        self.evaluate()

    @callback
    def _timer_fired(self, _now: Any) -> None:
        self._unsub_timer = None
        self.evaluate()

    # -- what is active --------------------------------------------------------

    def _entries(self) -> dict[str, str]:
        """Every PanelOS entry: entry id -> panel id."""
        return {e.entry_id: panel_config(e)["panel_id"] for e in self.hass.config_entries.async_entries(DOMAIN)}

    def _entity_holds(self, status: dict[str, Any], now: float) -> tuple[bool, float | None]:
        """Whether a status screen's entity trigger holds now, and when to look again (the end of the
        grace while its entity is unavailable or unknown)."""
        trigger = status["trigger"]
        state = self.hass.states.get(trigger["entity"]) if trigger["entity"] else None
        if state is not None and not state_known(state) and status["id"] in self._since:
            # it held before: it goes on holding for a while, the entity may be back in a moment
            end = self._unknown.setdefault(status["id"], now) + UNAVAILABLE_GRACE
            if now < end:
                return True, end
        self._unknown.pop(status["id"], None)
        return trigger_on(trigger, state), None

    def _base_active(self, now: float) -> tuple[set[str], set[str], float | None]:
        """Status screens whose own trigger (entity or action) holds now: those to show
        (switched on, countdown not over), every one that holds (for the "after" screens),
        and when to look again (an unavailable entity's grace ends)."""
        active = set()
        holding = set()
        wake: float | None = None
        for s in self.data["status"]:
            trigger = s["trigger"]
            if trigger["kind"] == "entity":
                holds, grace_end = self._entity_holds(s, now)
                if grace_end is not None:
                    wake = grace_end if wake is None else min(wake, grace_end)
                if not holds:
                    self._since.pop(s["id"], None)
                    continue
                since = self._since.setdefault(s["id"], now)
            elif trigger["kind"] == "service":
                if s["id"] not in self._manual:
                    continue
                since = self._manual[s["id"]]
            else:
                continue
            if s["countdown"] and now >= since + s["countdown"] * 60:
                if self._manual.pop(s["id"], None) is None:
                    holding.add(s["id"])  # an entity trigger still holds; its screen is over
                continue
            holding.add(s["id"])
            if s["on"]:
                active.add(s["id"])
        return active, holding, wake

    def _active(self, now: float) -> tuple[dict[str, dict[str, Any]], float | None]:
        """Requests that should be shown now (key -> request) and when to look again."""
        base, holding, wake = self._base_active(now)
        # a source "ends" when its trigger stops holding (not when it is switched off)
        ended = self._base_before - holding
        self._base_before = holding
        wanted: dict[str, dict[str, Any]] = {}
        for s in self.data["status"]:
            trigger = s["trigger"]
            if trigger["kind"] == "after":
                source = trigger["after"]
                if source in holding:
                    self._after.pop(s["id"], None)
                elif source in ended and s["on"]:
                    self._after[s["id"]] = now
            minutes = s["countdown"] or (AFTER_DEFAULT_MINUTES if trigger["kind"] == "after" else 0)
            if trigger["kind"] == "after":
                since = self._after.get(s["id"])
                if since is None:
                    continue
                if now >= since + minutes * 60:
                    self._after.pop(s["id"], None)
                    continue
            elif s["id"] in base:
                since = self._since.get(s["id"]) if trigger["kind"] == "entity" else self._manual.get(s["id"])
            else:
                continue
            if not s["on"]:
                continue
            left = 0
            if minutes and since is not None:
                end = since + minutes * 60
                left = max(1, math.ceil((end - now) / 60))
                wake = end if wake is None else min(wake, end)
            wanted[f"status:{s['id']}"] = status_request(s, left)
        for c in self.data["critical"]:
            state = self.hass.states.get(c["entity_id"])
            if c["on"] and state is not None and state.state == "on":
                wanted[f"critical:{c['entity_id']}"] = self.critical_request(c)
        for c in self.data.get("own_critical", []):
            trigger = c["trigger"]
            if trigger["kind"] == "entity":
                holds = bool(trigger["entity"]) and trigger_on(trigger, self.hass.states.get(trigger["entity"]))
            else:
                holds = c["id"] in self._crit_manual
            if c["on"] and holds:
                wanted[f"critical:own:{c['id']}"] = self.own_critical_request(c)
        return wanted, wake

    def _targets(self, key: str, entries: dict[str, str]) -> set[str]:
        kind, item_id = key.split(":", 1)
        if kind == "critical":
            item = self._critical_item(item_id)
            return {pid for eid, pid in entries.items() if item and eid not in item["off"]}
        item = next((s for s in self.data["status"] if s["id"] == item_id), None)
        return {pid for eid, pid in entries.items() if item and eid in item["panels"]}

    @callback
    def evaluate(self) -> None:
        """Works out what should be shown where and sends the differences."""
        now = self._now()
        before = self.active()
        wanted, wake = self._active(now)
        entries = self._entries()
        for key in list(self._shown):
            request, shown_on = self._shown[key]
            targets = self._targets(key, entries) if key in wanted else set()
            for pid in shown_on - targets:
                self._send(pid, clear_request(request))
            if key in wanted:
                # Sent again only when it changed; the minutes left alone do not count
                # (the panel counts down itself; a panel that reconnects gets them fresh).
                if _without_countdown(wanted[key]) != _without_countdown(request):
                    for pid in shown_on & targets:
                        self._send(pid, wanted[key])
                self._shown[key] = (wanted[key], shown_on & targets)
            else:
                del self._shown[key]
        for key, request in wanted.items():
            _, shown_on = self._shown.get(key, (request, set()))
            targets = self._targets(key, entries)
            for pid in targets - shown_on:
                self._send(pid, request)
            self._shown[key] = (request, shown_on | targets)
        if self._unsub_timer:
            self._unsub_timer()
            self._unsub_timer = None
        if wake is not None:
            self._unsub_timer = async_call_later(self.hass, max(1.0, wake - now + 0.5), self._timer_fired)
        if self.active() != before:
            async_dispatcher_send(self.hass, SIGNAL_PAGE)

    def _critical_item(self, item_id: str) -> dict[str, Any] | None:
        """A critical alert by its request id (an entity id, or own:<id>)."""
        if item_id.startswith("own:"):
            return next((c for c in self.data.get("own_critical", []) if c["id"] == item_id[4:]), None)
        return next((c for c in self.data["critical"] if c["entity_id"] == item_id), None)

    def active(self) -> dict[str, list[str]]:
        """What is active now, for the sidebar page."""
        return {
            "critical": sorted(k.split(":", 1)[1] for k in self._shown if k.startswith("critical:")),
            "status": sorted(k.split(":", 1)[1] for k in self._shown if k.startswith("status:")),
        }

    def requests_for(self, panel_id: str) -> list[dict[str, Any]]:
        """What a panel that just (re)connected must show."""
        return [request for request, shown_on in self._shown.values() if panel_id in shown_on]

    def _send(self, panel_id: str, request: dict[str, Any]) -> None:
        async_dispatcher_send(self.hass, SIGNAL_SHOW, panel_id, request)

    # -- requests ----------------------------------------------------------------

    def critical_request(self, item: dict[str, Any]) -> dict[str, Any]:
        entity_id = item["entity_id"]
        state = self.hass.states.get(entity_id)
        attrs = state.attributes if state else {}
        title, icon = CRITICAL_TEXTS.get(attrs.get("device_class", ""), _CRITICAL_DEFAULT)
        return {
            "type": "critical",
            "id": entity_id,
            "title": title,
            "message": str(attrs.get("friendly_name") or entity_id),
            "icon": attrs.get("icon") or icon,
            "source": self._area_name(entity_id),
            "hold": item["hold"],
            "sound": item.get("sound", True),
            "tone": item.get("tone", "auto"),
        }

    def own_critical_request(self, item: dict[str, Any]) -> dict[str, Any]:
        trigger = item["trigger"]
        source = self._area_name(trigger["entity"]) if trigger["kind"] == "entity" and trigger["entity"] else ""
        return {
            "type": "critical",
            "id": f"own:{item['id']}",
            "title": item["headline"] or "Critical alert",
            "message": item["line"],
            "icon": "mdi:alert-octagon-outline",
            "source": source,
            "hold": item["hold"],
            "sound": item["sound"],
            "tone": item["tone"],
        }

    def _area_name(self, entity_id: str) -> str:
        """The entity's area (its own or its device's), empty when none."""
        entry = er.async_get(self.hass).async_get(entity_id)
        if entry is None:
            return ""
        area_id = entry.area_id
        if area_id is None and entry.device_id:
            device = dr.async_get(self.hass).async_get(entry.device_id)
            area_id = device.area_id if device else None
        area = ar.async_get(self.hass).async_get_area(area_id) if area_id else None
        return area.name if area else ""

    # -- the action and tests ------------------------------------------------------

    def _status_named(self, name: str) -> dict[str, Any] | None:
        """A status screen by its name in automations (alias) or its id."""
        return next((s for s in self.data["status"] if s.get("alias") == name), None) or next(
            (s for s in self.data["status"] if s["id"] == name), None
        )

    @callback
    def show_status(self, status_id: str) -> None:
        """panelos.show_status: a status screen with the "action" trigger comes on."""
        status = self._status_named(status_id)
        if status is None:
            raise vol.Invalid(f"No status screen {status_id}")
        if status["trigger"]["kind"] != "service":
            raise vol.Invalid(f"The status screen {status_id} is not started by an action")
        self._manual[status["id"]] = self._now()
        self.evaluate()

    @callback
    def show_critical(self, alert_id: str) -> None:
        """panelos.critical_alert: an own critical alert started by an automation."""
        item = next((c for c in self.data.get("own_critical", []) if c["id"] == alert_id), None)
        if item is None:
            raise vol.Invalid(f"No own critical alert {alert_id}")
        if item["trigger"]["kind"] != "service":
            raise vol.Invalid(f"The critical alert {alert_id} is not started by an action")
        self._crit_manual.add(alert_id)
        self.evaluate()

    @callback
    def clear_critical(self, alert_id: str) -> None:
        self._crit_manual.discard(alert_id)
        self.evaluate()

    @callback
    def clear_status(self, status_id: str) -> None:
        """panelos.clear_status: a screen started by the action (or by another one's end) ends; one that
        follows an entity ends when the entity says so."""
        status = self._status_named(status_id)
        item_id = status["id"] if status else status_id
        self._manual.pop(item_id, None)
        self._after.pop(item_id, None)
        self.evaluate()

    @callback
    def test(self, panel_id: str, kind: str, item_id: str, seconds: int) -> None:
        """Shows a critical alert or a status screen on one panel for a while (the sidebar page's tests)."""
        if kind == "critical":
            item = self._critical_item(item_id)
            if item is None:
                request = None
            elif item_id.startswith("own:"):
                request = self.own_critical_request(item)
            else:
                request = self.critical_request(item)
        else:
            item = next((s for s in self.data["status"] if s["id"] == item_id), None)
            request = status_request(item, 0) if item else None
        if request is None:
            raise vol.Invalid(f"No {kind} {item_id}")
        key = f"{kind}:{item_id}"
        if cancel := self._tests.pop((key, panel_id), None):
            cancel()
        self._send(panel_id, request)

        @callback
        def done(_now: Any) -> None:
            self._tests.pop((key, panel_id), None)
            shown = self._shown.get(key)
            if shown and panel_id in shown[1]:
                self._send(panel_id, shown[0])  # really active: stays
            else:
                self._send(panel_id, clear_request(request))

        self._tests[(key, panel_id)] = async_call_later(self.hass, seconds, done)


def status_request(status: dict[str, Any], minutes_left: int) -> dict[str, Any]:
    own = status["own"] or status["cat"] == "custom"
    return {
        "type": "status",
        "id": status["id"],
        "cat": status["cat"],
        "headline": status["headline"] if own else "",
        "line": status["line"] if own else "",
        "picto": status["picto"],
        "colour": status["colour"],
        "button": status["button"],
        "button_label": status["button_label"] if status["button"] else "",
        "countdown": minutes_left,
    }


def _without_countdown(request: dict[str, Any]) -> dict[str, Any]:
    return {k: v for k, v in request.items() if k != "countdown"}


def clear_request(request: dict[str, Any]) -> dict[str, Any]:
    return {"type": f"{request['type']}_clear", "id": request["id"]}
