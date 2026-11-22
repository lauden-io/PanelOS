"""What ``panelos.show_view`` sends to the panels."""

from __future__ import annotations

from typing import Any

from homeassistant.core import HomeAssistant
from homeassistant.exceptions import ServiceValidationError


def view_request(hass: HomeAssistant, data: dict[str, Any]) -> dict[str, Any]:
    """The request for the panels from the action's fields (the panel ID already taken out).

    A camera view with ``entity_id`` carries the camera and its name (the panel shows the name when none of its cameras
    pages has that camera); the other views go without a camera. A camera Home Assistant does not have is an error.
    """
    request = {key: value for key, value in data.items() if key != "entity_id"}
    entity_id = data.get("entity_id")
    if data.get("view") == "camera" and entity_id:
        state = hass.states.get(entity_id)
        if state is None:
            raise ServiceValidationError(f"Home Assistant has no camera {entity_id}")
        request["entity_id"] = entity_id
        request["name"] = state.name
    request["type"] = "view"
    return request
