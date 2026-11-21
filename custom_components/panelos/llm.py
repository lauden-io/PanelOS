"""Timer and alarm tools for voice assistants: a timer or an alarm on a PanelOS panel, asked from any device.

Home Assistant's own timer tools always use the device the user talks to. These tools put a timer on a panel
named by its room or name ("set a 10 minute timer on the kitchen panel"): it becomes Home Assistant's timer
of that panel's voice device, so the panel shows and rings it and handles it as its own, and voice on any device
can list or cancel it. The alarm tools set, list and cancel the alarms a panel keeps and rings itself (the same
alarms as on its timers page and the PanelOS page). Agents that use the Assist API get the tools without any setup.
"""

from __future__ import annotations

import inspect
from typing import Any

from homeassistant.components.intent.const import TIMER_DATA
from homeassistant.components.llm import LLMTools
from homeassistant.core import HomeAssistant, callback
from homeassistant.helpers import area_registry as ar, config_validation as cv, device_registry as dr
from homeassistant.helpers.dispatcher import async_dispatcher_send
from homeassistant.helpers.llm import LLM_API_ASSIST, LLMContext, Tool, ToolAnnotations, ToolInput, ToolResult

try:  # Home Assistant's schema library since 2026.x
    import probatio as schema
except ImportError:  # pragma: no cover
    import voluptuous as schema

from .const import DOMAIN, SIGNAL_PAGE

PANEL_HELP = "The panel's room: one of the PanelOS panels named in the prompt (in that wording)."
ALARMS_MAX = 20
WEEKDAYS = ("monday", "tuesday", "wednesday", "thursday", "friday", "saturday", "sunday")


def _norm(text: str) -> str:
    return " ".join(text.casefold().replace("_", " ").split())


def _panels(hass: HomeAssistant, timers: bool = True) -> list[tuple[Any, list[str]]]:
    """The loaded panels (for timers: those with a voice device that has timers), and the names they answer to."""
    found = []
    areas = ar.async_get(hass)
    devices = dr.async_get(hass)
    for entry in hass.config_entries.async_entries(DOMAIN):
        panel = getattr(entry, "runtime_data", None)
        if panel is None or (timers and panel.timer_device_id() is None):
            continue
        names = {entry.title, panel.panel_id}
        for device in dr.async_entries_for_config_entry(devices, entry.entry_id):
            names.update(n for n in (device.name_by_user, device.name) if n)
            if device.area_id and (area := areas.async_get_area(device.area_id)):
                names.add(area.name)
                names.update(area.aliases)
        found.append((panel, sorted(n for n in names if n)))
    return found


def _room(hass: HomeAssistant, panel: Any) -> str:
    areas = ar.async_get(hass)
    for device in dr.async_entries_for_config_entry(dr.async_get(hass), panel.entry.entry_id):
        if device.area_id and (area := areas.async_get_area(device.area_id)):
            return area.name
    return panel.entry.title


def _words(text: str) -> list[str]:
    return [w for w in _norm(text).replace("panel", " ").split() if w]


def _named(name: str, said: list[str]) -> bool:
    # every word of the name starts a word the user said ("kitchen" in "kitchen's", inflected words too)
    words = _words(name)
    return bool(words) and all(any(s.startswith(w) for s in said) for w in words)


def _find(hass: HomeAssistant, said: str, timers: bool = True) -> tuple[Any | None, list[str]]:
    """The panel the user meant (by room or name), or None and the panels there are."""
    panels = _panels(hass, timers)
    known = [_room(hass, p) for p, _ in panels]
    want = _norm(said or "")
    for panel, names in panels:
        if any(_norm(n) == want for n in names):
            return panel, known
    words = _words(want)
    hits = [panel for panel, names in panels if any(_named(n, words) for n in names)]
    if len(hits) == 1:
        return hits[0], known
    if not words and len(panels) == 1:
        # one panel in the home and no room said: "the panel" can only be that one
        return panels[0][0], known
    return None, known


def _clock(seconds: int) -> str:
    h, rest = divmod(max(0, int(seconds)), 3600)
    m, s = divmod(rest, 60)
    return f"{h}:{m:02d}:{s:02d}" if h else f"{m}:{s:02d}"


