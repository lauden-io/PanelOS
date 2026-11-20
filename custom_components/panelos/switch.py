"""Switches of a panel: screen on/off, automatic brightness, wake on approach, child lock."""

from __future__ import annotations

from typing import Any

from homeassistant.components.switch import SwitchEntity, SwitchEntityDescription
from homeassistant.const import EntityCategory
from homeassistant.core import HomeAssistant
from homeassistant.helpers.entity_platform import AddEntitiesCallback

from . import PanelOSConfigEntry
from .entity import PanelOSEntity

DESCRIPTIONS = {
    # Off starts the screensaver, on wakes the panel.
    "screen": SwitchEntityDescription(key="screen", translation_key="screen", icon="mdi:tablet"),
    "auto_brightness": SwitchEntityDescription(
        key="auto_brightness",
        translation_key="auto_brightness",
        icon="mdi:brightness-auto",
        entity_category=EntityCategory.CONFIG,
    ),
    "proximity_wake": SwitchEntityDescription(
        key="proximity_wake",
        translation_key="proximity_wake",
        icon="mdi:motion-sensor",
        entity_category=EntityCategory.CONFIG,
    ),
    # On: touches do nothing until the lock is held on the panel (it locks again after a minute without a touch).
    "child_lock": SwitchEntityDescription(key="child_lock", translation_key="child_lock", icon="mdi:account-lock"),
}


async def async_setup_entry(
    hass: HomeAssistant, entry: PanelOSConfigEntry, async_add_entities: AddEntitiesCallback
) -> None:
    entry.runtime_data.add_platform("switch", async_add_entities, lambda panel, key: PanelOSSwitch(panel, DESCRIPTIONS[key]))


class PanelOSSwitch(PanelOSEntity, SwitchEntity):
    @property
    def is_on(self) -> bool | None:
        return self._value if isinstance(self._value, bool) else None

    async def async_turn_on(self, **kwargs: Any) -> None:
        self._command(True)

    async def async_turn_off(self, **kwargs: Any) -> None:
        self._command(False)
