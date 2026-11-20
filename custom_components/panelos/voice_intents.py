"""Music by voice on a panel: Home Assistant's own media intents act on the panel's chosen player.

Home Assistant brings the sentences itself, in every language it supports and in many variations ("play Queen",
"next song", "skip this", "play the previous track", "pause", "stop the music", "resume", "volume up", "set the volume
to 30 percent"; in Hungarian "következő szám", "játszd az előző számot", "szünet", "folytasd", "hangosabban" …). Said
to a panel's voice satellite without naming a player, a kind of device, an area or a floor, they go to the player
picked on the panel's music page, or to the panel itself (Voice page: Music commands; see ``panel.voice_player``).
Everything else (another device, a named player or room, a panel without a player) goes to Home Assistant's own
handler unchanged. A conversation agent's tool calls carry the device too, so the same holds for what an AI agent
does with a command (a plain "play" becomes resume there).

The handlers of the media player integration are wrapped once Home Assistant has started (it registers them while it
sets up), and again whenever a handler of these intents is registered anew (an intent_script or conversation reload,
an integration set up later); the reply is Home Assistant's own for that intent, in the language of the command. A
chosen player that cannot do what was asked gets Home Assistant's own "not supported" reply; an unavailable one leaves
the command to Home Assistant's own handler.
"""

from __future__ import annotations

import asyncio
import logging
from typing import Any

from homeassistant.components.media_player import (
    ATTR_MEDIA_POSITION,
    ATTR_MEDIA_POSITION_UPDATED_AT,
    ATTR_MEDIA_SEEK_POSITION,
    ATTR_MEDIA_VOLUME_LEVEL,
    ATTR_MEDIA_VOLUME_MUTED,
    DOMAIN as MEDIA_PLAYER_DOMAIN,
    SERVICE_MEDIA_SEEK,
    MediaPlayerEntityFeature,
)
from homeassistant.const import (
    ATTR_ENTITY_ID,
    EVENT_CALL_SERVICE,
    EVENT_COMPONENT_LOADED,
    SERVICE_MEDIA_NEXT_TRACK,
    SERVICE_MEDIA_PAUSE,
    SERVICE_MEDIA_PLAY,
    SERVICE_MEDIA_PREVIOUS_TRACK,
    SERVICE_VOLUME_DOWN,
    SERVICE_VOLUME_MUTE,
    SERVICE_VOLUME_SET,
    SERVICE_VOLUME_UP,
    STATE_UNAVAILABLE,
)
from homeassistant.core import Event, HomeAssistant, callback
from homeassistant.exceptions import HomeAssistantError
from homeassistant.helpers import area_registry as ar, device_registry as dr, entity_registry as er, intent
from homeassistant.helpers.event import async_call_later
from homeassistant.util import dt as dt_util

from .const import DOMAIN

try:
    from homeassistant.exceptions import ServiceNotSupported
except ImportError:  # an older Home Assistant: the feature check before the call is all
    ServiceNotSupported = None  # type: ignore[assignment,misc]

_LOGGER = logging.getLogger(__name__)

