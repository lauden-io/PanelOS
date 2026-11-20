"""Selects of a panel: dashboard, theme, screensaver, display mode, time format.

The panel sends the options with its report (the dashboards are the ones its
Home Assistant user may open), so they always match its own settings screen.
"""

from __future__ import annotations

from homeassistant.components.select import SelectEntity, SelectEntityDescription
from homeassistant.const import EntityCategory
from homeassistant.core import HomeAssistant
from homeassistant.helpers.entity_platform import AddEntitiesCallback

from . import PanelOSConfigEntry
from .entity import PanelOSEntity


def _setting(key: str, icon: str) -> SelectEntityDescription:
    return SelectEntityDescription(key=key, translation_key=key, icon=icon, entity_category=EntityCategory.CONFIG)


DESCRIPTIONS = {
    "dashboard": _setting("dashboard", "mdi:view-dashboard"),
    "theme": _setting("theme", "mdi:theme-light-dark"),
    "screensaver": _setting("screensaver", "mdi:timer-outline"),
    "screensaver_style": _setting("screensaver_style", "mdi:clock-outline"),
    "display_mode": _setting("display_mode", "mdi:monitor-dashboard"),
    "time_format": _setting("time_format", "mdi:hours-24"),
}


async def async_setup_entry(
    hass: HomeAssistant, entry: PanelOSConfigEntry, async_add_entities: AddEntitiesCallback
) -> None:
    entry.runtime_data.add_platform("select", async_add_entities, lambda panel, key: PanelOSSelect(panel, DESCRIPTIONS[key]))


class PanelOSSelect(PanelOSEntity, SelectEntity):
    @property
    def options(self) -> list[str]:
        return self._panel.options.get(self._key, [])

    @property
    def current_option(self) -> str | None:
        value = self._value
        return value if isinstance(value, str) and value in self.options else None

    async def async_select_option(self, option: str) -> None:
        self._command(option)
