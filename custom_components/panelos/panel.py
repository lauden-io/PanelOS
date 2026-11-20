"""One panel: what it reported, its device, and the way back to it.

A panel follows ``panelos/subscribe`` (one or more websocket connections,
each a "link"), reports its states with ``panelos/report`` and gets the
commands of its entities as events on its subscription.
"""

from __future__ import annotations

import asyncio
from collections.abc import Callable
import logging
import time
from typing import Any

from homeassistant.auth.models import User
from homeassistant.config_entries import ConfigEntry
from homeassistant.core import Event, HomeAssistant, callback
from homeassistant.exceptions import HomeAssistantError
from homeassistant.components.intent.const import TIMER_DATA
from homeassistant.helpers import area_registry as ar, device_registry as dr, entity_registry as er
from homeassistant.helpers.dispatcher import async_dispatcher_connect, async_dispatcher_send
from homeassistant.helpers.event import async_call_later
from homeassistant.helpers.entity import Entity, entity_sources
from homeassistant.helpers.entity_platform import AddEntitiesCallback
from homeassistant.helpers.start import async_at_started
from homeassistant.helpers.storage import Store
from homeassistant.helpers.typing import UNDEFINED

from .const import (
    CONF_DASHBOARD,
    CONF_DEVICE,
    CONF_MEDIA_PLAYER,
    CONF_PANEL_ID,
    CONF_PANEL_USER,
    CONF_SHOW_CONVERSATION,
    CONF_VOICE_LOOK,
    CONF_VOICE_PLAYER,
    CONF_SHOW_HEADER,
    CONF_KEEP_RESOURCES,
    CONF_SHOW_SIDEBAR,
    CONF_SKIP_UNUSED,
    CONF_TILES,
    CONF_WEATHER,
    CONF_ZOOM,
    CONF_LOADING_SCREEN,
    CONF_PAGES,
    CONF_HOME_EDIT,
    CONF_CAPS,
    CONF_DND,
    CONF_POPUPS,
    CONF_SCREENSAVER,
    CONF_THEME_AUTO,
    CONF_WAKE_DISTANCE,
    CONF_WIFI_ROAMING,
    DEFAULT_CAPS,
    DEFAULT_DND,
    DEFAULT_PAGES,
    DEFAULT_POPUPS,
    DEFAULT_SCREENSAVER,
    DEFAULT_THEME_AUTO,
    DEFAULT_WAKE_DISTANCE,
    DND_ALLOW,
    DND_MODES,
    DOMAIN,
    POPUP_KINDS,
    SCREENSAVER_WHEN,
    THEME_AUTO_BY,
    WAKE_DISTANCES,
    ENTITY_KEYS,
    ESPHOME_ENTITIES,
    REPORTING,
    SIGNAL_CONFIG_UPDATED,
    SIGNAL_LINKS,
    SIGNAL_PAGE,
    SIGNAL_PANEL_USER,
    SIGNAL_SHOW,
)
from .resources import async_resource_list

_LOGGER = logging.getLogger(__name__)

KEY_DOMAIN = {key: domain for domain, keys in ENTITY_KEYS.items() for key in keys}
_SELECT_KEYS = frozenset(ENTITY_KEYS["select"])
_DEVICE_FIELDS = ("name", "manufacturer", "model", "sw_version", "hw_version", "mac")
_TEXT_MAX = 255
_OPTIONS_MAX = 200
_CAP_MAX = 24
_CAPS_MAX = 32
_ALARMS_MAX = 20
_MISSING = object()

type Send = Callable[[dict[str, Any]], None]
type Factory = Callable[["Panel", str], Entity]


# A panel that went away keeps its entities' last state this long before they turn unavailable (most drops are a few
# seconds: a Wi-Fi hiccup, the app restarting; every unavailable span was a row in the recorder and the logbook).
RECONNECT_GRACE = 30.0
# An entry set up again within this time after its unload (a reload) takes over the last states (`keep_for_reload`).
KEPT_FOR_RELOAD = 120.0


def kept(hass: HomeAssistant) -> dict[str, dict[str, Any]]:
    """What unloaded entries left for their next setup (entry id → states and the reconnect state)."""
    return hass.data.setdefault(DOMAIN, {}).setdefault("kept", {})


def links(hass: HomeAssistant) -> dict[int, tuple[str, int, Send]]:
    """Panels following panelos/subscribe: key → (panel id, connection id, send)."""
    return hass.data.setdefault(DOMAIN, {}).setdefault("links", {})


# A panel says it is there every second (panelos/alive); a link that has said nothing for this long
# counts as gone, so both sides know within seconds when the network between them breaks (not less:
# Home Assistant's own stalls and the Wi-Fi add up to a few seconds now and then).
ALIVE_TIMEOUT = 8.0


def alive(hass: HomeAssistant) -> dict[int, float]:
    """When each connection's panel last said it is there (connection id → monotonic time). Panels
    without the keep-alive (older apps) are not in it and count as there while subscribed."""
    return hass.data.setdefault(DOMAIN, {}).setdefault("alive", {})


def link_gone(hass: HomeAssistant, connection_id: int) -> bool:
    """A link whose panel stopped saying it is there."""
    seen = alive(hass).get(connection_id)
    return seen is not None and time.monotonic() - seen > ALIVE_TIMEOUT


def find_entry(hass: HomeAssistant, panel_id: str) -> ConfigEntry | None:
    """The entry of a panel; a single entry also serves a panel without an id."""
    entries = hass.config_entries.async_entries(DOMAIN)
    for entry in entries:
        if {**entry.data, **entry.options}.get(CONF_PANEL_ID) == panel_id:
            return entry
    return entries[0] if len(entries) == 1 else None


