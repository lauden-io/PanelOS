"""Config and options flow for the PanelOS integration."""

from __future__ import annotations

from typing import Any

import voluptuous as vol

from homeassistant.config_entries import ConfigEntry, ConfigFlow, ConfigFlowResult, OptionsFlow
from homeassistant.core import HomeAssistant, callback
from homeassistant.helpers import selector

from .resources import async_resources

from .const import (
    CONF_DASHBOARD,
    CONF_MEDIA_PLAYER,
    CONF_PANEL_ID,
    CONF_SHOW_CONVERSATION,
    CONF_SHOW_HEADER,
    CONF_KEEP_RESOURCES,
    CONF_SHOW_SIDEBAR,
    CONF_SKIP_UNUSED,
    CONF_WEATHER,
    CONF_ZOOM,
    CONF_LOADING_SCREEN,
    DASHBOARD_FROM_PANEL,
    DEFAULT_PANEL_ID,
    DOMAIN,
    OLD_OPTIONS,
    REPORTING,
)


def dashboard_choices(hass: HomeAssistant) -> list[selector.SelectOptionDict]:
    """The Home Assistant dashboards, the default one first."""
    choices = [selector.SelectOptionDict(value=DASHBOARD_FROM_PANEL, label="Chosen on the panel")]
    data = hass.data.get("lovelace")
    dashboards = getattr(data, "dashboards", None)
    if dashboards is None and isinstance(data, dict):
        dashboards = data.get("dashboards")
    found = []
    for url_path, dashboard in (dashboards or {}).items():
        if url_path is None:
            choices.append(selector.SelectOptionDict(value="lovelace", label="Overview"))
            continue
        if url_path == "lovelace":
            continue  # the default dashboard, listed above
        config = getattr(dashboard, "config", None) or {}
        found.append(selector.SelectOptionDict(value=url_path, label=config.get("title") or url_path))
    return choices + sorted(found, key=lambda c: c["label"].lower())


def _dashboard_schema(hass: HomeAssistant, defaults: dict[str, Any], resources: list[str]) -> vol.Schema:
    choices = dashboard_choices(hass)
    current = defaults.get(CONF_DASHBOARD, DASHBOARD_FROM_PANEL)
    if current not in {c["value"] for c in choices}:
        choices.append(selector.SelectOptionDict(value=current, label=current))
    return vol.Schema(
        {
            vol.Required(CONF_DASHBOARD, default=current): selector.SelectSelector(
                selector.SelectSelectorConfig(options=choices, mode=selector.SelectSelectorMode.DROPDOWN)
            ),
            vol.Required(CONF_SHOW_HEADER, default=defaults.get(CONF_SHOW_HEADER, True)): bool,
            vol.Required(CONF_SHOW_SIDEBAR, default=defaults.get(CONF_SHOW_SIDEBAR, False)): bool,
            vol.Required(CONF_ZOOM, default=defaults.get(CONF_ZOOM, 100)): selector.NumberSelector(
                selector.NumberSelectorConfig(
                    min=50, max=150, step=5, unit_of_measurement="%", mode=selector.NumberSelectorMode.SLIDER
                )
            ),
            vol.Required(CONF_LOADING_SCREEN, default=defaults.get(CONF_LOADING_SCREEN, True)): bool,
            vol.Required(CONF_SKIP_UNUSED, default=defaults.get(CONF_SKIP_UNUSED, True)): bool,
            vol.Optional(CONF_KEEP_RESOURCES, default=defaults.get(CONF_KEEP_RESOURCES, [])): selector.SelectSelector(
                selector.SelectSelectorConfig(
                    options=sorted(set(resources) | set(defaults.get(CONF_KEEP_RESOURCES, []))),
                    multiple=True,
                    mode=selector.SelectSelectorMode.LIST,
                )
            ),
        }
    )