# intent → the player's service (volume intents are worked out from their slot)
_SERVICES = {
    "HassMediaNext": SERVICE_MEDIA_NEXT_TRACK,
    "HassMediaPrevious": SERVICE_MEDIA_PREVIOUS_TRACK,
    "HassMediaPause": SERVICE_MEDIA_PAUSE,
    "HassMediaUnpause": SERVICE_MEDIA_PLAY,
    "HassSetVolume": SERVICE_VOLUME_SET,
    "HassSetVolumeRelative": None,
    "HassMediaPlayerMute": SERVICE_VOLUME_MUTE,
    "HassMediaPlayerUnmute": SERVICE_VOLUME_MUTE,
    "HassMediaSearchAndPlay": None,
}
# a target named in the command: Home Assistant's own handler finds it (an area slot that is the panel's own room is no
# named target: Home Assistant fills it from the satellite's area when the sentence names none)
_TARGET_SLOTS = ("name", "floor", "device_class")
# search and play needs a player that can do both
_SEARCH_AND_PLAY = MediaPlayerEntityFeature.SEARCH_MEDIA | MediaPlayerEntityFeature.PLAY_MEDIA
# what the player must support for each intent (as Home Assistant's own handlers ask; resume: the play service)
_FEATURES = {
    "HassMediaNext": MediaPlayerEntityFeature.NEXT_TRACK,
    "HassMediaPrevious": MediaPlayerEntityFeature.PREVIOUS_TRACK,
    "HassMediaPause": MediaPlayerEntityFeature.PAUSE,
    "HassMediaUnpause": MediaPlayerEntityFeature.PLAY,
    "HassSetVolume": MediaPlayerEntityFeature.VOLUME_SET,
    "HassSetVolumeRelative": MediaPlayerEntityFeature.VOLUME_SET,
    "HassMediaPlayerMute": MediaPlayerEntityFeature.VOLUME_MUTE,
    "HassMediaPlayerUnmute": MediaPlayerEntityFeature.VOLUME_MUTE,
}
# after a reload service (intent_script, conversation, all YAML) the wrap is checked again this many seconds later: the
# event comes before the reload has registered its handlers
_RECHECK_AFTER_RELOAD = (1, 5, 15)
_LISTENING = "voice_intents_listening"


def _slot(intent_obj: intent.Intent, key: str) -> Any:
    value = (intent_obj.slots.get(key) or {}).get("value")
    return None if value in (None, "") else value


def _elapsed(hass: HomeAssistant, player: str) -> float:
    """Seconds the player's current track has played (0 when it does not say)."""
    state = hass.states.get(player)
    if state is None:
        return 0.0
    try:
        position = float(state.attributes.get(ATTR_MEDIA_POSITION) or 0)
    except (TypeError, ValueError):
        return 0.0
    updated = state.attributes.get(ATTR_MEDIA_POSITION_UPDATED_AT)
    if state.state == "playing" and updated is not None:
        updated = dt_util.parse_datetime(updated) if isinstance(updated, str) else updated
        if updated is not None:
            position += (dt_util.utcnow() - updated).total_seconds()
    return position


def _satellite(hass: HomeAssistant, intent_obj: intent.Intent) -> str | None:
    if getattr(intent_obj, "satellite_id", None):
        return intent_obj.satellite_id
    device_id = getattr(intent_obj, "device_id", None)
    if not device_id:
        return None
    for entry in er.async_entries_for_device(er.async_get(hass), device_id):
        if entry.domain == "assist_satellite":
            return entry.entity_id
    return None


def _own_area(hass: HomeAssistant, satellite: str, area: Any) -> bool:
    """Whether an area slot names the satellite's own room (its id, name or an alias)."""
    entity = er.async_get(hass).async_get(satellite)
    device = dr.async_get(hass).async_get(entity.device_id) if entity and entity.device_id else None
    area_id = (entity.area_id if entity else None) or (device.area_id if device else None)
    if not area_id and device and device.via_device_id:
        parent = dr.async_get(hass).async_get(device.via_device_id)
        area_id = parent.area_id if parent else None
    entry = ar.async_get(hass).async_get_area(area_id) if area_id else None
    if entry is None:
        return False
    said = str(area).strip().casefold()
    return said in {entry.id.casefold(), entry.name.casefold(), *(a.casefold() for a in entry.aliases)}


def _panel_player(hass: HomeAssistant, intent_obj: intent.Intent) -> str | None:
    """The player a command said to a panel acts on; None when Home Assistant's own handler should take it."""
    if any(_slot(intent_obj, key) is not None for key in _TARGET_SLOTS):
        return None
    satellite = _satellite(hass, intent_obj)
    if not satellite:
        return None
    area = _slot(intent_obj, "area")
    if area is not None and not _own_area(hass, satellite, area):
        return None
    from .panel import voice_player  # noqa: PLC0415

    player = voice_player(hass, satellite)
    state = hass.states.get(player) if player else None
    if state is None:
        return None
    if state.state == STATE_UNAVAILABLE:
        # a service call would skip it and the reply would say done: Home Assistant's own handler answers instead
        _LOGGER.debug("%s is unavailable; %s goes to Home Assistant's own handler", player, intent_obj.intent_type)
        return None
    return player


