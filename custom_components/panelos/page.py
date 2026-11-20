"""The PanelOS page in the sidebar: every panel's settings beside a live picture of its screen.

For administrators. The page uses these websocket commands:

* ``panelos/page/subscribe``: the panels with their device, states and
  settings, again after every change;
* ``panelos/page/options``: change integration options of a panel (dashboard
  and assistant mode, sensor reporting);
* ``panelos/page/command``: set a setting of the panel's device, the same as
  its entities;
* ``panelos/page/screen``: a fresh PNG of what the panel shows;
* ``panelos/page/resources``: the card resources with their size and whether
  the panel's dashboard needs them (for the resources it should skip);
* ``panelos/page/house``: store the critical alerts and status screens shared
  by every panel (``house.py``);
* ``panelos/page/test``: show a critical alert or a status screen on one panel
  for a few seconds;
* ``panelos/page/touch_test``: start the panel's touch calibration (its
  nine-point test; the result shows on the panel);
* ``panelos/page/proximity_calibration``: start the panel's wake distance
  calibration (it measures the proximity sensor's resting level; the result
  shows on the panel);
* ``panelos/page/forget_user``: forget the panel's Home Assistant user (the
  next user that subscribes for the panel is kept).

The panel sends that picture with ``panelos/screen`` when its subscription
asks for it (command ``screenshot``).
"""

from __future__ import annotations

import base64
import binascii
import hashlib
import json
import logging
import math
from pathlib import Path
import re
from typing import Any

import voluptuous as vol

from homeassistant.components import frontend, panel_custom, websocket_api
from homeassistant.components.http import StaticPathConfig
from homeassistant.config_entries import ConfigEntryState
from homeassistant.core import CALLBACK_TYPE, HomeAssistant, callback
from homeassistant.exceptions import HomeAssistantError
from homeassistant.helpers import config_validation as cv
from homeassistant.helpers.dispatcher import async_dispatcher_connect, async_dispatcher_send
from homeassistant.helpers.event import async_call_later
from homeassistant.loader import async_get_integration

from .config_flow import dashboard_choices
from .const import (
    CONF_DASHBOARD,
    CONF_DEVICE,
    CONF_MEDIA_PLAYER,
    CONF_SHOW_CONVERSATION,
    CONF_VOICE_LOOK,
    CONF_VOICE_PLAYER,
    VOICE_PLAYERS,
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
    CONF_DND,
    CONF_POPUPS,
    CONF_SCREENSAVER,
    CONF_THEME_AUTO,
    CONF_WAKE_DISTANCE,
    CONF_WIFI_ROAMING,
    DEFAULT_PAGES,
    DND_ALLOW,
    DND_MODES,
    DOMAIN,
    MAX_PAGES,
    PAGE_KINDS,
    POPUP_KINDS,
    REPORTING,
    SAFETY_CLASSES,
    SCREENSAVER_WHEN,
    SIGNAL_CONFIG_UPDATED,
    SIGNAL_LINKS,
    SIGNAL_PAGE,
    SIGNAL_SHOW,
    STATUS_CATS,
    THEME_AUTO_BY,
    TILE_DOMAINS,
    WAKE_DISTANCES,
)
from .house import HOUSE_SCHEMA, House, _tone
from .panel import KEY_DOMAIN, Panel, find_entry, panel_caps, panel_config, panel_user, set_panel_user

_LOGGER = logging.getLogger(__name__)
from .resources import async_resource_list

FRONTEND_DIR = Path(__file__).parent / "frontend"
STATIC_URL = "/panelos_static"
PAGE_URL = "panelos"
PAGE_ELEMENT = "panelos-page"
_PNG_MAX = 4 * 1024 * 1024
_DASHBOARD_PATH = re.compile(r"^[a-z0-9_-]{0,64}$")
_EMPTY_CLEARS = (CONF_WEATHER, CONF_MEDIA_PLAYER)

_OPTIONS_JSON_MAX = 16000
_OPTION_TEXT_MAX = 200
_OPTION_LIST_MAX = 64
_OPTION_KEYS_MAX = 32


def _option_value(value: Any, depth: int = 0) -> Any:
    """A page option: a scalar, or (two levels deep at most) a list or a small object of them."""
    if value is None or isinstance(value, bool):
        return value
    if isinstance(value, (int, float)):
        if isinstance(value, float) and not math.isfinite(value):
            raise vol.Invalid("numbers must be finite")
        return value
    if isinstance(value, str):
        if len(value) > _OPTION_TEXT_MAX:
            raise vol.Invalid(f"texts are at most {_OPTION_TEXT_MAX} characters")
        return value
    if depth < 3 and isinstance(value, list):
        if len(value) > _OPTION_LIST_MAX:
            raise vol.Invalid(f"lists have at most {_OPTION_LIST_MAX} items")
        return [_option_value(v, depth + 1) for v in value]
    if depth < 3 and isinstance(value, dict):
        if len(value) > _OPTION_KEYS_MAX or any(not isinstance(k, str) or len(k) > 40 for k in value):
            raise vol.Invalid(f"objects have at most {_OPTION_KEYS_MAX} keys of up to 40 characters")
        return {k: _option_value(v, depth + 1) for k, v in value.items()}
    raise vol.Invalid("not a valid page option")