def _timers(hass: HomeAssistant, panel: Any) -> list[Any]:
    manager = hass.data.get(TIMER_DATA)
    device_id = panel.timer_device_id()
    if manager is None or device_id is None:
        return []
    return [t for t in manager.timers.values() if t.device_id == device_id and not t.conversation_command]


def _no_panel(said: str, known: list[str]) -> ToolResult:
    return ToolResult(
        data={"success": False, "error": f"No panel matches '{said}'", "panels": known},
        error=True,
    )


class StartPanelTimer(Tool):
    """A timer on a panel named by its room or name."""

    name = f"{DOMAIN}__StartPanelTimer"
    title = "Start a timer on a wall panel"
    description = (
        "Starts a timer on a PanelOS wall panel named by its room or name, also when the user talks to another "
        "device (for example 'set a 10 minute timer on the kitchen panel'). The panel shows the timer and "
        "rings when it ends."
    )
    parameters = schema.Schema(
        {
            schema.Required("panel", description=PANEL_HELP): cv.string,
            schema.Optional("hours"): cv.positive_int,
            schema.Optional("minutes"): cv.positive_int,
            schema.Optional("seconds"): cv.positive_int,
            schema.Optional("name", description="The timer's name, if the user gave one."): cv.string,
        }
    )
    annotations = ToolAnnotations(destructive=False, open_world=False)
    integration = DOMAIN

    async def async_call(self, hass: HomeAssistant, tool_input: ToolInput, llm_context: LLMContext) -> ToolResult:
        args = tool_input.tool_args
        said = str(args.get("panel", ""))
        panel, known = _find(hass, said)
        if panel is None:
            return _no_panel(said, known)
        hours, minutes, seconds = (int(args[k]) if args.get(k) else None for k in ("hours", "minutes", "seconds"))
        total = 3600 * (hours or 0) + 60 * (minutes or 0) + (seconds or 0)
        if total <= 0 or total > 86400:
            return ToolResult(data={"success": False, "error": "The timer needs a length up to 24 hours"}, error=True)
        manager = hass.data[TIMER_DATA]
        name = str(args.get("name") or "").strip() or None
        extra: dict[str, Any] = {"name": name}
        if "context" in inspect.signature(manager.start_timer).parameters:  # Home Assistant 2026.11+
            extra["context"] = llm_context.context
        timer_id = manager.start_timer(
            panel.timer_device_id(), hours, minutes, seconds, llm_context.language or hass.config.language, **extra
        )
        return ToolResult(
            data={"success": True, "panel": _room(hass, panel), "length": _clock(total), "name": name or "", "timer_id": timer_id}
        )


class CancelPanelTimer(Tool):
    """Cancel a panel's timer (by name when it has several)."""

    name = f"{DOMAIN}__CancelPanelTimer"
    title = "Cancel a timer on a wall panel"
    description = (
        "Cancels a timer on a PanelOS wall panel named by its room or name. With several timers on the panel, "
        "pass the timer's name, or all=true to cancel every timer on it."
    )
    parameters = schema.Schema(
        {
            schema.Required("panel", description=PANEL_HELP): cv.string,
            schema.Optional("name", description="The timer's name."): cv.string,
            schema.Optional("all", description="Cancel every timer on the panel."): cv.boolean,
        }
    )
    annotations = ToolAnnotations(destructive=True, idempotent=True, open_world=False)
    integration = DOMAIN

    async def async_call(self, hass: HomeAssistant, tool_input: ToolInput, llm_context: LLMContext) -> ToolResult:
        args = tool_input.tool_args
        said = str(args.get("panel", ""))
        panel, known = _find(hass, said)
        if panel is None:
            return _no_panel(said, known)
        timers = _timers(hass, panel)
        if name := _norm(str(args.get("name") or "")):
            timers = [t for t in timers if _norm(t.name or "") == name]
        room = _room(hass, panel)
        if not timers:
            return ToolResult(data={"success": False, "panel": room, "error": "No such timer on the panel"}, error=True)
        if len(timers) > 1 and not args.get("all"):
            return ToolResult(
                data={
                    "success": False,
                    "panel": room,
                    "error": "Several timers run on the panel: ask which one, or cancel all",
                    "timers": [{"name": t.name or "", "left": _clock(t.seconds_left)} for t in timers],
                },
                error=True,
            )
        manager = hass.data[TIMER_DATA]
        for t in timers:
            manager.cancel_timer(t.id)
        return ToolResult(data={"success": True, "panel": room, "cancelled": len(timers)})


