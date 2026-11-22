"""Which ESPHome entry is a panel's own (its voice satellite), and which MAC address the panel's entry keeps (no Home
Assistant imports, so the tests load it on its own; `panel.py` carries it out).

The panel's ESPHome entry carries its "Panel user" sensor, and that sensor decides who may act as the panel. So what a
connection sends never picks the entry on its own: only what Home Assistant itself holds does (the ESPHome entries an
administrator added, whether it keeps their API key, the states their devices send over the encrypted connection),
and a MAC address that an administrator reported or that the panel's own ESPHome device confirmed. A device name is
not enough once that MAC address is kept: Home Assistant's ESPHome integration takes a new name from the device
itself, so any device added with a key could take the panel's name.
"""

from __future__ import annotations

from dataclasses import dataclass
from typing import Any


def plain_mac(value: Any) -> str | None:
    """A MAC address as 12 lower-case hex digits; None for anything else."""
    if not isinstance(value, str):
        return None
    plain = value.replace(":", "").replace("-", "").lower()
    return plain if len(plain) == 12 and all(c in "0123456789abcdef" for c in plain) else None


def written_mac(plain: str) -> str:
    """A plain MAC address as the panel writes it: aa:bb:cc:dd:ee:ff."""
    return ":".join(plain[i : i + 2] for i in range(0, 12, 2))


@dataclass(frozen=True)
class Candidate:
    """An ESPHome entry whose device name is the panel's ID."""

    # its MAC address (its unique id; None when that is not a MAC address)
    mac: str | None
    # Home Assistant keeps its API key: its states come over the encrypted connection that only the device can answer
    keyed: bool
    # the id of the Home Assistant user its Panel user sensor shows (None: no such sensor, or it shows none)
    user: str | None

    @property
    def named(self) -> str | None:
        """The user its device names: only an entry with a key counts (over Home Assistant's API only administrators
        can set an entity's state)."""
        return self.user if self.keyed else None


def pick(candidates: list[Candidate], known_mac: str | None) -> int | None:
    """The panel's ESPHome entry among the entries with its ID as their device name (an index), None for none.

    While the entry of the MAC address the panel's entry keeps is there, it alone is the panel's, whatever the others
    name. Without one (nothing kept yet, or that entry deleted: a panel that was replaced) an entry whose device names
    its user comes first, then one that fits the kept MAC address (an entry without a MAC address as its unique id fits
    any), then the first in Home Assistant's order; while none names a user, an entry of another MAC address is never
    taken."""
    known = plain_mac(known_mac)
    if known is not None and (own := next((i for i, c in enumerate(candidates) if c.mac == known), None)) is not None:
        return own

    def fits(i: int) -> bool:
        return known is None or candidates[i].mac is None

    naming = [i for i, c in enumerate(candidates) if c.named]
    if naming:
        return next((i for i in naming if fits(i)), naming[0])
    return next((i for i in range(len(candidates)) if fits(i)), None)


def confirmed(candidates: list[Candidate], mac: Any, user_id: str | None) -> bool:
    """Whether the panel's ESPHome device confirms a reported MAC address: an entry with the panel's ID and that MAC
    address has a key, and its device names this user (the user of the connection that reported it)."""
    wanted = plain_mac(mac)
    return wanted is not None and bool(user_id) and any(c.mac == wanted and c.named == user_id for c in candidates)


def kept_mac(stored: str | None, reported: Any, user_id: str | None, is_admin: bool, candidates: list[Candidate]) -> str | None:
    """The MAC address the panel's entry keeps after a device report: the reported one when an administrator reported
    it, or when the panel's ESPHome device confirms it and no entry of the kept one is there any more (nothing kept
    yet, or the old panel's entry deleted); else the one kept before (also for a report without one). So a user that
    has not been shown to be the panel neither sets it nor takes it away, and a user kept only for connecting first
    cannot move it to a device of its own."""
    plain = plain_mac(reported)
    if plain is None:
        return stored
    if is_admin:
        return written_mac(plain)
    kept = plain_mac(stored)
    if kept is not None and kept != plain and any(c.mac == kept for c in candidates):
        return stored
    return written_mac(plain) if confirmed(candidates, plain, user_id) else stored