def _page_options(value: Any) -> dict[str, Any]:
    """A page's options: plain ones (see _option_value); its widgets ("items") have their own schema (_HOME_ITEM)."""
    if not isinstance(value, dict):
        raise vol.Invalid("the options are an object")
    options = _option_value({k: v for k, v in value.items() if k != "items"})
    if "items" in value:
        options["items"] = value["items"]
    return options


# A widget of a custom or weather details page on its 4 x 4 grid, as the panel
# draws it (app/ui/home.slint, HomeItem; panelos-page.js, WIDGETS): kind 0 clock,
# 1 date, 2 weather now, 3 next event, 4 forecast curve, 5 hourly forecast,
# 6 daily forecast, 7 music, 8 a sensor's value, 9 the running timer, 10 the
# people at home, 11 weather details; style and opts per kind; the sensor
# widget names its sensor, the People widget the people it shows (none: everyone).
_WIDGET_KINDS = (0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11)
_PEOPLE_MAX = 32
# the weather details page offers the weather widgets only
_WEATHER_WIDGET_KINDS = (2, 4, 5, 6, 11)
_HOME_ITEM = vol.Schema(
    {
        vol.Required("kind"): vol.In(_WIDGET_KINDS),
        vol.Required("col"): vol.All(int, vol.Range(min=0, max=3)),
        vol.Required("row"): vol.All(int, vol.Range(min=0, max=3)),
        vol.Required("w"): vol.All(int, vol.Range(min=1, max=4)),
        vol.Required("h"): vol.All(int, vol.Range(min=1, max=4)),
        vol.Optional("style", default=0): vol.All(int, vol.Range(min=0, max=99)),
        vol.Optional("accent", default=""): vol.Any("", vol.All(str, vol.Match(r"^#[0-9a-fA-F]{6}$"))),
        vol.Optional("weight", default=600): vol.All(int, vol.Range(min=100, max=900)),
        vol.Optional("tap", default=True): bool,
        # neon letters on the synthwave background (each widget its own; without it: the page's old switch)
        vol.Optional("neon"): bool,
        vol.Optional("opts", default=0): vol.All(int, vol.Range(min=0, max=0xFFFF)),
        # the card behind the widget: -1 = its own (as designed), 0..100 % opacity
        vol.Optional("bg", default=-1): vol.All(int, vol.Range(min=-1, max=100)),
        # the sensor widget's sensor
        vol.Optional("entity"): vol.Any("", cv.entity_domain("sensor")),
        # the People widget's people (none: everyone, the ones added later too)
        vol.Optional("people"): vol.All([cv.entity_domain("person")], vol.Length(max=_PEOPLE_MAX)),
    }
)


def _home_items(items: Any, kinds: tuple[int, ...] = _WIDGET_KINDS) -> list[dict[str, Any]]:
    """Widgets may lie on each other (the later one on top; the page warns to check them on the panel)."""
    items = vol.Schema(vol.All([_HOME_ITEM], vol.Length(max=16)))(items)
    for item in items:
        if item["col"] + item["w"] > 4 or item["row"] + item["h"] > 4:
            raise vol.Invalid("a widget is outside the 4 x 4 grid")
        if item["kind"] not in kinds:
            raise vol.Invalid("the weather details page takes weather widgets only")
    return items


def _page(value: Any) -> dict[str, Any]:
    """One page of the panel: its kind, look, entities (of the kind's domains) and options."""
    page = vol.Schema(
        {
            vol.Required("kind"): vol.In(list(PAGE_KINDS)),
            vol.Optional("variant", default=0): vol.All(vol.Coerce(int), vol.Range(min=0, max=9)),
            vol.Optional("entities", default=list): [cv.entity_id],
            vol.Optional("options", default=dict): _page_options,
            # kept here with its settings, not sent to the panel
            vol.Optional("hidden", default=False): bool,
        }
    )(value)
    looks, domains, most = PAGE_KINDS[page["kind"]]
    if page["variant"] >= looks:
        raise vol.Invalid(f"{page['kind']} has {looks} looks")
    if len(page["entities"]) > most or any(e.split(".")[0] not in domains for e in page["entities"]):
        raise vol.Invalid(f"{page['kind']} takes up to {most} entities of {', '.join(domains) or 'no domain'}")
    if len(json.dumps(page["options"])) > _OPTIONS_JSON_MAX:
        raise vol.Invalid("the page's options are too large")
    if "items" in page["options"]:
        if page["kind"] not in ("home", "weather"):
            raise vol.Invalid("only custom and weather details pages have widgets")
        page["options"]["items"] = _home_items(
            page["options"]["items"], _WIDGET_KINDS if page["kind"] == "home" else _WEATHER_WIDGET_KINDS
        )
    return page


