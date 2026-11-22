"""Home Assistant's own wake word selects of a panel's voice device, kept in line with what the panel listens for.

The panel reports its wake words (`panelos/report`). When Home Assistant's configuration of the satellite or its
selects ("Wake word", "Wake word 2") say otherwise, the configuration takes the panel's words (a change made on the
panel itself) and the selects are set to them one after the other, the first first, each after Home Assistant has
sent it to the panel and read the panel's answer (see `wake_word_plan.py` for why they drift).
"""

from __future__ import annotations

import asyncio
from collections.abc import Callable
import dataclasses
import logging
import time
from typing import Any

from homeassistant.config_entries import ConfigEntry
from homeassistant.core import Event, HomeAssistant, callback
from homeassistant.helpers import entity_registry as er
from homeassistant.helpers.event import async_call_later, async_track_state_change_event

from .wake_word_plan import plan

_LOGGER = logging.getLogger(__name__)

# the selects' unique ids end with their keys (Home Assistant's ESPHome selects: "wake_word", then "wake_word_2")
_SELECT_KEYS = ("-wake_word", "-wake_word_2")
_GONE = ("unavailable", "unknown")
# after a report of the panel: how long before a look
_AFTER_REPORT = 2.0
# after a select changed (by hand, by Home Assistant): the panel's report of what it took follows within this time
_AFTER_SELECT = 5.0
# a select set: Home Assistant sends it to the panel and reads the panel's configuration back meanwhile
_ECHO = 2.0
# at most this many runs in _RUNS_WINDOW seconds: never a loop with a satellite that keeps answering otherwise
_RUNS = 4
_RUNS_WINDOW = 600.0


def _satellite(hass: HomeAssistant, entity_id: str) -> Any:
    try:
        from homeassistant.components.assist_satellite.const import DATA_COMPONENT
    except ImportError:
        return None
    component = hass.data.get(DATA_COMPONENT)
    return component.get_entity(entity_id) if component is not None else None


class WakeWordSelects:
    """One panel's follow-up of its voice device's wake word selects."""

    def __init__(self, hass: HomeAssistant, voice_entry: Callable[[], ConfigEntry | None]) -> None:
        self.hass = hass
        self._voice_entry = voice_entry
        # what the panel says it listens for (None: an older panel)
        self.words: list[str] | None = None
        self._watched: tuple[str, ...] = ()
        self._unsub_states: Callable[[], None] | None = None
        self._unsub_later: Callable[[], None] | None = None
        self._running = False
        self._quiet_until = 0.0
        self._runs: list[float] = []
        self._told_limit = False

    @callback
    def reported(self, words: list[str]) -> None:
        if words != self.words:
            self.words = words
            self.schedule(_AFTER_REPORT)

    @callback
    def schedule(self, delay: float = _AFTER_REPORT) -> None:
        self._follow()
        if self._unsub_later is not None:
            self._unsub_later()
        self._unsub_later = async_call_later(self.hass, delay, self._later)

    @callback
    def stop(self) -> None:
        if self._unsub_later is not None:
            self._unsub_later()
            self._unsub_later = None
        if self._unsub_states is not None:
            self._unsub_states()
            self._unsub_states = None
        self._watched = ()

    @callback
    def _later(self, _now: Any) -> None:
        self._unsub_later = None
        self.hass.async_create_task(self.async_check(), "PanelOS wake word selects")

    def _entities(self) -> tuple[list[str], str | None]:
        entry = self._voice_entry()
        if entry is None:
            return [], None
        selects: dict[int, str] = {}
        satellite = None
        for e in er.async_entries_for_config_entry(er.async_get(self.hass), entry.entry_id):
            if e.disabled_by:
                continue
            if e.domain == "select" and e.unique_id.endswith(_SELECT_KEYS[1]):
                selects[1] = e.entity_id
            elif e.domain == "select" and e.unique_id.endswith(_SELECT_KEYS[0]):
                selects[0] = e.entity_id
            elif e.domain == "assist_satellite":
                satellite = e.entity_id
        ordered = [selects[0]] if 0 in selects else []
        if ordered and 1 in selects:
            ordered.append(selects[1])
        return ordered, satellite

    @callback
    def _follow(self) -> None:
        watched = tuple(self._entities()[0])
        if watched == self._watched:
            return
        if self._unsub_states is not None:
            self._unsub_states()
        self._unsub_states = async_track_state_change_event(self.hass, list(watched), self._select_changed) if watched else None
        self._watched = watched

    @callback
    def _select_changed(self, event: Event) -> None:
        if self._running or time.monotonic() < self._quiet_until:
            return
        new = event.data.get("new_state")
        if new is None or new.state in _GONE:
            return
        self.schedule(_AFTER_SELECT)

    async def async_check(self) -> None:
        if self._running:
            return
        selects, satellite_id = self._entities()
        if not selects or satellite_id is None:
            return
        satellite = _satellite(self.hass, satellite_id)
        sat_state = self.hass.states.get(satellite_id)
        if satellite is None or sat_state is None or sat_state.state in _GONE:
            return
        states = [self.hass.states.get(e) for e in selects]
        if any(s is None or s.state in _GONE for s in states):
            return
        config = satellite.async_get_configuration()
        names = {w.id: w.wake_word for w in config.available_wake_words}
        if not names:
            return
        todo = plan(self.words, list(config.active_wake_words), names, config.max_active_wake_words, [s.state for s in states])
        if todo.nothing:
            return
        now = time.monotonic()
        self._runs = [t for t in self._runs if now - t < _RUNS_WINDOW]
        if len(self._runs) >= _RUNS:
            if not self._told_limit:
                self._told_limit = True
                _LOGGER.warning("The wake word selects of %s keep differing from the panel's wake words; left as they are", satellite_id)
            return
        self._runs.append(now)
        self._running = True
        try:
            if todo.push:
                # changed on the panel itself: Home Assistant's configuration takes the panel's words (sent back to the
                # panel, which already has them, and read again)
                _LOGGER.info("%s: the panel listens for %s", satellite_id, ", ".join(todo.target) or "no wake word")
                await satellite.async_set_configuration(dataclasses.replace(config, active_wake_words=list(todo.target)))
            for index, option in todo.selects:
                entity_id = selects[index]
                state = self.hass.states.get(entity_id)
                if state is None or state.state == option:
                    continue
                _LOGGER.info("%s: %s, as the panel listens", entity_id, option)
                await self.hass.services.async_call("select", "select_option", {"entity_id": entity_id, "option": option}, blocking=True)
                await asyncio.sleep(_ECHO)
        except Exception as err:  # noqa: BLE001 - a satellite or select that fails stays as it is; the next change tries again
            _LOGGER.warning("Wake word selects of %s not set: %s", satellite_id, err)
        finally:
            self._running = False
            self._quiet_until = time.monotonic() + _AFTER_REPORT
        # one more look once it settled (a select Home Assistant set back meanwhile)
        self.schedule(_AFTER_SELECT)