class ListPanelTimers(Tool):
    """The timers running on the panels."""

    name = f"{DOMAIN}__ListPanelTimers"
    title = "List the timers on the wall panels"
    description = (
        "Lists the timers on PanelOS wall panels (one panel by its room or name, or all of them) with the exact time "
        "left, also for the panel the user talks to. Use it for questions like 'what timers do I have' or 'how much "
        "time is left'."
    )
    parameters = schema.Schema({schema.Optional("panel", description=PANEL_HELP): cv.string})
    annotations = ToolAnnotations(read_only=True, open_world=False)
    integration = DOMAIN

    async def async_call(self, hass: HomeAssistant, tool_input: ToolInput, llm_context: LLMContext) -> ToolResult:
        said = str(tool_input.tool_args.get("panel") or "")
        if said:
            panel, known = _find(hass, said)
            if panel is None:
                return _no_panel(said, known)
            panels = [panel]
        else:
            panels = [p for p, _ in _panels(hass)]
        return ToolResult(
            data={
                "panels": [
                    {
                        "panel": _room(hass, p),
                        "timers": [
                            {"name": t.name or "", "left": _clock(t.seconds_left), "length": _clock(t.seconds), "paused": not t.is_active}
                            for t in _timers(hass, p)
                        ],
                    }
                    for p in panels
                ]
            }
        )


# -- alarms: the panel keeps and rings them itself; a change goes to it as its whole alarm list --------------------


def _alarms(panel: Any) -> list[dict[str, Any]] | None:
    """The panel's alarms as they will be (set but not confirmed yet first), None while it has never reported them."""
    if panel.pending_alarms is not None:
        return [dict(a) for a in panel.pending_alarms]
    if panel.alarms is not None:
        return [dict(a) for a in panel.alarms]
    return None


def _days(value: Any) -> list[bool] | None:
    """Repeat days from the agent: weekday names, "weekdays", "weekends", "every day"; empty: once. None: not understood."""
    words = value if isinstance(value, list) else [value] if isinstance(value, str) and value.strip() else []
    days = [False] * 7
    for word in words:
        w = _norm(str(word)).replace("-", " ")
        if w in ("every day", "daily", "everyday"):
            days = [True] * 7
        elif w in ("weekdays", "weekday", "workdays"):
            days[:5] = [True] * 5
        elif w in ("weekends", "weekend"):
            days[5:] = [True, True]
        else:
            hits = [i for i, d in enumerate(WEEKDAYS) if len(w) >= 2 and d.startswith(w)]
            if len(hits) != 1:
                return None
            days[hits[0]] = True
    return days


def _days_text(days: list[bool]) -> str:
    if all(days):
        return "every day"
    if days == [True] * 5 + [False] * 2:
        return "weekdays"
    if days == [False] * 5 + [True] * 2:
        return "weekends"
    if not any(days):
        return "once"
    return ", ".join(d.capitalize() for d, on in zip(WEEKDAYS, days) if on)


def _alarm_out(a: dict[str, Any]) -> dict[str, Any]:
    days = [bool(d) for d in (a.get("days") or [])[:7]] + [False] * 7
    return {"time": f"{int(a.get('h', 0)):02d}:{int(a.get('m', 0)):02d}", "days": _days_text(days[:7]),
            "label": a.get("label") or "", "on": bool(a.get("on", True))}


def _alarm_matches(a: dict[str, Any], hour: int | None, minute: int | None, label: str) -> bool:
    if hour is not None and int(a.get("h", -1)) != hour:
        return False
    if minute is not None and int(a.get("m", -1)) != minute:
        return False
    return not label or _norm(a.get("label") or "") == label


def _send_alarms(hass: HomeAssistant, panel: Any, alarms: list[dict[str, Any]], cancelled: list[dict[str, Any]] | None = None) -> bool:
    alarms.sort(key=lambda a: int(a.get("h", 0)) * 60 + int(a.get("m", 0)))
    sent = panel.set_pending_alarms(alarms, cancelled)
    async_dispatcher_send(hass, SIGNAL_PAGE)
    return sent


def _no_alarms(room: str) -> ToolResult:
    return ToolResult(
        data={"success": False, "panel": room, "error": "The panel has not reported its alarms yet (it may be offline); try again later"},
        error=True,
    )


