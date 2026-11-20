"""PanelOS: panels running PanelOS as devices of their own, their settings, and screen control.

A panel talks to Home Assistant on its own: over the WebSocket API with its
own login, and over the ESPHome native API for the voice satellite. This
integration gives each panel:

* its device with the display, its settings and its sensors (screen,
  brightness, theme, screensaver, dashboard, display mode, illuminance,
  presence, …). The panel reports their states with the websocket command
  ``panelos/report`` and gets their commands back on ``panelos/subscribe``.
  The ESPHome device keeps the voice satellite (with its volume, microphone
  and wake word sensors) and is shown as connected via this device;
* the dashboard mode's settings (which Home Assistant dashboard the panel's
  web view shows, with or without its header and sidebar) and the assistant
  mode's settings (weather, media player, quick tiles, full-screen
  conversation), set in the integration's options;
* how much the recorded sensors (illuminance, SoC temperature, Wi-Fi signal)
  must change before the panel sends a new value, also in the options;
* actions that put something on a panel's screen: ``panelos.show_view``
  (home, dashboard, weather, media, a camera) and ``panelos.notify``;
* critical alerts and status screens shared by every panel (``house.py``),
  with ``panelos.show_status`` / ``panelos.clear_status`` for the status
  screens an automation starts;
* the PanelOS page in the sidebar (administrators): every setting of a panel
  beside a live picture of its screen (``page.py``).

Panels follow their settings, show requests and commands with
``panelos/subscribe``, so the panel's Home Assistant user does not need to be
an administrator.
"""

from __future__ import annotations

import time
from datetime import timedelta
from typing import Any

import voluptuous as vol

from homeassistant.components import websocket_api
from homeassistant.components.intent.const import TIMER_DATA
from homeassistant.config_entries import ConfigEntry
from homeassistant.core import HomeAssistant, ServiceCall, callback
from homeassistant.exceptions import HomeAssistantError, ServiceValidationError
from homeassistant.helpers import config_validation as cv
from homeassistant.helpers.dispatcher import async_dispatcher_connect, async_dispatcher_send
from homeassistant.helpers.event import async_track_time_interval
from homeassistant.helpers.service import async_register_admin_service
from homeassistant.helpers.start import async_at_started

from .const import (
    CONF_PAGES,
    CONF_PANEL_ID,
    DEFAULT_PAGES,
    DOMAIN,
    PLATFORMS,
    SERVICE_CLEAR_CRITICAL_ALERT,
    SERVICE_CLEAR_STATUS,
    SERVICE_CRITICAL_ALERT,
    SERVICE_NOTIFY,
    SERVICE_SHOW_STATUS,
    SERVICE_SHOW_VIEW,
    SIGNAL_CONFIG_UPDATED,
    SIGNAL_LINKS,
    SIGNAL_SHOW,
    VIEWS,
)
from .house import House
from .notes import Notes
from .players import async_register_players_view
from .sounds import async_register_views
from .page import async_hide_page, async_setup_page, async_show_page
from .panel import Panel, alive, find_entry, link_gone, links, panel_config, voice_player

type PanelOSConfigEntry = ConfigEntry[Panel]

CONFIG_SCHEMA = cv.config_entry_only_config_schema(DOMAIN)

_SECONDS = vol.All(vol.Coerce(int), vol.Range(min=0, max=3600))

SHOW_VIEW_SCHEMA = vol.Schema(
    {
        vol.Optional(CONF_PANEL_ID, default=""): cv.string,
        vol.Required("view"): vol.In(VIEWS),
        vol.Optional("entity_id"): cv.entity_id,
        vol.Optional("seconds", default=30): _SECONDS,
    }
)