def checked_page(value: Any) -> dict[str, Any]:
    """A page as the options keep it (its kind, look, entities and options checked; the widgets on their grid)."""
    return _page(value)


# A quick controls page's options that go with its tiles, one value per tile in the tiles' order.
_TILE_LISTS = ("tapes", "group_starts", "group_names")


def split_quick_pages(pages: list[dict[str, Any]], max_pages: int = MAX_PAGES) -> list[dict[str, Any]] | None:
    """Quick controls pages with more tiles than a page takes, split into pages of their own right after them (each tile
    keeps its label and group); None when no page has too many. The list never grows past `max_pages`: tiles that find
    no room stay out."""
    most = PAGE_KINDS["quick"][2]
    if not any(p.get("kind") == "quick" and len(p.get("entities") or []) > most for p in pages):
        return None
    out: list[dict[str, Any]] = []
    for i, page in enumerate(pages):
        ents = list(page.get("entities") or [])
        if page.get("kind") != "quick" or len(ents) <= most:
            out.append(page)
            continue
        options = dict(page.get("options") or {})
        lists = {key: options[key] for key in _TILE_LISTS if isinstance(options.get(key), list)}
        for k in range(0, len(ents), most):
            # room for this part: the pages still to come keep theirs
            if k > 0 and len(out) + len(pages) - i > max_pages:
                break
            part = {**page, "entities": ents[k : k + most], "options": dict(options)}
            for key, values in lists.items():
                chunk = values[k : k + most]
                if any(chunk):
                    part["options"][key] = chunk
                else:
                    part["options"].pop(key, None)
            if k > 0:
                part["hidden"] = bool(page.get("hidden"))
            out.append(part)
    return out


def _pages(pages: list[dict[str, Any]]) -> list[dict[str, Any]]:
    if pages:
        pages[0]["hidden"] = False  # the start page is always shown
    return pages


_TIME = vol.All(str, vol.Match(r"^([01]\d|2[0-3]):[0-5]\d$"))

SCREENSAVER_SCHEMA = vol.Schema(
    {
        vol.Optional("when"): vol.In(SCREENSAVER_WHEN),
        vol.Optional("lux"): vol.All(vol.Coerce(float), vol.Range(min=0, max=1000)),
        vol.Optional("from"): _TIME,
        vol.Optional("to"): _TIME,
        vol.Optional("light_entity"): vol.Any("", cv.entity_domain("sensor")),
        # the clock on the screensaver: face (0..10), its parts, letters, the wandering speed, colour
        vol.Optional("face"): vol.All(vol.Coerce(int), vol.Range(min=0, max=10)),
        vol.Optional("seconds"): bool,
        vol.Optional("date"): bool,
        vol.Optional("weather"): bool,
        vol.Optional("next"): bool,
        vol.Optional("weight"): vol.All(vol.Coerce(int), vol.Range(min=300, max=800)),
        vol.Optional("width"): vol.All(vol.Coerce(int), vol.Range(min=70, max=130)),
        vol.Optional("speed"): vol.All(vol.Coerce(int), vol.Range(min=5, max=60)),
        vol.Optional("colour"): vol.Any("", vol.Match(r"^#[0-9a-fA-F]{6}$")),
    }
)

DND_SCHEMA = vol.Schema(
    {
        vol.Optional("mode"): vol.In(DND_MODES),
        vol.Optional("from"): _TIME,
        vol.Optional("to"): _TIME,
        vol.Optional("allow"): {vol.Optional(key): bool for key in DND_ALLOW},
    }
)

THEME_AUTO_SCHEMA = vol.Schema({vol.Optional("by"): vol.In(THEME_AUTO_BY), vol.Optional("from"): _TIME, vol.Optional("to"): _TIME})

POPUPS_SCHEMA = vol.Schema(
    {
        vol.Optional("kind"): vol.In(POPUP_KINDS),
        vol.Optional("sound"): bool,
        vol.Optional("seconds"): vol.All(vol.Coerce(int), vol.Range(min=0, max=600)),
    }
)