def panel_user(entry: ConfigEntry) -> dict[str, str] | None:
    """The Home Assistant user the panel logs in with ({"id", "name"}), None until one subscribed for it."""
    user = entry.data.get(CONF_PANEL_USER)
    return user if isinstance(user, dict) and isinstance(user.get("id"), str) else None


@callback
def set_panel_user(hass: HomeAssistant, entry: ConfigEntry, user: User | None) -> None:
    """Keep the panel's user (None: forget it, the next subscriber is kept). The links of every other user that is not
    an administrator end first (SIGNAL_PANEL_USER), so none of them is asked again when the entry changes."""
    async_dispatcher_send(hass, SIGNAL_PANEL_USER, entry.entry_id, user.id if user else None)
    data = {key: value for key, value in entry.data.items() if key != CONF_PANEL_USER}
    if user is not None:
        data[CONF_PANEL_USER] = {"id": user.id, "name": user.name or ""}
    hass.config_entries.async_update_entry(entry, data=data)


@callback
def may_act(hass: HomeAssistant, entry: ConfigEntry, user: User) -> bool:
    """Whether this user may act as the entry's panel: administrators always, otherwise only the panel's user. While
    the panel has none, the first user that is not an administrator becomes its user."""
    if user.is_admin:
        return True
    kept = panel_user(entry)
    if kept is None:
        set_panel_user(hass, entry, user)
        return True
    if kept["id"] != user.id:
        return False
    if kept.get("name") != (user.name or ""):
        # renamed in Home Assistant: the PanelOS page shows the new name
        set_panel_user(hass, entry, user)
    return True


def panel_config(entry: ConfigEntry) -> dict[str, Any]:
    """What the panel gets on its subscription (its settings)."""
    options = {**entry.data, **entry.options}
    return {
        "panel_id": options.get(CONF_PANEL_ID, ""),
        "weather_entity": options.get(CONF_WEATHER, ""),
        "media_player_entity": options.get(CONF_MEDIA_PLAYER, ""),
        "tile_entities": list(options.get(CONF_TILES, [])),
        "show_conversation": bool(options.get(CONF_SHOW_CONVERSATION, True)),
        "voice_look": max(0, min(3, int(options.get(CONF_VOICE_LOOK, 0)))),
        "voice_player": options.get(CONF_VOICE_PLAYER, "selected"),
        "dashboard": options.get(CONF_DASHBOARD, ""),
        "show_header": bool(options.get(CONF_SHOW_HEADER, True)),
        "show_sidebar": bool(options.get(CONF_SHOW_SIDEBAR, False)),
        "skip_resources": list(getattr(getattr(entry, "runtime_data", None), "skip", [])),
        "zoom": max(50, min(150, int(options.get(CONF_ZOOM, 100)))),
        "loading_screen": bool(options.get(CONF_LOADING_SCREEN, True)),
        "pages": _door_speakers(entry, shown_pages(options.get(CONF_PAGES, DEFAULT_PAGES))),
        # every person in Home Assistant: a People widget without its own choice shows them all
        "person_entities": _persons(entry),
        # the weather pages' widgets may be edited on the panel too (a long press, then holding the lock)
        "home_edit": bool(options.get(CONF_HOME_EDIT, False)),
        "screensaver": screensaver_config(options.get(CONF_SCREENSAVER)),
        "dnd": dnd_config(options.get(CONF_DND)),
        "popups": popups_config(options.get(CONF_POPUPS)),
        # what switches the Auto theme, Wi-Fi roaming, how close someone comes before the panel wakes
        "theme_auto": theme_auto_config(options.get(CONF_THEME_AUTO)),
        "wifi_roaming": options.get(CONF_WIFI_ROAMING, True) is not False,
        "wake_distance": options[CONF_WAKE_DISTANCE] if options.get(CONF_WAKE_DISTANCE) in WAKE_DISTANCES else DEFAULT_WAKE_DISTANCE,
        "keep_sounds": _keep_sounds(entry),
        "media_player": _media_player(entry),
        # the Music Assistant entity of that player (music alarms play through it), "" until it has one
        "ma_player": _ma_player(entry),
        # each Music Assistant player and the speaker it plays on: the panel trusts the speaker when they
        # disagree (Music Assistant can say "playing" long after the speaker stopped)
        "player_links": _player_links(entry),
        # the area of the panel's device ("" when it has none)
        "room": _room(entry),
        # the speech recognition of the panel's assistant: what is said while a timer rings is checked with it for a
        # stop word (never sent to the assistant)
        "stt": _stt(entry),
        # the text-to-speech of the panel's assistant: the cameras page's replies at the door are spoken with it
        "tts": _tts(entry),
        **{key: float(options.get(key, default)) for key, (default, *_rest) in REPORTING.items()},
    }


def door_speaker(hass: HomeAssistant, cameras: list[str]) -> str:
    """The speaker at the door a cameras page starts with: the media player on a camera's own device (UniFi Protect,
    for one, has a camera's speaker as a media player of its device), the first camera's that has one; "" for none."""
    entities = er.async_get(hass)
    for camera in cameras:
        entity = entities.async_get(camera)
        if entity is None or not entity.device_id:
            continue
        for e in er.async_entries_for_device(entities, entity.device_id):
            if e.domain == "media_player":
                return e.entity_id
    return ""


def _door_speakers(entry: ConfigEntry, pages: list[dict[str, Any]]) -> list[dict[str, Any]]:
    """The cameras pages without a chosen speaker at the door get the one on their camera's device ("None" is kept as
    the empty text)."""
    panel = getattr(entry, "runtime_data", None)
    if panel is None:
        return pages
    for page in pages:
        if page["kind"] == "camera" and not isinstance(page["options"].get("speaker"), str):
            page["options"]["speaker"] = door_speaker(panel.hass, page["entities"])
    return pages