class SetPanelAlarm(Tool):
    """An alarm on a panel named by its room or name."""

    name = f"{DOMAIN}__SetPanelAlarm"
    title = "Set an alarm on a wall panel"
    description = (
        "Sets an alarm (a wake-up time of day, not a timer) on a PanelOS wall panel named by its room or name, also "
        "when the user talks to another device (for example 'wake me at 6:45 on weekdays with the bedroom panel'). "
        "The panel rings it at that time; without days it rings once, the next time that time comes. An alarm that "
        "already exists at that time and on those days is switched on instead of added again."
    )
    parameters = schema.Schema(
        {
            schema.Required("panel", description=PANEL_HELP): cv.string,
            schema.Required("hour", description="Hour of the day, 0-23 (24-hour clock)."): cv.positive_int,
            schema.Optional("minute", description="Minute, 0-59."): cv.positive_int,
            schema.Optional(
                "days",
                description="Repeat days: weekday names in English (monday … sunday), or 'weekdays', 'weekends', 'every day'. Leave out for once.",
            ): [cv.string],
            schema.Optional("label", description="The alarm's name, if the user gave one."): cv.string,
        }
    )
    annotations = ToolAnnotations(destructive=False, open_world=False)
    integration = DOMAIN

    async def async_call(self, hass: HomeAssistant, tool_input: ToolInput, llm_context: LLMContext) -> ToolResult:
        args = tool_input.tool_args
        said = str(args.get("panel", ""))
        panel, known = _find(hass, said, timers=False)
        if panel is None:
            return _no_panel(said, known)
        room = _room(hass, panel)
        hour, minute = int(args["hour"]), int(args.get("minute") or 0)
        if not (0 <= hour <= 23 and 0 <= minute <= 59):
            return ToolResult(data={"success": False, "error": "The time needs an hour 0-23 and a minute 0-59"}, error=True)
        days = _days(args.get("days") or [])
        if days is None:
            return ToolResult(data={"success": False, "error": "Days not understood: use English weekday names, 'weekdays', 'weekends' or 'every day'"}, error=True)
        label = str(args.get("label") or "").strip()[:32]
        alarms = _alarms(panel)
        if alarms is None:
            return _no_alarms(room)
        same = next((a for a in alarms if int(a.get("h", -1)) == hour and int(a.get("m", -1)) == minute
                     and [bool(d) for d in (a.get("days") or [])[:7]] + [False] * (7 - len((a.get("days") or [])[:7])) == days), None)
        if same is not None:
            same["on"] = True
            if label:
                same["label"] = label
        else:
            if len(alarms) >= ALARMS_MAX:
                return ToolResult(data={"success": False, "panel": room, "error": f"The panel already has {ALARMS_MAX} alarms"}, error=True)
            same = {"h": hour, "m": minute, "days": days, "label": label, "on": True, "tone": "auto", "tone_name": ""}
            alarms.append(same)
        sent = _send_alarms(hass, panel, alarms)
        return ToolResult(data={"success": True, "panel": room, "alarm": _alarm_out(same), "delivered_now": sent})


class ListPanelAlarms(Tool):
    """The alarms of the panels."""

    name = f"{DOMAIN}__ListPanelAlarms"
    title = "List the alarms on the wall panels"
    description = (
        "Lists the alarms (wake-up times) of PanelOS wall panels, one panel by its room or name or all of them: time "
        "(24-hour), repeat days, name and whether each is on. Use it for questions like 'what alarms do I have'."
    )
    parameters = schema.Schema({schema.Optional("panel", description=PANEL_HELP): cv.string})
    annotations = ToolAnnotations(read_only=True, open_world=False)
    integration = DOMAIN

    async def async_call(self, hass: HomeAssistant, tool_input: ToolInput, llm_context: LLMContext) -> ToolResult:
        said = str(tool_input.tool_args.get("panel") or "")
        if said:
            panel, known = _find(hass, said, timers=False)
            if panel is None:
                return _no_panel(said, known)
            panels = [panel]
        else:
            panels = [p for p, _ in _panels(hass, timers=False)]
        out = []
        for p in panels:
            alarms = _alarms(p)
            out.append({"panel": _room(hass, p), "alarms": None if alarms is None else [_alarm_out(a) for a in alarms]})
        return ToolResult(data={"panels": out})