OPTIONS_SCHEMA = vol.Schema(
    {
        vol.Optional(CONF_DASHBOARD): vol.All(str, vol.Match(_DASHBOARD_PATH)),
        vol.Optional(CONF_SHOW_HEADER): bool,
        vol.Optional(CONF_SHOW_SIDEBAR): bool,
        vol.Optional(CONF_WEATHER): vol.Any("", cv.entity_domain("weather")),
        vol.Optional(CONF_MEDIA_PLAYER): vol.Any("", cv.entity_domain("media_player")),
        vol.Optional(CONF_TILES): vol.All([cv.entity_domain(list(TILE_DOMAINS))], vol.Length(max=24)),
        vol.Optional(CONF_SHOW_CONVERSATION): bool,
        vol.Optional(CONF_VOICE_LOOK): vol.All(vol.Coerce(int), vol.Range(min=0, max=3)),
        vol.Optional(CONF_VOICE_PLAYER): vol.In(VOICE_PLAYERS),
        vol.Optional(CONF_SKIP_UNUSED): bool,
        vol.Optional(CONF_KEEP_RESOURCES): vol.All([vol.All(str, vol.Length(max=300))], vol.Length(max=100)),
        vol.Optional(CONF_ZOOM): vol.All(vol.Coerce(int), vol.Range(min=50, max=150)),
        vol.Optional(CONF_LOADING_SCREEN): bool,
        vol.Optional(CONF_PAGES): vol.All([_page], vol.Length(min=1, max=MAX_PAGES), _pages),
        vol.Optional(CONF_HOME_EDIT): bool,
        vol.Optional(CONF_SCREENSAVER): SCREENSAVER_SCHEMA,
        vol.Optional(CONF_DND): DND_SCHEMA,
        vol.Optional(CONF_POPUPS): POPUPS_SCHEMA,
        vol.Optional(CONF_THEME_AUTO): THEME_AUTO_SCHEMA,
        vol.Optional(CONF_WIFI_ROAMING): bool,
        vol.Optional(CONF_WAKE_DISTANCE): vol.In(WAKE_DISTANCES),
        **{
            vol.Optional(key): vol.All(vol.Coerce(float), vol.Range(min=low, max=high))
            for key, (_default, low, high, _step, _unit) in REPORTING.items()
        },
    }
)


def _script_tag() -> str:
    """The first 8 hex digits of the page script's MD5 (empty when it cannot be read)."""
    try:
        return hashlib.md5((FRONTEND_DIR / "panelos-page.js").read_bytes()).hexdigest()[:8]
    except OSError:
        return ""


async def async_setup_page(hass: HomeAssistant) -> None:
    """Serve the page's files and register its commands (once per run)."""
    # the integration's version and the page script's own fingerprint: a browser never keeps an
    # older script after an update, even within one version
    version = (await async_get_integration(hass, DOMAIN)).version or "0"
    tag = await hass.async_add_executor_job(_script_tag)
    hass.data.setdefault(DOMAIN, {})["version"] = f"{version}-{tag}"
    await hass.http.async_register_static_paths([StaticPathConfig(STATIC_URL, str(FRONTEND_DIR), cache_headers=False)])
    # The PanelOS mark as an icon ("panelos:mark") everywhere in the frontend.
    frontend.add_extra_js_url(hass, f"{STATIC_URL}/panelos-icons.js?v={version}")
    for command in (
        ws_page_subscribe,
        ws_page_options,
        ws_page_command,
        ws_page_screen,
        ws_page_resources,
        ws_page_house,
        ws_page_test,
        ws_page_touch_test,
        ws_page_proximity_calibration,
        ws_page_alarms,
        ws_page_ma,
        ws_page_ma_player,
        ws_page_forget_user,
        ws_screen,
    ):
        websocket_api.async_register_command(hass, command)


async def async_show_page(hass: HomeAssistant) -> None:
    """Add the sidebar entry (while at least one panel is set up)."""
    if PAGE_URL in hass.data.get("frontend_panels", {}):
        return
    version = hass.data[DOMAIN]["version"]
    await panel_custom.async_register_panel(
        hass,
        webcomponent_name=PAGE_ELEMENT,
        frontend_url_path=PAGE_URL,
        module_url=f"{STATIC_URL}/panelos-page.js?v={version}",
        sidebar_title="PanelOS",
        sidebar_icon="panelos:mark",
        require_admin=True,
        config={"static_url": STATIC_URL, "version": version},
    )


@callback
def async_hide_page(hass: HomeAssistant) -> None:
    frontend.async_remove_panel(hass, PAGE_URL, warn_if_unknown=False)


def _panel(hass: HomeAssistant, entry_id: str) -> Panel:
    entry = hass.config_entries.async_get_entry(entry_id)
    panel = getattr(entry, "runtime_data", None) if entry and entry.domain == DOMAIN else None
    if panel is None:
        raise HomeAssistantError(f"No loaded PanelOS entry {entry_id}")
    return panel


def _house(hass: HomeAssistant) -> House:
    return hass.data[DOMAIN]["house"]