NOTIFY_SCHEMA = vol.Schema(
    {
        vol.Optional(CONF_PANEL_ID, default=[]): vol.Any(cv.string, [cv.string]),
        vol.Optional("title", default=""): vol.All(cv.string, vol.Length(max=100)),
        vol.Required("message"): vol.All(cv.string, vol.Length(max=1000)),
        vol.Optional("icon", default=""): vol.All(cv.string, vol.Length(max=64)),
        vol.Optional("level", default="info"): vol.In(("info", "warning")),
        vol.Optional("picture", default=""): vol.All(cv.string, vol.Length(max=500)),
        vol.Optional("buttons", default=[]): vol.All([vol.All(cv.string, vol.Length(max=24))], vol.Length(max=3)),
        vol.Optional("until_answered", default=False): cv.boolean,
    }
)


STATUS_ACTION_SCHEMA = vol.Schema({vol.Required("screen"): vol.All(cv.string, vol.Match(r"^[a-z0-9_]{1,40}$"))})
CRITICAL_ACTION_SCHEMA = vol.Schema({vol.Required("alert"): vol.All(cv.string, vol.Match(r"^[a-z0-9_]{1,40}$"))})


def _loaded_panel(hass: HomeAssistant, panel_id: str) -> Panel | None:
    entry = find_entry(hass, panel_id)
    return getattr(entry, "runtime_data", None) if entry else None


@websocket_api.websocket_command(
    {vol.Required("type"): "panelos/subscribe", vol.Optional("panel_id", default=""): str}
)
@callback
def ws_subscribe(hass: HomeAssistant, connection: websocket_api.ActiveConnection, msg: dict[str, Any]) -> None:
    """Send a panel its settings now and on every change, then its show requests and commands."""
    panel_id = msg["panel_id"]
    key = hash((id(connection), msg["id"]))

    @callback
    def send_config() -> None:
        entry = find_entry(hass, panel_id)
        config = panel_config(entry) if entry else {}
        connection.send_message(websocket_api.event_message(msg["id"], {"config": config}))

    @callback
    def config_changed(changed_panel: str) -> None:
        if not panel_id or changed_panel in (panel_id, ""):
            send_config()

    @callback
    def show(target: str, request: dict[str, Any]) -> None:
        # A target is a panel id; a panel that subscribed without one is the
        # only entry's panel.
        if not target or target == panel_id or (
            (entry := find_entry(hass, panel_id)) is not None and panel_config(entry)["panel_id"] == target
        ):
            connection.send_message(websocket_api.event_message(msg["id"], {"show": request}))

    @callback
    def command(cmd: dict[str, Any]) -> None:
        connection.send_message(websocket_api.event_message(msg["id"], {"command": cmd}))

    unsub_config = async_dispatcher_connect(hass, SIGNAL_CONFIG_UPDATED, config_changed)
    unsub_show = async_dispatcher_connect(hass, SIGNAL_SHOW, show)
    links(hass)[key] = (panel_id, id(connection), command)

    @callback
    def unsubscribe() -> None:
        unsub_config()
        unsub_show()
        links(hass).pop(key, None)
        alive(hass).pop(id(connection), None)
        async_dispatcher_send(hass, SIGNAL_LINKS)

    connection.subscriptions[msg["id"]] = unsubscribe
    connection.send_result(msg["id"])
    send_config()
    # the timers running on the panel now (Home Assistant's, so the panel shows them after a restart too)
    if (entry := find_entry(hass, panel_id)) is not None and (panel := getattr(entry, "runtime_data", None)) is not None:
        connection.send_message(websocket_api.event_message(msg["id"], {"timers": panel.timers_now()}))
    # The critical alerts and status screens active right now.
    if (entry := find_entry(hass, panel_id)) is not None:
        for request in hass.data[DOMAIN]["house"].requests_for(panel_config(entry)["panel_id"]):
            connection.send_message(websocket_api.event_message(msg["id"], {"show": request}))
        # PanelOS's own notifications, kept until answered (the unread ones pop up again)
        if "notes" in hass.data[DOMAIN]:
            request = hass.data[DOMAIN]["notes"].request_for(panel_config(entry)["panel_id"])
            connection.send_message(websocket_api.event_message(msg["id"], {"show": request}))
    async_dispatcher_send(hass, SIGNAL_LINKS)