def _persons(entry: ConfigEntry) -> list[str]:
    """Every person.* entity of Home Assistant, in entity id order ([] before the panel's entry is set up)."""
    panel = getattr(entry, "runtime_data", None)
    if panel is None:
        return []
    return sorted(panel.hass.states.async_entity_ids("person"))


def _assist_pipeline(entry: ConfigEntry) -> Any:
    """The pipeline of the panel's assistant (its satellite's Assistant select), else the preferred one; None without."""
    panel = getattr(entry, "runtime_data", None)
    if panel is None:
        return None
    hass = panel.hass
    try:
        from homeassistant.components.assist_pipeline import async_get_pipeline, async_get_pipelines
    except ImportError:
        return None
    pipeline_id = None
    voice = panel._voice_entry()
    if voice is not None:
        for e in er.async_entries_for_config_entry(er.async_get(hass), voice.entry_id):
            if e.domain == "select" and e.unique_id.endswith("-pipeline"):
                state = hass.states.get(e.entity_id)
                if state is not None:
                    pipeline_id = next((p.id for p in async_get_pipelines(hass) if p.name == state.state), None)
                break
    try:
        return async_get_pipeline(hass, pipeline_id)
    except Exception:  # noqa: BLE001 - no pipelines (assist_pipeline not set up)
        return None


def _tts(entry: ConfigEntry) -> dict[str, str]:
    """The text-to-speech engine (an entity), language and voice of the panel's assistant; without one there, Home
    Assistant's default engine in its own language. {} when Home Assistant has none."""
    panel = getattr(entry, "runtime_data", None)
    if panel is None:
        return {}
    hass = panel.hass
    pipeline = _assist_pipeline(entry)
    engine = getattr(pipeline, "tts_engine", None) or ""
    if engine.startswith("tts.") and hass.states.get(engine) is not None:
        return {"engine": engine, "language": pipeline.tts_language or "", "voice": pipeline.tts_voice or ""}
    try:
        from homeassistant.components.tts import async_default_engine

        engine = async_default_engine(hass) or ""
    except Exception:  # noqa: BLE001 - text-to-speech not set up
        engine = ""
    if not engine.startswith("tts."):
        engine = next((s.entity_id for s in hass.states.async_all("tts")), "")
    return {"engine": engine, "language": "", "voice": ""} if engine else {}


def _room(entry: ConfigEntry) -> str:
    panel = getattr(entry, "runtime_data", None)
    if panel is None:
        return ""
    hass = panel.hass
    areas = ar.async_get(hass)
    for device in dr.async_entries_for_config_entry(dr.async_get(hass), entry.entry_id):
        area = areas.async_get_area(device.area_id) if device.area_id else None
        if area is not None:
            return area.name
    return ""


def _stt(entry: ConfigEntry) -> dict[str, str]:
    panel = getattr(entry, "runtime_data", None)
    if panel is None:
        return {}
    hass = panel.hass
    try:
        from homeassistant.components.assist_pipeline import async_get_pipeline, async_get_pipelines
    except ImportError:
        return {}
    # the pipeline the panel's satellite uses (its Assistant select), else the preferred one
    pipeline_id = None
    voice = panel._voice_entry()
    if voice is not None:
        for e in er.async_entries_for_config_entry(er.async_get(hass), voice.entry_id):
            if e.domain == "select" and e.unique_id.endswith("-pipeline"):
                state = hass.states.get(e.entity_id)
                if state is not None:
                    pipeline_id = next((p.id for p in async_get_pipelines(hass) if p.name == state.state), None)
                break
    try:
        pipeline = async_get_pipeline(hass, pipeline_id)
    except Exception:  # noqa: BLE001 - no pipelines (assist_pipeline not set up)
        return {}
    if not pipeline.stt_engine:
        return {}
    return {"engine": pipeline.stt_engine, "language": pipeline.stt_language or hass.config.language}


def _player_links(entry: ConfigEntry) -> dict[str, str]:
    panel = getattr(entry, "runtime_data", None)
    if panel is None:
        return {}
    hass = panel.hass
    devices = dr.async_get(hass)
    name_of = {}
    for d in devices.devices:
        name_of[d.id] = (d.name_by_user or d.name or "").strip().lower()
    speakers: dict[str, str] = {}
    mass: list[tuple[str, str]] = []
    for e in er.async_get(hass).entities.values():
        if e.domain != "media_player" or e.disabled or not e.device_id:
            continue
        name = name_of.get(e.device_id, "")
        if not name:
            continue
        if e.platform == "music_assistant":
            mass.append((e.entity_id, name))
        else:
            speakers.setdefault(name, e.entity_id)
    return {ma: speakers[name] for ma, name in mass if name in speakers}


def _ma_player(entry: ConfigEntry) -> str:
    panel = getattr(entry, "runtime_data", None)
    if panel is None:
        return ""
    from .ma import ma_entity  # noqa: PLC0415

    try:
        return ma_entity(panel.hass, _media_player(entry))
    except Exception:  # noqa: BLE001 - Music Assistant away or changed: the panel uses its own player
        return ""


def _media_player(entry: ConfigEntry) -> str:
    """The panel's media player entity (on its voice device, connected via the panel's device)."""
    panel = getattr(entry, "runtime_data", None)
    if panel is None or not panel.device_id:
        return ""
    # iterating the registry's devices (not using it as a mapping) is the supported way
    voice = {d.id for d in dr.async_get(panel.hass).devices if d.via_device_id == panel.device_id} | {panel.device_id}
    for e in er.async_get(panel.hass).entities.values():
        if e.domain == "media_player" and e.device_id in voice and not e.disabled:
            return e.entity_id
    return ""