def _page_data(hass: HomeAssistant) -> dict[str, Any]:
    panels = []
    for entry in hass.config_entries.async_entries(DOMAIN):
        panel: Panel | None = getattr(entry, "runtime_data", None)
        options = {**entry.data, **entry.options}
        panels.append(
            {
                "entry_id": entry.entry_id,
                "title": entry.title,
                "device": entry.data.get(CONF_DEVICE, {}),
                "device_id": panel.device_id if panel else None,
                # the name of the Home Assistant user the panel logs in with, None until it subscribed
                "panel_user": (panel_user(entry) or {}).get("name"),
                "loaded": panel is not None,
                "connected": bool(panel and panel.connected),
                "states": dict(panel.states) if panel else {},
                "select_options": dict(panel.options) if panel else {},
                # what the panel can do; an NSPanel Pro until it reports it
                "caps": panel_caps(entry),
                "caps_reported": isinstance(entry.data.get("caps"), list),
                "settings": {
                    **panel_config(entry),
                    "skip_unused": bool(entry.options.get(CONF_SKIP_UNUSED, True)),
                    "keep_resources": list(entry.options.get(CONF_KEEP_RESOURCES, [])),
                    # every page with its settings, the hidden ones too
                    "all_pages": [dict(page) for page in options.get(CONF_PAGES, DEFAULT_PAGES)],
                },
                # The dashboard the web view shows and its card resources.
                "web_dashboard": panel.dashboard if panel else "",
                # the panel's alarms (they live on the panel), None until it reports them
                "alarms": panel.alarms if panel else None,
                # alarms set here that the panel has not confirmed yet (they go when it is online)
                "alarms_pending": panel.pending_alarms if panel else None,
                "resources": list(panel.resources) if panel else [],
            }
        )
    house = _house(hass)
    return {
        "panels": panels,
        "dashboards": [dict(choice) for choice in dashboard_choices(hass)],
        "tile_domains": list(TILE_DOMAINS),
        "page_kinds": {kind: {"looks": looks, "domains": list(domains), "most": most} for kind, (looks, domains, most) in PAGE_KINDS.items()},
        "max_pages": MAX_PAGES,
        "reporting": {
            key: {"default": default, "min": low, "max": high, "step": step, "unit": unit}
            for key, (default, low, high, step, unit) in REPORTING.items()
        },
        "house": {**house.data, "active": house.active()},
        "status_cats": list(STATUS_CATS),
        "safety_classes": list(SAFETY_CLASSES),
    }


@websocket_api.require_admin
@websocket_api.websocket_command({vol.Required("type"): "panelos/page/subscribe"})
@callback
def ws_page_subscribe(hass: HomeAssistant, connection: websocket_api.ActiveConnection, msg: dict[str, Any]) -> None:
    """The page's data now and after every change (at most every 0.4 s)."""
    pending: CALLBACK_TYPE | None = None

    @callback
    def send(_now: Any = None) -> None:
        nonlocal pending
        pending = None
        connection.send_message(websocket_api.event_message(msg["id"], _page_data(hass)))

    @callback
    def changed(*_args: Any) -> None:
        nonlocal pending
        if pending is None:
            pending = async_call_later(hass, 0.4, send)

    unsubs = [async_dispatcher_connect(hass, signal, changed) for signal in (SIGNAL_LINKS, SIGNAL_CONFIG_UPDATED, SIGNAL_PAGE)]

    @callback
    def unsubscribe() -> None:
        for unsub in unsubs:
            unsub()
        if pending:
            pending()

    connection.subscriptions[msg["id"]] = unsubscribe
    connection.send_result(msg["id"])
    send()


@websocket_api.require_admin
@websocket_api.websocket_command(
    {vol.Required("type"): "panelos/page/options", vol.Required("entry_id"): str, vol.Required("options"): dict}
)
@callback
def ws_page_options(hass: HomeAssistant, connection: websocket_api.ActiveConnection, msg: dict[str, Any]) -> None:
    """Change some of a panel's integration options (the others stay)."""
    entry = hass.config_entries.async_get_entry(msg["entry_id"])
    if entry is None or entry.domain != DOMAIN:
        connection.send_error(msg["id"], "not_found", "No such PanelOS entry")
        return
    try:
        changes = OPTIONS_SCHEMA(msg["options"])
    except vol.Invalid as err:
        connection.send_error(msg["id"], "invalid_format", str(err))
        return
    options = {**entry.options, **changes}
    # The grouped settings change field by field.
    for key in (CONF_SCREENSAVER, CONF_DND, CONF_POPUPS, CONF_THEME_AUTO):
        if key in changes and isinstance(entry.options.get(key), dict):
            merged = {**entry.options[key], **changes[key]}
            if key == CONF_DND and "allow" in changes[key] and isinstance(entry.options[key].get("allow"), dict):
                merged["allow"] = {**entry.options[key]["allow"], **changes[key]["allow"]}
            options[key] = merged
    for key in _EMPTY_CLEARS:
        if options.get(key) == "":
            options.pop(key)
    hass.config_entries.async_update_entry(entry, options=options)
    connection.send_result(msg["id"])