@websocket_api.websocket_command(
    {
        vol.Required("type"): "panelos/notification",
        vol.Optional("panel_id", default=""): str,
        vol.Required("notification_id"): str,
        vol.Required("answer"): vol.In(("ok", "read", "dismiss", "button")),
        vol.Optional("button"): int,
    }
)
@callback
def ws_notification(hass: HomeAssistant, connection: websocket_api.ActiveConnection, msg: dict[str, Any]) -> None:
    """A panel answered one of PanelOS's notifications (OK, a button, Dismiss)."""
    entry = find_entry(hass, msg["panel_id"])
    panel_id = panel_config(entry)["panel_id"] if entry else msg["panel_id"]
    hass.data[DOMAIN]["notes"].answer(panel_id, msg["notification_id"], msg["answer"], msg.get("button"))
    connection.send_result(msg["id"])


@websocket_api.websocket_command({vol.Required("type"): "panelos/alive", vol.Optional("panel_id", default=""): str})
@callback
def ws_alive(hass: HomeAssistant, connection: websocket_api.ActiveConnection, msg: dict[str, Any]) -> None:
    """The panel is there (every second); the answer tells the panel that the integration is too."""
    gone = link_gone(hass, id(connection))
    alive(hass)[id(connection)] = time.monotonic()
    if gone:
        async_dispatcher_send(hass, SIGNAL_LINKS)
    connection.send_result(msg["id"])


# A development panel's system log (it sends its new lines every second): kept in
# /config/panelos/logs/<panel id>.log, turned over at 5 MB, three old files kept.
LOG_MAX_BYTES = 5 * 1024 * 1024
LOG_KEEP = 3


def _append_log(path: str, lines: list[str]) -> None:
    import os  # noqa: PLC0415

    os.makedirs(os.path.dirname(path), exist_ok=True)
    if os.path.exists(path) and os.path.getsize(path) > LOG_MAX_BYTES:
        for i in range(LOG_KEEP - 1, 0, -1):
            if os.path.exists(f"{path}.{i}"):
                os.replace(f"{path}.{i}", f"{path}.{i + 1}")
        os.replace(path, f"{path}.1")
    with open(path, "a", encoding="utf-8") as f:
        f.write("\n".join(lines) + "\n")


@websocket_api.websocket_command(
    {
        vol.Required("type"): "panelos/log",
        vol.Optional("panel_id", default=""): str,
        vol.Required("lines"): vol.All([vol.All(str, vol.Length(max=2000))], vol.Length(max=1000)),
    }
)
@websocket_api.async_response
async def ws_log(hass: HomeAssistant, connection: websocket_api.ActiveConnection, msg: dict[str, Any]) -> None:
    """Lines of a development panel's system log, kept in a file in the config folder."""
    name = "".join(ch if ch.isalnum() or ch in "-_" else "_" for ch in (msg["panel_id"] or "panel"))[:40]
    await hass.async_add_executor_job(_append_log, hass.config.path("panelos", "logs", f"{name}.log"), msg["lines"])
    connection.send_result(msg["id"])


@websocket_api.websocket_command(
    {
        vol.Required("type"): "panelos/report",
        vol.Optional("panel_id", default=""): str,
        vol.Optional("device"): dict,
        vol.Optional("options"): dict,
        vol.Optional("states"): dict,
        vol.Optional("dashboard"): str,
        vol.Optional("alarms"): list,
        # the player picked on the panel's music page (music commands said to the panel go there)
        vol.Optional("music_player"): str,
    }
)
@callback
def ws_report(hass: HomeAssistant, connection: websocket_api.ActiveConnection, msg: dict[str, Any]) -> None:
    """The panel's device info, select options and entity states (changes only)."""
    panel = _loaded_panel(hass, msg["panel_id"])
    if panel is None:
        connection.send_error(msg["id"], "not_found", f"No PanelOS entry for the panel '{msg['panel_id']}'")
        return
    # Only the connection that follows the panel's subscription reports for it.
    if not panel.linked_on(id(connection)):
        connection.send_error(msg["id"], "not_subscribed", "Subscribe with panelos/subscribe first")
        return
    panel.apply_report(msg)
    connection.send_result(msg["id"])