def voice_player(hass: HomeAssistant, satellite_id: str) -> str | None:
    """The player music commands said to a panel act on (voice_device_context asks for it with ``player: panelos``):
    the player picked on the panel's music page, or the panel's own player when the integration says so. None when
    the satellite belongs to no panel."""
    entity = er.async_get(hass).async_get(satellite_id) if satellite_id else None
    device = dr.async_get(hass).async_get(entity.device_id) if entity and entity.device_id else None
    if device is None:
        return None
    for entry in hass.config_entries.async_entries(DOMAIN):
        panel = getattr(entry, "runtime_data", None)
        if panel is None or not panel.device_id or panel.device_id not in (device.id, device.via_device_id):
            continue
        options = {**entry.data, **entry.options}
        own = _ma_player(entry) or _media_player(entry)
        if options.get(CONF_VOICE_PLAYER, "selected") == "self":
            return own
        return panel.music_player or options.get(CONF_MEDIA_PLAYER, "") or own
    return None


def _keep_sounds(entry: ConfigEntry) -> list[str]:
    """The uploaded sounds of the shared critical alerts this panel shows (it keeps a copy of each)."""
    panel = getattr(entry, "runtime_data", None)
    house = panel.hass.data.get(DOMAIN, {}).get("house") if panel else None
    return house.keep_sounds(entry.entry_id) if house else []


def shown_pages(pages: list[dict[str, Any]]) -> list[dict[str, Any]]:
    """The pages the panel swipes through: the hidden ones are left out (the first, the start page, never is)."""
    return [
        {"kind": page["kind"], "variant": int(page.get("variant", 0)), "entities": list(page.get("entities", [])), "options": dict(page.get("options", {}))}
        for i, page in enumerate(pages)
        if i == 0 or not page.get("hidden")
    ]


def _time(value: Any, default: str) -> str:
    return value if isinstance(value, str) and len(value) == 5 and value[2] == ":" else default


def screensaver_config(stored: Any) -> dict[str, Any]:
    """When the screensaver may start, with the defaults filled in."""
    s = {**DEFAULT_SCREENSAVER, **(stored if isinstance(stored, dict) else {})}
    lux = s.get("lux")
    return {
        "when": s["when"] if s.get("when") in SCREENSAVER_WHEN else DEFAULT_SCREENSAVER["when"],
        "lux": lux if isinstance(lux, (int, float)) and not isinstance(lux, bool) else DEFAULT_SCREENSAVER["lux"],
        "from": _time(s.get("from"), DEFAULT_SCREENSAVER["from"]),
        "to": _time(s.get("to"), DEFAULT_SCREENSAVER["to"]),
        "light_entity": s["light_entity"] if isinstance(s.get("light_entity"), str) else "",
        # the clock on it
        "face": _int(s.get("face"), 1, 0, 10),
        "seconds": s.get("seconds", True) is not False,
        "date": s.get("date", True) is not False,
        "weather": s.get("weather", True) is not False,
        "next": s.get("next", True) is not False,
        "weight": _int(s.get("weight"), 600, 300, 800),
        "width": _int(s.get("width"), 100, 70, 130),
        "speed": _int(s.get("speed"), 20, 5, 60),
        "colour": s["colour"] if isinstance(s.get("colour"), str) else "",
    }


def theme_auto_config(stored: Any) -> dict[str, Any]:
    """What switches the Auto theme, with the defaults filled in."""
    s = stored if isinstance(stored, dict) else {}
    return {
        "by": s["by"] if s.get("by") in THEME_AUTO_BY else DEFAULT_THEME_AUTO["by"],
        "from": _time(s.get("from"), DEFAULT_THEME_AUTO["from"]),
        "to": _time(s.get("to"), DEFAULT_THEME_AUTO["to"]),
    }


def _int(value: Any, default: int, low: int, high: int) -> int:
    """A whole number within a range, else the default (stored options may be old or hand-made)."""
    if isinstance(value, bool) or not isinstance(value, (int, float)):
        return default
    return max(low, min(high, int(value)))


def dnd_config(stored: Any) -> dict[str, Any]:
    """Do not disturb, with the defaults filled in."""
    s = stored if isinstance(stored, dict) else {}
    allow = s.get("allow") if isinstance(s.get("allow"), dict) else {}
    return {
        "mode": s["mode"] if s.get("mode") in DND_MODES else DEFAULT_DND["mode"],
        "from": _time(s.get("from"), DEFAULT_DND["from"]),
        "to": _time(s.get("to"), DEFAULT_DND["to"]),
        "allow": {key: bool(allow.get(key, DEFAULT_DND["allow"][key])) for key in DND_ALLOW},
    }


def popups_config(stored: Any) -> dict[str, Any]:
    """How new notifications show, with the defaults filled in."""
    s = {**DEFAULT_POPUPS, **(stored if isinstance(stored, dict) else {})}
    seconds = s.get("seconds")
    return {
        "kind": s["kind"] if s.get("kind") in POPUP_KINDS else DEFAULT_POPUPS["kind"],
        "sound": bool(s.get("sound")),
        "seconds": int(seconds) if isinstance(seconds, (int, float)) and not isinstance(seconds, bool) else DEFAULT_POPUPS["seconds"],
        "until": bool(s.get("until")),
    }


def panel_caps(entry: ConfigEntry) -> list[str]:
    """What the panel can do, as it reported it (an NSPanel Pro until it does)."""
    caps = entry.data.get(CONF_CAPS)
    return list(caps) if isinstance(caps, list) else list(DEFAULT_CAPS)


def _number(value: Any, low: int, high: int) -> int:
    return max(low, min(high, int(value))) if isinstance(value, (int, float)) and not isinstance(value, bool) else low


def _text(value: Any, most: int) -> str:
    return value[:most] if isinstance(value, str) else ""