@websocket_api.require_admin
@websocket_api.websocket_command(
    {
        vol.Required("type"): "panelos/page/command",
        vol.Required("entry_id"): str,
        # the device's settings, and "page": show a page now (while its settings are open on the page)
        vol.Required("key"): vol.In([*KEY_DOMAIN, "page"]),
        vol.Optional("value"): vol.Any(None, bool, int, float, vol.All(str, vol.Length(max=255))),
    }
)
@callback
def ws_page_command(hass: HomeAssistant, connection: websocket_api.ActiveConnection, msg: dict[str, Any]) -> None:
    """Set a setting of the panel's device (as its entity would)."""
    try:
        _panel(hass, msg["entry_id"]).send_command(msg["key"], msg.get("value"))
    except HomeAssistantError as err:
        connection.send_error(msg["id"], "not_connected", str(err))
        return
    connection.send_result(msg["id"])


@websocket_api.require_admin
@websocket_api.websocket_command({vol.Required("type"): "panelos/page/screen", vol.Required("entry_id"): str})
@websocket_api.async_response
async def ws_page_screen(hass: HomeAssistant, connection: websocket_api.ActiveConnection, msg: dict[str, Any]) -> None:
    """A fresh picture of the panel's screen (base64; type image/jpeg from new panels, image/png from older ones)."""
    try:
        data, mime = await _panel(hass, msg["entry_id"]).async_screenshot()
    except HomeAssistantError as err:
        connection.send_error(msg["id"], "no_picture", str(err))
        return
    connection.send_result(msg["id"], {"png": base64.b64encode(data).decode(), "type": mime})


@websocket_api.require_admin
@websocket_api.websocket_command({vol.Required("type"): "panelos/page/resources", vol.Required("entry_id"): str})
@websocket_api.async_response
async def ws_page_resources(hass: HomeAssistant, connection: websocket_api.ActiveConnection, msg: dict[str, Any]) -> None:
    """The card resources, their size and whether the panel's dashboard needs them."""
    entry = hass.config_entries.async_get_entry(msg["entry_id"])
    if entry is None or entry.domain != DOMAIN:
        connection.send_error(msg["id"], "not_found", "No such PanelOS entry")
        return
    panel: Panel | None = getattr(entry, "runtime_data", None)
    dashboard = panel.dashboard if panel else panel_config(entry)["dashboard"]
    connection.send_result(
        msg["id"], {"dashboard": dashboard, "resources": await async_resource_list(hass, dashboard)}
    )


@websocket_api.require_admin
@websocket_api.websocket_command({vol.Required("type"): "panelos/page/house", vol.Required("house"): dict})
@websocket_api.async_response
async def ws_page_house(hass: HomeAssistant, connection: websocket_api.ActiveConnection, msg: dict[str, Any]) -> None:
    """Store the critical alerts and status screens shared by every panel."""
    try:
        house = HOUSE_SCHEMA(msg["house"])
    except vol.Invalid as err:
        connection.send_error(msg["id"], "invalid_format", str(err))
        return
    await _house(hass).async_save(house)
    connection.send_result(msg["id"])


@websocket_api.require_admin
@websocket_api.websocket_command(
    {
        vol.Required("type"): "panelos/page/test",
        vol.Required("entry_id"): str,
        vol.Required("kind"): vol.In(("critical", "status")),
        vol.Required("item"): vol.All(str, vol.Length(max=255)),
        vol.Optional("seconds", default=10): vol.All(vol.Coerce(int), vol.Range(min=3, max=60)),
    }
)
@callback
def ws_page_test(hass: HomeAssistant, connection: websocket_api.ActiveConnection, msg: dict[str, Any]) -> None:
    """Show a critical alert or a status screen on one panel for a few seconds."""
    try:
        panel = _panel(hass, msg["entry_id"])
        if not panel.connected:
            raise HomeAssistantError(f"The PanelOS panel '{panel.panel_id}' is not connected")
        _house(hass).test(panel_config(panel.entry)["panel_id"], msg["kind"], msg["item"], msg["seconds"])
    except (HomeAssistantError, vol.Invalid) as err:
        connection.send_error(msg["id"], "not_shown", str(err))
        return
    connection.send_result(msg["id"])


_ALARM = vol.Schema(
    {
        vol.Required("h"): vol.All(vol.Coerce(int), vol.Range(min=0, max=23)),
        vol.Required("m"): vol.All(vol.Coerce(int), vol.Range(min=0, max=59)),
        vol.Optional("days", default=list): vol.All([bool], vol.Length(max=7)),
        vol.Optional("label", default=""): vol.All(str, vol.Length(max=32)),
        vol.Optional("on", default=True): bool,
        vol.Optional("tone", default="auto"): vol.Any(vol.All(str, vol.Match(r"^ma:\S{1,110}$")), _tone),
        vol.Optional("tone_name", default=""): vol.All(str, vol.Length(max=60)),
    }
)