@websocket_api.websocket_command(
    {
        vol.Required("type"): "panelos/page_set",
        vol.Optional("panel_id", default=""): str,
        vol.Required("index"): vol.All(int, vol.Range(min=0, max=63)),
        vol.Optional("variant"): vol.All(int, vol.Range(min=0, max=15)),
        # values: plain ones, or a short list of ids (the calendars hidden on the panel)
        vol.Optional("options"): {
            str: vol.Any(None, bool, int, float, vol.All(str, vol.Length(max=200)), vol.All([vol.All(str, vol.Length(max=255))], vol.Length(max=16)))
        },
    }
)
@callback
def ws_page_set(hass: HomeAssistant, connection: websocket_api.ActiveConnection, msg: dict[str, Any]) -> None:
    """A page's look or some of its options set on the panel itself (its page sheet); `index` counts the pages it shows."""
    panel = _loaded_panel(hass, msg["panel_id"])
    if panel is None:
        connection.send_error(msg["id"], "not_found", f"No PanelOS entry for the panel '{msg['panel_id']}'")
        return
    if not panel.linked_on(id(connection)):
        connection.send_error(msg["id"], "not_subscribed", "Subscribe with panelos/subscribe first")
        return
    entry = panel.entry
    pages = [dict(p) for p in {**entry.data, **entry.options}.get(CONF_PAGES, DEFAULT_PAGES)]
    shown = [i for i, p in enumerate(pages) if i == 0 or not p.get("hidden")]
    if msg["index"] >= len(shown):
        connection.send_error(msg["id"], "not_found", "No such page")
        return
    page = pages[shown[msg["index"]]]
    if "variant" in msg:
        page["variant"] = msg["variant"]
    if "options" in msg:
        if any(not key.replace("_", "").isalpha() or len(key) > 32 for key in msg["options"]):
            connection.send_error(msg["id"], "invalid_format", "Option names are lower-case words")
            return
        options = dict(page.get("options", {}))
        for key, value in msg["options"].items():
            if value is None:
                options.pop(key, None)
            else:
                options[key] = value
        page["options"] = options
    hass.config_entries.async_update_entry(entry, options={**entry.options, CONF_PAGES: pages})
    connection.send_result(msg["id"])


@websocket_api.websocket_command(
    {
        vol.Required("type"): "panelos/timer",
        vol.Optional("panel_id", default=""): str,
        vol.Required("action"): vol.In(["start", "pause", "resume", "add", "remove", "cancel"]),
        vol.Optional("timer_id"): vol.All(str, vol.Length(max=64)),
        vol.Optional("seconds"): vol.All(int, vol.Range(min=1, max=86400)),
        vol.Optional("name"): vol.All(str, vol.Length(max=60)),
    }
)
@callback
def ws_timer(hass: HomeAssistant, connection: websocket_api.ActiveConnection, msg: dict[str, Any]) -> None:
    """A timer started or changed on the panel: Home Assistant's own timer of the panel's voice device."""
    panel = _loaded_panel(hass, msg["panel_id"])
    if panel is None:
        connection.send_error(msg["id"], "not_found", f"No PanelOS entry for the panel '{msg['panel_id']}'")
        return
    if not panel.linked_on(id(connection)):
        connection.send_error(msg["id"], "not_subscribed", "Subscribe with panelos/subscribe first")
        return
    manager = hass.data.get(TIMER_DATA)
    device_id = panel.timer_device_id()
    if manager is None or device_id is None:
        connection.send_error(msg["id"], "not_supported", "The panel's voice device has no timers in Home Assistant")
        return
    action = msg["action"]
    if action == "start":
        if "seconds" not in msg:
            connection.send_error(msg["id"], "invalid_format", "seconds missing")
            return
        timer_id = manager.start_timer(device_id, None, None, msg["seconds"], hass.config.language, name=msg.get("name") or None)
        connection.send_result(msg["id"], {"timer_id": timer_id})
        return
    timer = manager.timers.get(msg.get("timer_id", ""))
    if timer is None or timer.device_id != device_id:
        connection.send_error(msg["id"], "not_found", "No such timer on this panel")
        return
    if action == "pause":
        manager.pause_timer(timer.id)
    elif action == "resume":
        manager.unpause_timer(timer.id)
    elif action == "add":
        manager.add_time(timer.id, msg.get("seconds", 60))
    elif action == "remove":
        manager.remove_time(timer.id, msg.get("seconds", 60))
    else:
        manager.cancel_timer(timer.id)
    connection.send_result(msg["id"])


