"""What Home Assistant's own wake word selects of a panel's voice device should show (no Home Assistant imports, so
the tests load it on its own; `wake_words.py` carries it out).

Home Assistant's ESPHome selects ("Wake word", "Wake word 2") keep the option they had and take a device's wake word
only when it has exactly one. With two (set on the panel or on the PanelOS page) they stay at "No wake word", and
picking a word in one of them then sends the panel only the words the selects show, so the other word stops working.
"""

from __future__ import annotations

from dataclasses import dataclass, field

NO_WAKE_WORD = "no_wake_word"


@dataclass
class Plan:
    """The panel's words as Home Assistant should know them, and the selects to set (index, option), the first first."""

    target: list[str]
    push: bool = False
    selects: list[tuple[int, str]] = field(default_factory=list)

    @property
    def nothing(self) -> bool:
        return not self.push and not self.selects


def plan(
    reported: list[str] | None,
    active: list[str],
    names: dict[str, str],
    max_active: int,
    shown: list[str],
) -> Plan:
    """`reported`: the ids the panel says it listens for (None: an older panel, which does not say; Home Assistant's
    own configuration counts then). `active`: the ids in Home Assistant's configuration of the satellite. `names`: id →
    name of the wake words the satellite offers. `shown`: what the selects show now, in their order. `push`: Home
    Assistant's configuration has to take the panel's words first (a change made on the panel itself)."""
    source = reported if reported is not None else active
    limit = max(0, min(len(shown), max_active))
    target: list[str] = []
    for word in source:
        if word in names and word not in target and len(target) < limit:
            target.append(word)
    wanted = [names[target[k]] if k < len(target) else NO_WAKE_WORD for k in range(len(shown))]
    return Plan(
        target=target,
        push=list(active) != target,
        selects=[(k, option) for k, option in enumerate(wanted) if shown[k] != option],
    )