@websocket_api.require_admin
@websocket_api.websocket_command(
    {vol.Required("type"): "panelos/page/alarms", vol.Required("entry_id"): str, vol.Required("alarms"): vol.All([dict], vol.Length(max=20))}
)
@callback
def ws_page_alarms(hass: HomeAssistant, connection: websocket_api.ActiveConnection, msg: dict[str, Any]) -> None:
    """Set a panel's alarms (the panel keeps them and rings them itself, then reports them back)."""
    try:
        alarms = [_ALARM(a) for a in msg["alarms"]]
        panel = _panel(hass, msg["entry_id"])
    except vol.Invalid as err:
        connection.send_error(msg["id"], "invalid_format", str(err))
        return
    except HomeAssistantError as err:
        connection.send_error(msg["id"], "not_found", str(err))
        return
    for a in alarms:
        a["days"] = (a["days"] + [False] * 7)[:7]
    # kept until the panel reports exactly these alarms (sent now when it is online, else when it comes back)
    sent = panel.set_pending_alarms(alarms)
    async_dispatcher_send(hass, SIGNAL_PAGE)
    connection.send_result(msg["id"], {"sent": sent})
    # an alarm with music: Music Assistant must know the panel's player; it is added there by itself
    if any(str(a.get("tone", "")).startswith("ma:") for a in alarms):
        hass.async_create_task(_async_ma_player(hass, panel), eager_start=False)


async def _async_ma_player(hass: HomeAssistant, panel: Panel) -> None:
    from .ma import async_add  # noqa: PLC0415 - only when an alarm has music

    entity = panel_config(panel.entry)["media_player"]
    try:
        status = await async_add(hass, entity)
    except Exception as err:  # noqa: BLE001 - Music Assistant's own errors (rights, provider missing)
        _LOGGER.warning("Could not add %s to Music Assistant: %s", entity, err)
        return
    if status.get("entity_ma"):
        # the panel learns which entity plays its music
        async_dispatcher_send(hass, SIGNAL_CONFIG_UPDATED, panel_config(panel.entry)["panel_id"])


@websocket_api.require_admin
@websocket_api.websocket_command({vol.Required("type"): "panelos/page/ma_player", vol.Required("entry_id"): str, vol.Optional("add", default=False): bool})
@websocket_api.async_response
async def ws_page_ma_player(hass: HomeAssistant, connection: websocket_api.ActiveConnection, msg: dict[str, Any]) -> None:
    """Whether Music Assistant knows the panel's player; with add, adds it there."""
    from .ma import NotAllowed, async_add, async_status  # noqa: PLC0415

    try:
        panel = _panel(hass, msg["entry_id"])
    except HomeAssistantError as err:
        connection.send_error(msg["id"], "not_found", str(err))
        return
    entity = panel_config(panel.entry)["media_player"]
    try:
        result = await (async_add(hass, entity) if msg["add"] else async_status(hass, entity))
    except NotAllowed:
        # the page then shows how to add it in Music Assistant itself
        connection.send_result(msg["id"], {**(await async_status(hass, entity)), "by_hand": True})
        return
    except Exception as err:  # noqa: BLE001 - shown on the page as it is
        connection.send_error(msg["id"], "music_assistant", str(err))
        return
    if msg["add"] and result.get("entity_ma"):
        async_dispatcher_send(hass, SIGNAL_CONFIG_UPDATED, panel_config(panel.entry)["panel_id"])
    connection.send_result(msg["id"], result)


_MA_KINDS = {"playlist": "Playlist", "radio": "Radio station", "album": "Album", "track": "Track", "artist": "Artist"}


@websocket_api.require_admin
@websocket_api.websocket_command({vol.Required("type"): "panelos/page/ma", vol.Optional("query", default=""): vol.All(str, vol.Length(max=100))})
@websocket_api.async_response
async def ws_page_ma(hass: HomeAssistant, connection: websocket_api.ActiveConnection, msg: dict[str, Any]) -> None:
    """Music from Music Assistant for an alarm: its playlists and radio stations, or what a search finds."""
    entries = [e for e in hass.config_entries.async_entries("music_assistant") if e.state is ConfigEntryState.LOADED]
    if not entries or not hass.services.has_service("music_assistant", "search"):
        connection.send_result(msg["id"], {"available": False, "items": []})
        return
    entry_id = entries[0].entry_id
    query = msg["query"].strip()
    found: list[dict[str, Any]] = []
    try:
        if query:
            resp = await hass.services.async_call(
                "music_assistant", "search",
                {"config_entry_id": entry_id, "name": query, "limit": 6, "media_type": ["playlist", "radio", "album", "track", "artist"]},
                blocking=True, return_response=True,
            )
            for group in ("playlists", "radio", "albums", "tracks", "artists"):
                found.extend((resp or {}).get(group, []))
        else:
            for kind in ("playlist", "radio"):
                resp = await hass.services.async_call(
                    "music_assistant", "get_library", {"config_entry_id": entry_id, "media_type": kind, "limit": 25},
                    blocking=True, return_response=True,
                )
                found.extend((resp or {}).get("items", []))
    except HomeAssistantError as err:
        connection.send_error(msg["id"], "music_assistant", str(err))
        return
    items = [
        {"uri": i["uri"], "name": str(i.get("name") or i["uri"])[:80], "kind": _MA_KINDS.get(i.get("media_type", ""), "Music")}
        for i in found
        if isinstance(i, dict) and isinstance(i.get("uri"), str) and len(i["uri"]) <= 110
    ]
    connection.send_result(msg["id"], {"available": True, "items": items[:60]})