async def _setup_voice_intents(hass: HomeAssistant) -> None:
    from .voice_intents import async_setup_voice_intents  # noqa: PLC0415

    async_setup_voice_intents(hass)


async def async_setup(hass: HomeAssistant, config: dict[str, Any]) -> bool:
    """Register the websocket commands, the actions and the sidebar page's files once."""
    websocket_api.async_register_command(hass, ws_subscribe)
    websocket_api.async_register_command(hass, ws_report)
    websocket_api.async_register_command(hass, ws_alive)
    websocket_api.async_register_command(hass, ws_log)
    websocket_api.async_register_command(hass, ws_page_set)
    websocket_api.async_register_command(hass, ws_timer)
    websocket_api.async_register_command(hass, ws_notification)

    # A panel that stopped saying it is there is offline at once (checked every second).
    gone_before: set[int] = set()

    @callback
    def check_alive(_now: Any) -> None:
        nonlocal gone_before
        gone = {cid for _, cid, _ in links(hass).values() if link_gone(hass, cid)}
        if gone != gone_before:
            gone_before = gone
            async_dispatcher_send(hass, SIGNAL_LINKS)

    async_track_time_interval(hass, check_alive, timedelta(seconds=1), cancel_on_shutdown=True)
    house = House(hass)
    hass.data.setdefault(DOMAIN, {})["house"] = house
    await house.async_load()
    notes = Notes(hass)
    hass.data[DOMAIN]["notes"] = notes
    # voice_device_context (a device with ``player: panelos``) asks here which player music commands said to a panel
    # act on, at the moment of the command
    hass.data[DOMAIN]["voice_player"] = lambda satellite_id: voice_player(hass, satellite_id)
    await notes.async_load()
    await async_setup_page(hass)
    # music by voice on a panel: Home Assistant's media intents act on the panel's chosen player
    async_at_started(hass, _setup_voice_intents)

    async def handle_show(call: ServiceCall) -> None:
        request = dict(call.data)
        target = request.pop(CONF_PANEL_ID, "")
        panels = {pid for pid, _, _ in links(hass).values()}
        if not panels or (target and target not in panels):
            raise HomeAssistantError(
                f"No PanelOS panel is connected{f' as {target}' if target else ''} (connected: {sorted(panels) or 'none'})"
            )
        request["type"] = "view"
        async_dispatcher_send(hass, SIGNAL_SHOW, target, request)

    async def handle_notify(call: ServiceCall) -> None:
        data = dict(call.data)
        wanted = data.pop(CONF_PANEL_ID, [])
        wanted = [wanted] if isinstance(wanted, str) and wanted else list(wanted or [])
        known = [panel_config(e)["panel_id"] for e in hass.config_entries.async_entries(DOMAIN)]
        if not known:
            raise HomeAssistantError("No PanelOS panel is set up")
        unknown = [w for w in wanted if w not in known]
        if unknown:
            raise ServiceValidationError(f"No PanelOS panel {', '.join(unknown)} (panels: {', '.join(sorted(known))})")
        notes.add(wanted or known, data)

    # Admin users and automations only: the actions put a camera or a message
    # on the panel.
    async_register_admin_service(hass, DOMAIN, SERVICE_SHOW_VIEW, handle_show, schema=SHOW_VIEW_SCHEMA)
    async_register_admin_service(hass, DOMAIN, SERVICE_NOTIFY, handle_notify, schema=NOTIFY_SCHEMA)

    async def handle_status(call: ServiceCall) -> None:
        try:
            if call.service == SERVICE_SHOW_STATUS:
                house.show_status(call.data["screen"])
            else:
                house.clear_status(call.data["screen"])
        except vol.Invalid as err:
            raise ServiceValidationError(str(err)) from err

    # Status screens whose trigger is an action (set on the PanelOS page).
    async_register_admin_service(hass, DOMAIN, SERVICE_SHOW_STATUS, handle_status, schema=STATUS_ACTION_SCHEMA)
    async_register_admin_service(hass, DOMAIN, SERVICE_CLEAR_STATUS, handle_status, schema=STATUS_ACTION_SCHEMA)

    async def handle_critical(call: ServiceCall) -> None:
        try:
            if call.service == SERVICE_CRITICAL_ALERT:
                house.show_critical(call.data["alert"])
            else:
                house.clear_critical(call.data["alert"])
        except vol.Invalid as err:
            raise ServiceValidationError(str(err)) from err

    # Own critical alerts started by an automation (set on the PanelOS page).
    async_register_admin_service(hass, DOMAIN, SERVICE_CRITICAL_ALERT, handle_critical, schema=CRITICAL_ACTION_SCHEMA)
    async_register_admin_service(hass, DOMAIN, SERVICE_CLEAR_CRITICAL_ALERT, handle_critical, schema=CRITICAL_ACTION_SCHEMA)
    # Sounds uploaded on the page, fetched by the panels.
    async_register_views(hass)
    async_register_players_view(hass)
    return True


