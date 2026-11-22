"""Home Assistant's own IP addresses for panelos/ha_addresses (custom_components/panelos/addresses.py).

Runs with the standard library only: python3 -m unittest discover -s tests

addresses.py imports nothing from Home Assistant, so it is loaded by its path, outside the package. The adapters are
written as Home Assistant's network integration gives them (its Adapter, IPv4ConfiguredAddress and
IPv6ConfiguredAddress); the addresses are documentation ranges.
"""

from __future__ import annotations

import importlib.util
from pathlib import Path
import sys
import unittest

ADDRESSES = Path(__file__).resolve().parents[1] / "custom_components" / "panelos" / "addresses.py"


def _load():
    spec = importlib.util.spec_from_file_location("panelos_addresses_under_test", ADDRESSES)
    module = importlib.util.module_from_spec(spec)
    write_bytecode = sys.dont_write_bytecode
    try:
        sys.dont_write_bytecode = True
        sys.modules[spec.name] = module
        spec.loader.exec_module(module)
    finally:
        sys.dont_write_bytecode = write_bytecode
    return module


m = _load()


def adapter(name: str, enabled: bool, ipv4: list[str], ipv6: list[str] = ()) -> dict:
    return {
        "name": name,
        "index": 1,
        "enabled": enabled,
        "auto": True,
        "default": enabled,
        "ipv4": [{"address": a, "network_prefix": 24} for a in ipv4],
        "ipv6": [{"address": a, "flowinfo": 0, "scope_id": 2, "network_prefix": 64} for a in ipv6],
    }


class OwnAddresses(unittest.TestCase):
    def test_enabled_adapters_ipv4_and_ipv6(self) -> None:
        adapters = [
            adapter("eth0", True, ["192.0.2.10"], ["2001:db8::10", "fe80::1"]),
            adapter("docker0", False, ["198.51.100.1"]),
            adapter("wlan0", True, ["203.0.113.7"]),
        ]
        self.assertEqual(m.own_addresses(adapters), ["192.0.2.10", "2001:db8::10", "203.0.113.7"])

    def test_loopback_link_local_and_odd_ones_left_out(self) -> None:
        adapters = [
            adapter("lo", True, ["127.0.0.1"], ["::1"]),
            adapter("eth0", True, ["169.254.3.4", "0.0.0.0", "not an address", "192.0.2.10"], ["fe80::1", "2001:DB8::A"]),
        ]
        self.assertEqual(m.own_addresses(adapters), ["192.0.2.10", "2001:db8::a"])

    def test_each_once(self) -> None:
        adapters = [adapter("eth0", True, ["192.0.2.10"]), adapter("br0", True, ["192.0.2.10"])]
        self.assertEqual(m.own_addresses(adapters), ["192.0.2.10"])

    def test_odd_input(self) -> None:
        for adapters in (None, {}, [None], [{"enabled": "yes", "ipv4": [{"address": "192.0.2.1"}]}], [{"enabled": True, "ipv4": None}]):
            with self.subTest(adapters=adapters):
                self.assertEqual(m.own_addresses(adapters), [])


if __name__ == "__main__":
    unittest.main()