def _can_search(hass: HomeAssistant, player: str) -> bool:
    """The player can search and play (a player that cannot: Home Assistant's own handler picks one)."""
    state = hass.states.get(player)
    features = int(state.attributes.get("supported_features") or 0) if state else 0
    return features & _SEARCH_AND_PLAY == _SEARCH_AND_PLAY


def _features(hass: HomeAssistant, player: str) -> int:
    state = hass.states.get(player)
    try:
        return int(state.attributes.get("supported_features") or 0) if state else 0
    except (TypeError, ValueError):
        return 0


def _not_supported(features: int) -> intent.IntentError:
    """Home Assistant's own error for a device that cannot do what was asked (its reply: "not supported", in the
    language of the command)."""
    try:
        return intent.MatchFailedError(
            result=intent.MatchTargetsResult(False, intent.MatchFailedReason.FEATURE),
            constraints=intent.MatchTargetsConstraints(domains={MEDIA_PLAYER_DOMAIN}, features=features),
        )
    except (AttributeError, TypeError):  # an older Home Assistant without these classes
        return intent.IntentHandleError("The player does not support this")


def _field(item: Any, key: str) -> Any:
    return item.get(key) if isinstance(item, dict) else getattr(item, key, None)


async def _search_and_play(hass: HomeAssistant, intent_obj: intent.Intent, player: str) -> intent.IntentResponse:
    """As Home Assistant's own HassMediaSearchAndPlay, on the panel's player: search, then play the first result."""
    query = _slot(intent_obj, "search_query")
    if query is None:
        raise intent.IntentHandleError("Nothing to search for")
    search: dict[str, Any] = {ATTR_ENTITY_ID: player, "search_query": str(query)}
    if media_class := _slot(intent_obj, "media_class"):
        search["media_filter_classes"] = [media_class]
    try:
        found = await hass.services.async_call(
            MEDIA_PLAYER_DOMAIN, "search_media", search, blocking=True, context=intent_obj.context, return_response=True
        )
    except HomeAssistantError as err:
        raise intent.IntentHandleError(f"Error searching media: {err}") from err
    results = _field((found or {}).get(player), "result") or []
    if not results:
        raise intent.IntentHandleError(f"No results found for {query}")
    first = results[0]
    try:
        await hass.services.async_call(
            MEDIA_PLAYER_DOMAIN,
            "play_media",
            {
                ATTR_ENTITY_ID: player,
                "media_content_id": _field(first, "media_content_id"),
                "media_content_type": _field(first, "media_content_type"),
            },
            blocking=True,
            context=intent_obj.context,
        )
    except HomeAssistantError as err:
        raise intent.IntentHandleError(f"Error playing media: {err}") from err
    _LOGGER.debug("HassMediaSearchAndPlay %r on %s (said to a panel)", query, player)
    response = intent_obj.create_response()
    response.async_set_speech_slots({"media": first.as_dict() if hasattr(first, "as_dict") else dict(first)})
    return response