async def async_setup_entry(hass: HomeAssistant, entry: PanelOSConfigEntry) -> bool:
    """Set up one panel: its device and entities, and its settings."""
    panel = Panel(hass, entry)
    await panel.async_load_pending()
    entry.runtime_data = panel
    panel.async_setup()
    await hass.config_entries.async_forward_entry_setups(entry, PLATFORMS)
    await async_show_page(hass)
    entry.async_on_unload(entry.add_update_listener(_options_updated))
    async_dispatcher_send(hass, SIGNAL_CONFIG_UPDATED, panel_config(entry)["panel_id"])
    # Shared alerts and status screens for this panel too.
    hass.data[DOMAIN]["house"].evaluate()
    return True


async def _options_updated(hass: HomeAssistant, entry: ConfigEntry) -> None:
    async_dispatcher_send(hass, SIGNAL_CONFIG_UPDATED, panel_config(entry)["panel_id"])
    # Another dashboard, or other resource settings: work the skip list out again.
    if panel := getattr(entry, "runtime_data", None):
        panel.refresh_skip()


async def async_unload_entry(hass: HomeAssistant, entry: PanelOSConfigEntry) -> bool:
    """Unload a panel's entry (the panel falls back to its defaults)."""
    if not await hass.config_entries.async_unload_platforms(entry, PLATFORMS):
        return False
    if not any(
        other.entry_id != entry.entry_id and getattr(other, "runtime_data", None)
        for other in hass.config_entries.async_entries(DOMAIN)
    ):
        async_hide_page(hass)
    panel_id = panel_config(entry)["panel_id"]
    hass.async_create_task(_after_unload(hass, panel_id))
    return True


async def _after_unload(hass: HomeAssistant, panel_id: str) -> None:
    # The entry is gone from the list only after unload returns.
    async_dispatcher_send(hass, SIGNAL_CONFIG_UPDATED, panel_id)
    hass.data[DOMAIN]["house"].evaluate()