class CancelPanelAlarm(Tool):
    """Cancel (delete) or switch off a panel's alarm."""

    name = f"{DOMAIN}__CancelPanelAlarm"
    title = "Cancel an alarm on a wall panel"
    description = (
        "Cancels an alarm on a PanelOS wall panel named by its room or name: by its time (hour, minute) and/or its "
        "name; all=true for every alarm on the panel. keep=true only switches it off (it stays on the panel to switch "
        "on again); otherwise it is deleted. With several matching alarms, ask which one or pass all=true."
    )
    parameters = schema.Schema(
        {
            schema.Required("panel", description=PANEL_HELP): cv.string,
            schema.Optional("hour", description="The alarm's hour, 0-23."): cv.positive_int,
            schema.Optional("minute", description="The alarm's minute, 0-59."): cv.positive_int,
            schema.Optional("label", description="The alarm's name."): cv.string,
            schema.Optional("all", description="Every matching alarm (every alarm on the panel when nothing else is given)."): cv.boolean,
            schema.Optional("keep", description="Only switch it off instead of deleting it."): cv.boolean,
        }
    )
    annotations = ToolAnnotations(destructive=True, idempotent=True, open_world=False)
    integration = DOMAIN

    async def async_call(self, hass: HomeAssistant, tool_input: ToolInput, llm_context: LLMContext) -> ToolResult:
        args = tool_input.tool_args
        said = str(args.get("panel", ""))
        panel, known = _find(hass, said, timers=False)
        if panel is None:
            return _no_panel(said, known)
        room = _room(hass, panel)
        alarms = _alarms(panel)
        if alarms is None:
            return _no_alarms(room)
        hour = int(args["hour"]) if args.get("hour") is not None else None
        minute = int(args["minute"]) if args.get("minute") is not None else None
        label = _norm(str(args.get("label") or ""))
        hits = [a for a in alarms if _alarm_matches(a, hour, minute, label)]
        if not hits:
            return ToolResult(data={"success": False, "panel": room, "error": "No such alarm on the panel",
                                    "alarms": [_alarm_out(a) for a in alarms]}, error=True)
        if len(hits) > 1 and not args.get("all"):
            return ToolResult(
                data={"success": False, "panel": room, "error": "Several alarms match: ask which one, or pass all=true",
                      "alarms": [_alarm_out(a) for a in hits]},
                error=True,
            )
        # the panel ends their snooze too (an alarm that rang once is off already, and still rings at its snooze time)
        cancelled = [{"h": a.get("h"), "m": a.get("m"), "days": a.get("days") or [], "label": a.get("label") or ""} for a in hits]
        if args.get("keep"):
            for a in hits:
                a["on"] = False
        else:
            alarms = [a for a in alarms if not any(a is h for h in hits)]
        sent = _send_alarms(hass, panel, alarms, cancelled)
        return ToolResult(data={"success": True, "panel": room, "switched_off" if args.get("keep") else "deleted": len(hits),
                                "delivered_now": sent})


@callback
def async_get_tools(hass: HomeAssistant, llm_context: LLMContext, api_id: str) -> LLMTools | None:
    """The panel timer and alarm tools for agents that use the Assist API, while a panel is set up."""
    if api_id != LLM_API_ASSIST:
        return None
    timer_panels = _panels(hass)
    alarm_panels = _panels(hass, timers=False)
    if not alarm_panels:
        return None
    tools: list[Tool] = []
    lines = [f"PanelOS wall panels: {', '.join(_room(hass, p) for p, _ in alarm_panels)}."]
    if timer_panels:
        tools += [StartPanelTimer(), CancelPanelTimer(), ListPanelTimers()]
        lines.append(
            f"The {DOMAIN}__ timer tools start, list and cancel the timers of these panels, whether the user talks to the "
            "panel itself or to another device (for example 'set a timer on the kitchen panel'). The time left on these "
            f"timers can be told: when asked which timers run or how much time is left, call {DOMAIN}__ListPanelTimers "
            "and answer from it."
        )
    tools += [SetPanelAlarm(), ListPanelAlarms(), CancelPanelAlarm()]
    lines.append(
        f"The {DOMAIN}__ alarm tools set, list and cancel the panels' alarms (wake-up times of day the panel rings, also "
        "repeating on weekdays); a length of time is a timer, a time of day is an alarm."
    )
    return LLMTools(tools=tools, prompt=" ".join(lines))