async def _act(hass: HomeAssistant, intent_obj: intent.Intent, intent_type: str, player: str) -> intent.IntentResponse:
    if intent_type == "HassMediaSearchAndPlay":
        return await _search_and_play(hass, intent_obj, player)
    needed = int(_FEATURES[intent_type])
    if _features(hass, player) & needed != needed:
        _LOGGER.debug("%s cannot do %s (said to a panel)", player, intent_type)
        raise _not_supported(needed)
    data: dict[str, Any] = {ATTR_ENTITY_ID: player}
    service = _SERVICES[intent_type]
    if intent_type == "HassSetVolume":
        level = _slot(intent_obj, ATTR_MEDIA_VOLUME_LEVEL)
        try:
            data[ATTR_MEDIA_VOLUME_LEVEL] = max(0, min(100, int(level))) / 100
        except (TypeError, ValueError) as err:
            raise intent.IntentHandleError(f"Volume not understood: {level}") from err
    elif intent_type == "HassSetVolumeRelative":
        step = _slot(intent_obj, "volume_step")
        if step == "up":
            service = SERVICE_VOLUME_UP
        elif step == "down":
            service = SERVICE_VOLUME_DOWN
        else:
            try:
                change = int(step) / 100
            except (TypeError, ValueError) as err:
                raise intent.IntentHandleError(f"Volume step not understood: {step}") from err
            state = hass.states.get(player)
            now = float(state.attributes.get(ATTR_MEDIA_VOLUME_LEVEL) or 0) if state else 0.0
            service = SERVICE_VOLUME_SET
            data[ATTR_MEDIA_VOLUME_LEVEL] = max(0.0, min(1.0, now + change))
    elif intent_type in ("HassMediaPlayerMute", "HassMediaPlayerUnmute"):
        data[ATTR_MEDIA_VOLUME_MUTED] = intent_type == "HassMediaPlayerMute"
    elif intent_type == "HassMediaPrevious" and _elapsed(hass, player) > 3:
        # a player's "previous" restarts the current track once it has played a few seconds (the button's habit):
        # back to its start first, so "previous" by voice really goes to the previous track
        try:
            await hass.services.async_call(
                MEDIA_PLAYER_DOMAIN, SERVICE_MEDIA_SEEK, {ATTR_ENTITY_ID: player, ATTR_MEDIA_SEEK_POSITION: 0}, blocking=True, context=intent_obj.context
            )
            await asyncio.sleep(0.7)
        except Exception:  # noqa: BLE001 - a player that cannot seek (a radio stream): plain previous
            _LOGGER.debug("%s cannot seek; previous as it is", player)
    try:
        await hass.services.async_call(MEDIA_PLAYER_DOMAIN, service, data, blocking=True, context=intent_obj.context)
    except HomeAssistantError as err:
        if ServiceNotSupported is not None and isinstance(err, ServiceNotSupported):
            raise _not_supported(needed) from err
        raise
    _LOGGER.debug("%s on %s (said to a panel)", intent_type, player)
    response = intent_obj.create_response()
    response.response_type = intent.IntentResponseType.ACTION_DONE
    if state := hass.states.get(player):
        response.async_set_states([state])
    return response


@callback
def _wrap(hass: HomeAssistant) -> None:
    """Wraps the media intent handlers registered now; a handler already wrapped is left as it is."""
    handlers = hass.data.get(intent.DATA_KEY) or {}
    wrapped = []
    for intent_type in _SERVICES:
        handler = handlers.get(intent_type)
        if handler is None or getattr(handler, "_panelos_wrapped", False):
            continue
        original = handler.async_handle

        async def async_handle(intent_obj: intent.Intent, _original=original, _type=intent_type) -> intent.IntentResponse:
            player = _panel_player(intent_obj.hass, intent_obj)
            if player is None or (_type == "HassMediaSearchAndPlay" and not _can_search(intent_obj.hass, player)):
                return await _original(intent_obj)
            return await _act(intent_obj.hass, intent_obj, _type, player)

        handler.async_handle = async_handle  # type: ignore[method-assign]
        handler._panelos_wrapped = True  # noqa: SLF001
        wrapped.append(intent_type)
    if wrapped:
        _LOGGER.info("Music by voice on panels: %s act on each panel's chosen player", ", ".join(wrapped))


@callback
def async_setup_voice_intents(hass: HomeAssistant) -> None:
    """Wraps Home Assistant's media intent handlers (call after Home Assistant has started), and again whenever they
    may have been registered anew: after a reload service and when an integration has been set up."""
    _wrap(hass)
    data = hass.data.setdefault(DOMAIN, {})
    if data.get(_LISTENING):
        return
    data[_LISTENING] = True

    @callback
    def _recheck(_now: Any) -> None:
        _wrap(hass)

    @callback
    def _service_called(event: Event) -> None:
        if event.data.get("service") in ("reload", "reload_all"):
            for delay in _RECHECK_AFTER_RELOAD:
                async_call_later(hass, delay, _recheck)

    @callback
    def _component_loaded(_event: Event) -> None:
        _wrap(hass)

    hass.bus.async_listen(EVENT_CALL_SERVICE, _service_called)
    hass.bus.async_listen(EVENT_COMPONENT_LOADED, _component_loaded)
