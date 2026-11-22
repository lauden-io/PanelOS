"""Home Assistant's own IP addresses, from its network adapters (no Home Assistant imports, so the tests load it on its
own; `__init__.py` asks Home Assistant's network integration for the adapters).

A panel whose Home Assistant address is changed asks for them on its old connection: a new address among them is the
same Home Assistant, so the panel keeps its login.
"""

from __future__ import annotations

import ipaddress
from typing import Any


def own_addresses(adapters: Any) -> list[str]:
    """The IPv4 and IPv6 addresses of the enabled adapters (the ones Home Assistant itself uses on the network), each
    once, in the adapters' order. Loopback and link-local addresses are left out: from a panel they do not name this
    machine (127.0.0.1 is the panel itself, an fe80:: address is whoever holds it on the panel's own link)."""
    found: list[str] = []
    for adapter in adapters if isinstance(adapters, list) else []:
        if not isinstance(adapter, dict) or adapter.get("enabled") is not True:
            continue
        for kind in ("ipv4", "ipv6"):
            configured = adapter.get(kind)
            for item in configured if isinstance(configured, list) else []:
                text = item.get("address") if isinstance(item, dict) else None
                try:
                    # (an IPv6 address without its zone: the panel writes none)
                    address = ipaddress.ip_address(text.split("%", 1)[0]) if isinstance(text, str) else None
                except ValueError:
                    address = None
                if address is None or address.is_loopback or address.is_link_local or address.is_unspecified or address.is_multicast:
                    continue
                if (written := str(address)) not in found:
                    found.append(written)
    return found