def _alarm(value: dict[str, Any]) -> dict[str, Any]:
    """One alarm as the panel reports it, with only the fields the page uses."""
    days = value.get("days") if isinstance(value.get("days"), list) else []
    days = [bool(d) for d in days[:7]]
    return {
        "h": _number(value.get("h"), 0, 23),
        "m": _number(value.get("m"), 0, 59),
        "days": days + [False] * (7 - len(days)),
        "label": _text(value.get("label"), 32),
        "sound": _number(value.get("sound"), 0, 5),
        "on": bool(value.get("on", True)),
        "tone": _text(value.get("tone"), 120),
        "tone_name": _text(value.get("tone_name"), 60),
    }


def _alarm_key(a: dict[str, Any]) -> tuple:
    """What makes two alarms the same (as set on the page and as the panel reports it)."""
    days = [bool(d) for d in (a.get("days") or [])[:7]]
    return (int(a.get("h", 0)), int(a.get("m", 0)), tuple(days + [False] * (7 - len(days))), str(a.get("label", "")),
            bool(a.get("on", True)), str(a.get("tone", "auto") or "auto"))


def _scalar(value: Any) -> bool:
    return value is None or isinstance(value, (bool, int, float)) or (isinstance(value, str) and len(value) <= _TEXT_MAX)


def _user_settings(entry: er.RegistryEntry | er.DeletedRegistryEntry | None) -> dict[str, Any]:
    """What a user may have set on an entity, to carry over to its new entry."""
    if entry is None:
        return {}
    settings: dict[str, Any] = {
        key: value
        for key, value in (
            ("name", entry.name),
            ("icon", entry.icon),
            ("area_id", entry.area_id),
            ("hidden_by", entry.hidden_by),
            ("disabled_by", entry.disabled_by),
            ("aliases", entry.aliases or None),
            ("labels", entry.labels or None),
        )
        if value is not None and value is not UNDEFINED
    }
    if entry.options and entry.options is not UNDEFINED:
        settings["options"] = {domain: dict(options) for domain, options in entry.options.items()}
    return settings


