"""The options without the old quick tiles (custom_components/panelos/config_flow.py, panel.py).

Runs with the standard library only: python3 -m unittest discover -s tests

The modules are loaded against small stand-ins for Home Assistant and voluptuous; sys.modules is put back afterwards,
so a real Home Assistant in the same run is not touched.
"""

from __future__ import annotations

import importlib
from pathlib import Path
import sys
import types
import unittest

from test_panel_user import panel

PACKAGE = "panelos_show_view_test"
PACKAGE_DIR = Path(__file__).resolve().parents[1] / "custom_components" / "panelos"


def _module(name: str, package: bool = False, **attrs) -> types.ModuleType:
    m = types.ModuleType(name)
    m.__dict__.update(attrs)
    if package:
        m.__path__ = []
    return m


class Placeholder:
    def __init__(self, *args, **kwargs) -> None:
        self.args, self.kwargs = args, kwargs


class _Key(str):
    """vol.Optional / vol.Required: the key's name, with its default."""

    def __new__(cls, schema: str, default=None, description=None, **kwargs):
        key = str.__new__(cls, schema)
        key.default, key.description = default, description
        return key


class _Schema:
    def __init__(self, schema: dict) -> None:
        self.schema = dict(schema)

    def extend(self, other: dict) -> _Schema:
        return _Schema({**self.schema, **other})


class _FlowBase:
    def __init_subclass__(cls, **kwargs) -> None:
        super().__init_subclass__()

    def async_create_entry(self, **kwargs) -> dict:
        return kwargs

    def async_show_form(self, **kwargs) -> dict:
        return kwargs

    def async_show_menu(self, **kwargs) -> dict:
        return kwargs


def _stubs() -> dict[str, types.ModuleType]:
    selector = _module("homeassistant.helpers.selector")
    selector.__getattr__ = lambda name: Placeholder
    helpers = _module("homeassistant.helpers", package=True, selector=selector)
    modules = [
        _module("voluptuous", Schema=_Schema, Optional=_Key, Required=_Key),
        _module("homeassistant", package=True),
        _module(
            "homeassistant.config_entries",
            ConfigEntry=Placeholder,
            ConfigFlow=_FlowBase,
            ConfigFlowResult=dict,
            OptionsFlow=_FlowBase,
        ),
        _module("homeassistant.core", HomeAssistant=Placeholder, callback=lambda func: func),
        helpers,
        selector,
        _module(f"{PACKAGE}.resources", async_resources=None),
    ]
    return {m.__name__: m for m in modules}


def _load(name: str) -> types.ModuleType:
    stubs = _stubs()
    names = [*stubs, PACKAGE, f"{PACKAGE}.{name}", f"{PACKAGE}.const"]
    saved = {n: sys.modules.get(n) for n in names}
    write_bytecode = sys.dont_write_bytecode
    try:
        sys.dont_write_bytecode = True
        sys.modules.update(stubs)
        package = types.ModuleType(PACKAGE)
        package.__path__ = [str(PACKAGE_DIR)]
        sys.modules[PACKAGE] = package
        return importlib.import_module(f"{PACKAGE}.{name}")
    finally:
        sys.dont_write_bytecode = write_bytecode
        for n, old in saved.items():
            if old is None:
                sys.modules.pop(n, None)
            else:
                sys.modules[n] = old


config_flow = _load("config_flow")


class OptionsWithoutQuickTiles(unittest.TestCase):
    """The assistant mode's old quick tiles: not offered, not sent to the panel, dropped on the next save."""

    def test_the_flows_do_not_offer_them(self) -> None:
        # the assistant step of the options, and the panel's first form after its ID
        keys = {str(key) for key in config_flow._assistant_schema({"tile_entities": ["light.a"]}).schema}
        self.assertEqual(keys, {"weather_entity", "media_player_entity", "show_conversation"})

    def test_a_save_drops_stored_ones_and_keeps_the_rest(self) -> None:
        flow = config_flow.PanelOSOptionsFlow()
        flow.config_entry = types.SimpleNamespace(
            options={"tile_entities": ["light.a"], "weather_entity": "weather.before", "zoom": 90, "pages": [{"kind": "home"}]}
        )
        schema = config_flow._assistant_schema(dict(flow.config_entry.options))
        saved = flow._save(schema, {"media_player_entity": "media_player.a", "show_conversation": False})
        self.assertEqual(
            saved["data"],
            {"zoom": 90, "pages": [{"kind": "home"}], "media_player_entity": "media_player.a", "show_conversation": False},
        )

    def test_the_panel_does_not_get_them(self) -> None:
        entry = types.SimpleNamespace(
            domain="panelos", entry_id="entry", data={"panel_id": "nspanel-pro"}, options={"tile_entities": ["light.a"]}
        )
        config = panel.panel_config(entry)
        self.assertNotIn("tile_entities", config)
        self.assertEqual(config["panel_id"], "nspanel-pro")


if __name__ == "__main__":
    unittest.main()