def _assistant_schema(defaults: dict[str, Any]) -> vol.Schema:
    return vol.Schema(
        {
            vol.Optional(CONF_WEATHER, description={"suggested_value": defaults.get(CONF_WEATHER)}): selector.EntitySelector(
                selector.EntitySelectorConfig(domain="weather")
            ),
            vol.Optional(
                CONF_MEDIA_PLAYER, description={"suggested_value": defaults.get(CONF_MEDIA_PLAYER)}
            ): selector.EntitySelector(selector.EntitySelectorConfig(domain="media_player")),
            vol.Optional(CONF_SHOW_CONVERSATION, default=defaults.get(CONF_SHOW_CONVERSATION, True)): bool,
        }
    )


def _reporting_schema(defaults: dict[str, Any]) -> vol.Schema:
    return vol.Schema(
        {
            vol.Required(key, default=defaults.get(key, default)): selector.NumberSelector(
                selector.NumberSelectorConfig(
                    min=low, max=high, step=step, unit_of_measurement=unit, mode=selector.NumberSelectorMode.SLIDER
                )
            )
            for key, (default, low, high, step, unit) in REPORTING.items()
        }
    )


class PanelOSConfigFlow(ConfigFlow, domain=DOMAIN):
    """Add a panel."""

    VERSION = 1

    async def async_step_user(self, user_input: dict[str, Any] | None = None) -> ConfigFlowResult:
        if user_input is not None:
            panel_id = user_input[CONF_PANEL_ID].strip()
            await self.async_set_unique_id(panel_id)
            self._abort_if_unique_id_configured()
            data = {CONF_PANEL_ID: panel_id}
            options = {k: v for k, v in user_input.items() if k != CONF_PANEL_ID}
            return self.async_create_entry(title=f"PanelOS ({panel_id})", data=data, options=options)
        schema = vol.Schema({vol.Required(CONF_PANEL_ID, default=DEFAULT_PANEL_ID): str}).extend(
            _assistant_schema({}).schema
        )
        return self.async_show_form(step_id="user", data_schema=schema)

    @staticmethod
    @callback
    def async_get_options_flow(config_entry: ConfigEntry) -> OptionsFlow:
        return PanelOSOptionsFlow()


class PanelOSOptionsFlow(OptionsFlow):
    """Change what the panel shows and how its sensors report."""

    async def async_step_init(self, user_input: dict[str, Any] | None = None) -> ConfigFlowResult:
        return self.async_show_menu(step_id="init", menu_options=["dashboard", "assistant", "sensor_reporting"])

    async def async_step_dashboard(self, user_input: dict[str, Any] | None = None) -> ConfigFlowResult:
        resources = [r["url"] for r in await async_resources(self.hass)]
        schema = _dashboard_schema(self.hass, dict(self.config_entry.options), resources)
        if user_input is not None:
            user_input = {**user_input, CONF_ZOOM: int(user_input.get(CONF_ZOOM, 100))}
            return self._save(schema, user_input)
        return self.async_show_form(step_id="dashboard", data_schema=schema)

    async def async_step_assistant(self, user_input: dict[str, Any] | None = None) -> ConfigFlowResult:
        schema = _assistant_schema(dict(self.config_entry.options))
        if user_input is not None:
            return self._save(schema, user_input)
        return self.async_show_form(step_id="assistant", data_schema=schema)

    async def async_step_sensor_reporting(self, user_input: dict[str, Any] | None = None) -> ConfigFlowResult:
        schema = _reporting_schema(dict(self.config_entry.options))
        if user_input is not None:
            return self._save(schema, user_input)
        return self.async_show_form(step_id="sensor_reporting", data_schema=schema)

    def _save(self, schema: vol.Schema, user_input: dict[str, Any]) -> ConfigFlowResult:
        # A step replaces only its own fields (a cleared entity field is
        # missing from user_input and must not keep its old value); options of
        # earlier versions go.
        own = {str(key) for key in schema.schema}
        kept = {k: v for k, v in self.config_entry.options.items() if k not in own and k not in OLD_OPTIONS}
        return self.async_create_entry(data={**kept, **user_input})