class Panel:
    """Live data and device of one panel (the entry's runtime data)."""

    def __init__(self, hass: HomeAssistant, entry: ConfigEntry) -> None:
        # the reconnect grace (see `reachable`)
        self._gone_since: float | None = None
        self._grace_cancel: Any = None
        self._ever_connected = False
        self.hass = hass
        self.entry = entry
        self.panel_id: str = {**entry.data, **entry.options}.get(CONF_PANEL_ID, "")
        self.states: dict[str, Any] = {}
        self.options: dict[str, list[str]] = {}
        self.device_id: str | None = None
        self.signal = f"{DOMAIN}_update_{entry.entry_id}"
        self._adders: dict[str, tuple[AddEntitiesCallback, Factory]] = {}
        self._added: set[str] = set()
        # Entities an older PanelOS on the panel still has on its ESPHome
        # device (entity id → key): ours follow once ESPHome drops them, with
        # the same entity id and the user's settings.
        self._waiting: dict[str, str] = {}
        self._saved: dict[str, dict[str, Any]] = {}
        self._unsub_waiting: Callable[[], None] | None = None
        # Pages waiting for a picture of the screen (panelos/page/screen); the newest picture (data,
        # type, when) and when the last one was handed out and the panel last asked.
        self._screen_waiters: list[asyncio.Future[tuple[bytes, str]]] = []
        self._screen_latest: tuple[bytes, str, float] | None = None
        self._screen_given = 0.0
        self._screen_asked = 0.0
        # The dashboard the web view shows (the panel reports it) and the
        # card resources it leaves out there (worked out by refresh_skip).
        self.web_dashboard = ""
        # The player picked on the panel's music page (it reports it), "" until it does.
        self.music_player = ""
        # The panel's alarms as it reports them (they live on the panel), None until it does.
        self.alarms: list[dict[str, Any]] | None = None
        # Alarms set on the PanelOS page that the panel has not confirmed yet: kept (also over a restart)
        # and sent again until the panel reports exactly them.
        self.pending_alarms: list[dict[str, Any]] | None = None
        self._pending_store: Store = Store(hass, 1, f"panelos.pending.{entry.entry_id}")
        self._pending_sent = 0.0
        self.skip: list[str] = []
        self.resources: list[dict[str, Any]] = []
        self._skip_task: asyncio.Task | None = None

    def unique_id(self, key: str) -> str:
        return f"{self.panel_id}-{key}"

    # -- changes kept until the panel confirms them ---------------------------------

    async def async_load_pending(self) -> None:
        data = await self._pending_store.async_load()
        alarms = data.get("alarms") if isinstance(data, dict) else None
        self.pending_alarms = alarms if isinstance(alarms, list) else None

    @callback
    def set_pending_alarms(self, alarms: list[dict[str, Any]]) -> bool:
        """Keeps the alarms until the panel reports them; sends them now if it is connected (True)."""
        self.pending_alarms = alarms
        self._pending_store.async_delay_save(lambda: {"alarms": alarms}, 1)
        return self._send_pending(force=True)

    @callback
    def _send_pending(self, force: bool = False) -> bool:
        if self.pending_alarms is None or not self.connected:
            return False
        now = time.monotonic()
        if not force and now - self._pending_sent < 20:
            return False
        self._pending_sent = now
        async_dispatcher_send(self.hass, SIGNAL_SHOW, self.panel_id, {"type": "alarms", "list": self.pending_alarms})
        return True

    @callback
    def _confirm_pending(self, reported: list[dict[str, Any]]) -> None:
        """The panel reported its alarms: the kept ones are done when they match, else sent again."""
        if self.pending_alarms is None:
            return
        if [_alarm_key(a) for a in reported] == [_alarm_key(a) for a in self.pending_alarms]:
            self.pending_alarms = None
            self._pending_store.async_delay_save(lambda: {}, 1)
        else:
            self._send_pending()

    # -- links -------------------------------------------------------------

    def _sends(self, connection_id: int | None = None) -> list[Send]:
        return [
            send
            for pid, cid, send in links(self.hass).values()
            if (connection_id is None or cid == connection_id) and find_entry(self.hass, pid) is self.entry and not link_gone(self.hass, cid)
        ]

    @property
    def connected(self) -> bool:
        return bool(self._sends())

    def linked_on(self, connection_id: int) -> bool:
        """True when this connection follows the panel's subscription."""
        return bool(self._sends(connection_id))

    def send_command(self, key: str, value: Any) -> None:
        sends = self._sends()
        if not sends:
            raise HomeAssistantError(f"The PanelOS panel '{self.panel_id}' is not connected")
        for send in sends:
            send({"key": key, "value": value})

    @property
    def reachable(self) -> bool:
        """Connected, or away for less than RECONNECT_GRACE: the entities keep their last state through a short
        reconnect (a Wi-Fi hiccup, the panel's app restarting) instead of turning unavailable and back."""
        if self.connected:
            self._gone_since = None
            return True
        return self._gone_since is not None and time.monotonic() - self._gone_since < RECONNECT_GRACE

    @callback
    def _links_changed(self) -> None:
        if self.connected:
            self._gone_since = None
            if self._grace_cancel is not None:
                self._grace_cancel()
                self._grace_cancel = None
        elif self._gone_since is None and self._ever_connected:
            self._gone_since = time.monotonic()
            # when the grace is over, the entities look again (unavailable now, unless the panel is back)
            self._grace_cancel = async_call_later(self.hass, RECONNECT_GRACE + 0.5, self._grace_over)
        if self.connected:
            self._ever_connected = True
        async_dispatcher_send(self.hass, self.signal, None)

    @callback
    def _grace_over(self, _now: Any) -> None:
        self._grace_cancel = None
        async_dispatcher_send(self.hass, self.signal, None)

    @callback
    def _cancel_grace(self) -> None:
        if self._grace_cancel is not None:
            self._grace_cancel()
            self._grace_cancel = None

    @callback
    def keep_for_reload(self) -> None:
        """The entry unloads: its last states and the reconnect state wait for the entry set up again (a reload),
        so the entities come back with their states and a drop gets its grace as before."""
        kept(self.hass)[self.entry.entry_id] = {
            "at": time.monotonic(),
            "states": dict(self.states),
            "options": {k: list(v) for k, v in self.options.items()},
            "ever": self._ever_connected or self.connected,
            "gone_since": None if self.connected else self._gone_since,
        }

    @callback
    def _resume_after_reload(self) -> None:
        """Set up again soon after an unload (a reload): the last states and the grace go on. A panel that stays
        connected through the reload counts as connected before, so its next drop gets the grace too."""
        old = kept(self.hass).pop(self.entry.entry_id, None)
        now = time.monotonic()
        if old and now - old["at"] < KEPT_FOR_RELOAD:
            self.states.update(old["states"])
            self.options.update(old["options"])
            self._ever_connected = bool(old["ever"])
            if self._ever_connected and not self.connected:
                # away during the reload: the grace runs from when it went away (or from the reload)
                self._gone_since = old["gone_since"] if old["gone_since"] is not None else old["at"]
                left = RECONNECT_GRACE - (now - self._gone_since)
                if left > 0:
                    self._grace_cancel = async_call_later(self.hass, left + 0.5, self._grace_over)
        if self.connected:
            self._ever_connected = True

    # -- screen picture ----------------------------------------------------

    async def async_screenshot(self, timeout: float = 8) -> tuple[bytes, str]:
        """A picture of what the panel shows now and its type. The panel streams for a few seconds after
        each request, so a picture newer than the last one handed out is returned at once."""
        now = time.monotonic()
        if now - self._screen_asked > 1.0:
            self._screen_asked = now
            self.send_command("screenshot", None)
        latest = self._screen_latest
        if latest is not None and latest[2] > self._screen_given and now - latest[2] < 1.0:
            self._screen_given = latest[2]
            return latest[0], latest[1]
        future: asyncio.Future[tuple[bytes, str]] = self.hass.loop.create_future()
        self._screen_waiters.append(future)
        try:
            async with asyncio.timeout(timeout):
                return await future
        except TimeoutError as err:
            raise HomeAssistantError(f"The PanelOS panel '{self.panel_id}' sent no picture") from err
        finally:
            if future in self._screen_waiters:
                self._screen_waiters.remove(future)

    @callback
    def screen_received(self, data: bytes, mime: str = "image/png") -> None:
        now = time.monotonic()
        self._screen_latest = (data, mime, now)
        waiters, self._screen_waiters = self._screen_waiters, []
        if waiters:
            self._screen_given = now
        for future in waiters:
            if not future.done():
                future.set_result((data, mime))

    # -- setup -------------------------------------------------------------

    @callback
    def async_setup(self) -> None:
        """Register the device, link the voice device and take over old entities."""
        info = dict(self.entry.data.get(CONF_DEVICE, {}))
        voice_entry = self._voice_entry()
        if not info.get("mac") and voice_entry and voice_entry.unique_id:
            info["mac"] = voice_entry.unique_id
        if not info.get("name") and voice_entry:
            info["name"] = voice_entry.title
        self._register_device(info)
        if voice_entry:
            self._link_voice_device(voice_entry)
        if info.get("mac"):
            self._take_over_esphome_entities(info["mac"])
        self._resume_after_reload()
        self.entry.async_on_unload(async_dispatcher_connect(self.hass, SIGNAL_LINKS, self._links_changed))
        self.entry.async_on_unload(self._cancel_grace)
        self.entry.async_on_unload(self._stop_waiting)
        # A dashboard saved in Home Assistant may need other card resources.
        self.entry.async_on_unload(self.hass.bus.async_listen("lovelace_updated", self._lovelace_updated))
        # Dashboards and resources are there once Home Assistant has started.
        self.entry.async_on_unload(async_at_started(self.hass, self._started))

    @callback
    def _started(self, _hass: HomeAssistant) -> None:
        # a callback: it runs in the event loop (a plain function would run in a worker thread)
        self.refresh_skip()

    @callback
    def _lovelace_updated(self, event: Event) -> None:
        path = event.data.get("url_path") or "lovelace"
        if path in (self.dashboard, None) or (path == "lovelace" and self.dashboard in ("", "lovelace")):
            self.refresh_skip()

    # -- timers ------------------------------------------------------------
    # The panel's timers are Home Assistant's own (its timer manager, the ones voice starts and stops): started,
    # paused, extended and cancelled from the panel too, so voice, the panel and any other device see the same.

    def timer_device_id(self) -> str | None:
        """The panel's voice satellite device: Home Assistant's timers belong to it."""
        voice = self._voice_entry()
        if voice is None:
            return None
        manager = self.hass.data.get(TIMER_DATA)
        for device in dr.async_entries_for_config_entry(dr.async_get(self.hass), voice.entry_id):
            if manager is not None and manager.is_timer_device(device.id):
                return device.id
        return None

    def timers_now(self) -> list[dict[str, Any]]:
        """The timers running on the panel now (for a panel that just connected)."""
        manager = self.hass.data.get(TIMER_DATA)
        device_id = self.timer_device_id()
        if manager is None or device_id is None:
            return []
        return [
            {"id": t.id, "name": t.name or "", "total": t.seconds, "left": t.seconds_left, "active": t.is_active}
            for t in manager.timers.values()
            if t.device_id == device_id and not t.conversation_command
        ]

    def _voice_entry(self) -> ConfigEntry | None:
        """The panel's ESPHome entry (voice satellite): same device name."""
        for entry in self.hass.config_entries.async_entries("esphome"):
            if entry.data.get("device_name") == self.panel_id:
                return entry
        return None

    def _register_device(self, info: dict[str, str]) -> None:
        connections = {(dr.CONNECTION_NETWORK_MAC, dr.format_mac(info["mac"]))} if info.get("mac") else set()
        device = dr.async_get(self.hass).async_get_or_create(
            config_entry_id=self.entry.entry_id,
            identifiers={(DOMAIN, self.panel_id)},
            connections=connections,
            name=info.get("name") or self.panel_id,
            manufacturer=info.get("manufacturer") or "PanelOS",
            model=info.get("model"),
            sw_version=info.get("sw_version"),
            hw_version=info.get("hw_version"),
        )
        self.device_id = device.id

    def _link_voice_device(self, voice_entry: ConfigEntry) -> None:
        """The ESPHome device is "connected via" this one (the panel)."""
        dev_reg = dr.async_get(self.hass)
        for device in dr.async_entries_for_config_entry(dev_reg, voice_entry.entry_id):
            if device.via_device_id is not None:
                continue
            dev_reg.async_update_device(device.id, via_device_id=self.device_id)
            ours = dev_reg.async_get(self.device_id) if self.device_id else None
            if ours and ours.area_id is None and device.area_id:
                dev_reg.async_update_device(ours.id, area_id=device.area_id)

    # -- entities ----------------------------------------------------------

    def _take_over_esphome_entities(self, mac: str) -> None:
        """Until PanelOS 0.0.3 these entities were on the ESPHome device."""
        ent_reg = er.async_get(self.hass)
        loaded = entity_sources(self.hass)
        for key, (esphome_platform, name) in ESPHOME_ENTITIES.items():
            domain = KEY_DOMAIN[key]
            if ent_reg.async_get_entity_id(domain, DOMAIN, self.unique_id(key)):
                continue
            old_unique_id = f"{mac.upper()}/0/{esphome_platform}/{name}"
            old_id = ent_reg.async_get_entity_id(domain, "esphome", old_unique_id)
            if old_id is None:
                # Already dropped by ESPHome (the panel runs a newer PanelOS):
                # the same entity id, while it is free.
                deleted = ent_reg.deleted_entities.get((domain, "esphome", old_unique_id))
                if (
                    deleted
                    and not ent_reg.async_is_registered(deleted.entity_id)
                    and self.hass.states.async_available(deleted.entity_id)
                ):
                    self._create_entry(key, deleted.entity_id, _user_settings(deleted))
                continue
            if old_id in loaded:
                self._waiting[old_id] = key
                self._saved[old_id] = _user_settings(ent_reg.async_get(old_id))
                continue
            ent_reg.async_update_entity_platform(
                old_id,
                DOMAIN,
                new_config_entry_id=self.entry.entry_id,
                new_unique_id=self.unique_id(key),
                new_device_id=self.device_id,
            )
            _LOGGER.info("%s moved from the panel's ESPHome device to its PanelOS device", old_id)
        if self._waiting and self._unsub_waiting is None:
            _LOGGER.warning(
                "The panel '%s' still has %s on its ESPHome device (an older PanelOS); "
                "they move here once the panel runs PanelOS 0.0.4 or later",
                self.panel_id,
                ", ".join(sorted(self._waiting)),
            )
            self._unsub_waiting = self.hass.bus.async_listen(
                er.EVENT_ENTITY_REGISTRY_UPDATED, self._registry_updated, event_filter=self._is_waiting_removal
            )

    @callback
    def _stop_waiting(self) -> None:
        if self._unsub_waiting:
            self._unsub_waiting()
            self._unsub_waiting = None

    @callback
    def _is_waiting_removal(self, data: er.EventEntityRegistryUpdatedData) -> bool:
        return data["action"] == "remove" and data["entity_id"] in self._waiting

    @callback
    def _registry_updated(self, event: Event[er.EventEntityRegistryUpdatedData]) -> None:
        old_id = event.data["entity_id"]
        key = self._waiting.pop(old_id)
        if not self._waiting:
            self._stop_waiting()
        self.entry.async_create_task(self.hass, self._add_moved(old_id, key))

    async def _add_moved(self, old_id: str, key: str) -> None:
        # ESPHome removes the entity itself just after its registry entry.
        for _ in range(50):
            if self.hass.states.get(old_id) is None:
                break
            await asyncio.sleep(0.1)
        self._create_entry(key, old_id, self._saved.pop(old_id, {}))
        self._add(key)

    def _create_entry(self, key: str, entity_id: str, saved: dict[str, Any]) -> None:
        """Registry entry of one of our entities with an old entity id and settings."""
        ent_reg = er.async_get(self.hass)
        new = ent_reg.async_get_or_create(
            KEY_DOMAIN[key],
            DOMAIN,
            self.unique_id(key),
            config_entry=self.entry,
            device_id=self.device_id,
            suggested_object_id=entity_id.split(".", 1)[1],
        )
        options = saved.pop("options", {})
        if saved:
            ent_reg.async_update_entity(new.entity_id, **saved)
        for domain, values in options.items():
            ent_reg.async_update_entity_options(new.entity_id, domain, values)
        _LOGGER.info("%s moved from the panel's ESPHome device to its PanelOS device", new.entity_id)

    @callback
    def add_platform(self, domain: str, add_entities: AddEntitiesCallback, factory: Factory) -> None:
        """Called by each platform: adds its entities (later the waiting ones)."""
        self._adders[domain] = (add_entities, factory)
        waiting = set(self._waiting.values())
        for key in ENTITY_KEYS[domain]:
            if key not in waiting:
                self._add(key)

    @callback
    def _add(self, key: str) -> None:
        adder = self._adders.get(KEY_DOMAIN[key])
        if adder is None or key in self._added:
            return
        self._added.add(key)
        add_entities, factory = adder
        add_entities([factory(self, key)])

    # -- reports -----------------------------------------------------------

    @callback
    def apply_report(self, msg: dict[str, Any]) -> None:
        changed: set[str] = set()
        for key, value in (msg.get("states") or {}).items():
            if key in KEY_DOMAIN and _scalar(value) and self.states.get(key, _MISSING) != value:
                self.states[key] = value
                changed.add(key)
        for key, options in (msg.get("options") or {}).items():
            if key in _SELECT_KEYS and isinstance(options, list):
                clean = [o[:_TEXT_MAX] for o in options[:_OPTIONS_MAX] if isinstance(o, str)]
                if self.options.get(key) != clean:
                    self.options[key] = clean
                    changed.add(key)
        if isinstance(device := msg.get("device"), dict):
            self._device_reported(device)
        if isinstance(alarms := msg.get("alarms"), list):
            clean = [_alarm(a) for a in alarms[:_ALARMS_MAX] if isinstance(a, dict)]
            if clean != self.alarms:
                self.alarms = clean
                changed.add("alarms")
            pending = self.pending_alarms is not None
            self._confirm_pending(clean)
            if pending and self.pending_alarms is None:
                changed.add("alarms")
        elif self.pending_alarms is not None:
            # a report without alarms (an older panel or between): the kept ones go again
            self._send_pending()
        if isinstance(player := msg.get("music_player"), str) and player != self.music_player:
            self.music_player = player[:_TEXT_MAX]
        if isinstance(dashboard := msg.get("dashboard"), str) and dashboard != self.web_dashboard:
            self.web_dashboard = dashboard[:_TEXT_MAX]
            self.refresh_skip()
            changed.add("dashboard")
        if changed:
            async_dispatcher_send(self.hass, self.signal, changed)
            async_dispatcher_send(self.hass, SIGNAL_PAGE)

    # -- card resources ------------------------------------------------------

    @property
    def dashboard(self) -> str:
        """The dashboard the web view shows: the one set here, else the panel's."""
        return {**self.entry.data, **self.entry.options}.get(CONF_DASHBOARD) or self.web_dashboard

    @callback
    def refresh_skip(self) -> None:
        """Works out again which card resources the shown dashboard needs."""
        if self._skip_task and not self._skip_task.done():
            self._skip_task.cancel()
        self._skip_task = self.entry.async_create_task(self.hass, self._async_refresh_skip())

    async def _async_refresh_skip(self) -> None:
        options = {**self.entry.data, **self.entry.options}
        dashboard = self.dashboard
        self.resources = await async_resource_list(self.hass, dashboard) if dashboard else []
        keep = set(options.get(CONF_KEEP_RESOURCES, []))
        skip = (
            [r["url"].split("?")[0] for r in self.resources if not r["needed"] and r["url"] not in keep]
            if options.get(CONF_SKIP_UNUSED, True)
            else []
        )
        async_dispatcher_send(self.hass, SIGNAL_PAGE)
        if skip != self.skip:
            self.skip = skip
            async_dispatcher_send(self.hass, SIGNAL_CONFIG_UPDATED, self.panel_id)

    def _device_reported(self, device: dict[str, Any]) -> None:
        info = {key: device[key][:64] for key in _DEVICE_FIELDS if isinstance(device.get(key), str) and device[key]}
        caps = device.get("caps")
        if isinstance(caps, list):
            caps = sorted({c[:_CAP_MAX] for c in caps[:_CAPS_MAX] if isinstance(c, str) and c})
            if caps != self.entry.data.get(CONF_CAPS):
                self.hass.config_entries.async_update_entry(self.entry, data={**self.entry.data, CONF_CAPS: caps})
                async_dispatcher_send(self.hass, SIGNAL_PAGE)
        if info == self.entry.data.get(CONF_DEVICE):
            return
        self.hass.config_entries.async_update_entry(self.entry, data={**self.entry.data, CONF_DEVICE: info})
        self._register_device(info)
        if voice_entry := self._voice_entry():
            self._link_voice_device(voice_entry)