@websocket_api.require_admin
@websocket_api.websocket_command({vol.Required("type"): "panelos/page/touch_test", vol.Required("entry_id"): str})
@callback
def ws_page_touch_test(hass: HomeAssistant, connection: websocket_api.ActiveConnection, msg: dict[str, Any]) -> None:
    """Start the panel's touch test (it corrects the touch only when the offsets are systematic)."""
    try:
        panel = _panel(hass, msg["entry_id"])
        if not panel.connected:
            raise HomeAssistantError(f"The PanelOS panel '{panel.panel_id}' is not connected")
    except HomeAssistantError as err:
        connection.send_error(msg["id"], "not_connected", str(err))
        return
    async_dispatcher_send(hass, SIGNAL_SHOW, panel_config(panel.entry)["panel_id"], {"type": "touch_test"})
    connection.send_result(msg["id"])


@websocket_api.require_admin
@websocket_api.websocket_command({vol.Required("type"): "panelos/page/proximity_calibration", vol.Required("entry_id"): str})
@callback
def ws_page_proximity_calibration(hass: HomeAssistant, connection: websocket_api.ActiveConnection, msg: dict[str, Any]) -> None:
    """Start the panel's wake distance calibration (it measures the sensor's resting level with nothing in front)."""
    try:
        panel = _panel(hass, msg["entry_id"])
        if not panel.connected:
            raise HomeAssistantError(f"The PanelOS panel '{panel.panel_id}' is not connected")
    except HomeAssistantError as err:
        connection.send_error(msg["id"], "not_connected", str(err))
        return
    async_dispatcher_send(hass, SIGNAL_SHOW, panel_config(panel.entry)["panel_id"], {"type": "proximity_calibration"})
    connection.send_result(msg["id"])


@websocket_api.require_admin
@websocket_api.websocket_command({vol.Required("type"): "panelos/page/forget_user", vol.Required("entry_id"): str})
@callback
def ws_page_forget_user(hass: HomeAssistant, connection: websocket_api.ActiveConnection, msg: dict[str, Any]) -> None:
    """Forget the panel's Home Assistant user: its links end, and the next user that subscribes for the panel is kept."""
    entry = hass.config_entries.async_get_entry(msg["entry_id"])
    if entry is None or entry.domain != DOMAIN:
        connection.send_error(msg["id"], "not_found", "No such PanelOS entry")
        return
    set_panel_user(hass, entry, None)
    connection.send_result(msg["id"])


@websocket_api.websocket_command(
    {
        vol.Required("type"): "panelos/screen",
        vol.Optional("panel_id", default=""): str,
        # a JPEG from new panels (small: the live preview), a PNG from older ones
        vol.Exclusive("png", "picture"): str,
        vol.Exclusive("jpeg", "picture"): str,
    }
)
@callback
def ws_screen(hass: HomeAssistant, connection: websocket_api.ActiveConnection, msg: dict[str, Any]) -> None:
    """The panel's answer to a screenshot command."""
    entry = find_entry(hass, msg["panel_id"])
    panel: Panel | None = getattr(entry, "runtime_data", None) if entry else None
    if panel is None or not panel.linked_on(id(connection)):
        connection.send_error(msg["id"], "not_subscribed", "Subscribe with panelos/subscribe first")
        return
    jpeg = "jpeg" in msg
    try:
        data = base64.b64decode(msg["jpeg"] if jpeg else msg.get("png", ""), validate=True)
    except (binascii.Error, ValueError):
        connection.send_error(msg["id"], "invalid_format", "Not base64")
        return
    ok = data.startswith(b"\xff\xd8\xff") if jpeg else data.startswith(b"\x89PNG\r\n\x1a\n")
    if len(data) > _PNG_MAX or not ok:
        connection.send_error(msg["id"], "invalid_format", "Not a JPEG" if jpeg else "Not a PNG")
        return
    panel.screen_received(data, "image/jpeg" if jpeg else "image/png")
    connection.send_result(msg["id"])
