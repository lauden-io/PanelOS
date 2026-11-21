"""Home Assistant's time zone for the panels, as the POSIX TZ rule in the footer of the zone's TZif file.

A panel has no zone database. With the rule (``CET-1CEST,M3.5.0,M10.5.0/3`` for Europe/Berlin, ``EST5EDT,M3.2.0,M11.1.0``
for America/New_York) its C library works out the local time of any date, daylight saving time included. The panels get
it with their settings as ``time_zone``: ``{"name": "Europe/Berlin", "posix": "CET-1CEST,M3.5.0,M10.5.0/3"}``.

The footer describes the times after the file's last transition. A few zones list their changes years ahead because no
rule describes them (Morocco's Ramadan changes): there the footer's offsets miss the offset in effect now, and the panels
get that offset as a fixed rule instead (``<+01>-1``); Home Assistant looks again every hour.

Standard library only (the tests run without Home Assistant). Reading the file is blocking: Home Assistant calls
`time_zone` in its executor.
"""

from __future__ import annotations

from datetime import datetime, timezone
from importlib import resources
import os
import re
import struct
import zoneinfo

# magic, version, 15 reserved bytes, then the counts: UT/local indicators, standard/wall indicators, leap seconds,
# transition times, local time types, characters of the abbreviations
_HEADER = struct.Struct(">4sc15x6L")
# a zone name: parts of letters, digits, _ + and -, joined with /
_NAME = re.compile(r"[A-Za-z0-9_+-]+(?:/[A-Za-z0-9_+-]+)*")
# the start of a POSIX TZ rule: standard time's name and offset, daylight time's name and offset (both optional)
_ZONE = r"(?:<[A-Za-z0-9+-]{3,}>|[A-Za-z]{3,})"
_OFFSET = r"[+-]?\d{1,2}(?::\d{1,2}){0,2}"
_RULE = re.compile(rf"{_ZONE}(?P<std>{_OFFSET})(?:{_ZONE}(?P<dst>{_OFFSET})?)?(?P<dated>,.*)?")


def footer_rule(data: bytes) -> str | None:
    """The POSIX TZ rule in the footer of a TZif file (version 2 or later); None when the file has no rule
    (an empty footer, a version 1 file) or is not a TZif file."""
    if len(data) < _HEADER.size:
        return None
    magic, version, isut, isstd, leap, times, types, chars = _HEADER.unpack_from(data)
    if magic != b"TZif" or version < b"2":
        return None
    # the version 1 data block (32-bit times), then the second header and its block (64-bit times)
    pos = _HEADER.size + times * 5 + types * 6 + chars + leap * 8 + isstd + isut
    if len(data) < pos + _HEADER.size:
        return None
    magic, _version, isut, isstd, leap, times, types, chars = _HEADER.unpack_from(data, pos)
    if magic != b"TZif":
        return None
    pos += _HEADER.size + times * 9 + types * 6 + chars + leap * 12 + isstd + isut
    # the footer: a newline, the rule, a newline
    if data[pos : pos + 1] != b"\n":
        return None
    end = data.find(b"\n", pos + 1)
    if end < 0:
        return None
    try:
        rule = data[pos + 1 : end].decode("ascii")
    except UnicodeDecodeError:
        return None
    return rule if rule and rule.isprintable() and " " not in rule else None


def _seconds(text: str) -> int:
    """``[+|-]hh[:mm[:ss]]`` in seconds."""
    sign = -1 if text.startswith("-") else 1
    parts = [int(p) for p in text.lstrip("+-").split(":")] + [0, 0]
    return sign * (parts[0] * 3600 + parts[1] * 60 + parts[2])


def rule_offsets(rule: str) -> tuple[int, ...]:
    """The UTC offsets (seconds east, as Python counts them) a POSIX TZ rule can give: standard time's, and daylight
    time's (an hour ahead when the rule names none); () for a rule that cannot be read."""
    m = _RULE.fullmatch(rule)
    if m is None:
        return ()
    # POSIX counts west of Greenwich as positive
    std = -_seconds(m["std"])
    if m["dst"] is not None:
        return (std, -_seconds(m["dst"]))
    return (std, std + 3600) if m.end("std") < len(rule) else (std,)


def fixed_rule(offset: int, abbreviation: str | None = None) -> str:
    """A POSIX TZ rule for one UTC offset (seconds east): ``IST-5:30``, ``<+01>-1``, ``<-0330>3:30``."""
    a = abs(offset)
    h, m, s = a // 3600, a % 3600 // 60, a % 60
    if abbreviation and re.fullmatch(r"[A-Za-z]{3,}", abbreviation):
        name = abbreviation
    elif abbreviation and re.fullmatch(r"[A-Za-z0-9+-]{3,}", abbreviation):
        name = f"<{abbreviation}>"
    else:
        name = f"<{'-' if offset < 0 else '+'}{h:02d}{f'{m:02d}' if m or s else ''}{f'{s:02d}' if s else ''}>"
    value = f"{h}{f':{m:02d}' if m or s else ''}{f':{s:02d}' if s else ''}"
    return f"{name}{'' if offset < 0 or a == 0 else '-'}{value}"


def pick_rule(footer: str | None, offset: int | None, abbreviation: str | None = None) -> str | None:
    """The rule the panels get: the footer's, unless it cannot give the offset in effect now (`offset`, seconds east;
    None when not known): then a fixed rule for that offset."""
    if offset is None:
        return footer
    if footer is not None and offset in rule_offsets(footer):
        return footer
    return fixed_rule(offset, abbreviation)


def zone_file(name: str) -> bytes | None:
    """The TZif file of a zone where Python's zoneinfo finds it: the system's zone folders, then the tzdata package."""
    if not _NAME.fullmatch(name):
        return None
    parts = name.split("/")
    for root in zoneinfo.TZPATH:
        path = os.path.join(root, *parts)
        if os.path.isfile(path):
            try:
                with open(path, "rb") as file:
                    return file.read()
            except OSError:
                continue
    try:
        return resources.files(".".join(["tzdata", "zoneinfo", *parts[:-1]])).joinpath(parts[-1]).read_bytes()
    except (ImportError, OSError, ValueError):
        return None


def _now_in(name: str, now: datetime | None) -> tuple[int, str | None] | None:
    """The UTC offset (seconds east) and abbreviation in effect in a zone at `now` (now when None)."""
    try:
        moment = (now or datetime.now(timezone.utc)).astimezone(zoneinfo.ZoneInfo(name))
    except (zoneinfo.ZoneInfoNotFoundError, ValueError, OSError):
        return None
    offset = moment.utcoffset()
    return (int(offset.total_seconds()), moment.tzname()) if offset is not None else None


def zone_message(name: str, rule: str | None) -> dict[str, str] | None:
    """What the panels get as ``time_zone``; None without a rule (the panels keep the zone they have)."""
    return {"name": name, "posix": rule} if rule else None


def time_zone(name: str, now: datetime | None = None) -> dict[str, str] | None:
    """The ``time_zone`` of a zone (Home Assistant's ``time_zone`` setting) at `now` (now when None)."""
    if not _NAME.fullmatch(name):
        return None
    data = zone_file(name)
    current = _now_in(name, now)
    rule = pick_rule(footer_rule(data) if data else None, *(current or (None, None)))
    return zone_message(name, rule)
