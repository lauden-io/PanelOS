#!/usr/bin/env python3
"""A stand-in panel for the PanelOS integration (development): it talks to Home Assistant as a panel of another name
would, so two panels can be tried with one real panel.

It logs in with a token, subscribes as the given panel (`panelos/subscribe`), reports a device with a made-up MAC
address and a few states (`panelos/report`), says it is there every second (`panelos/alive`), takes the commands of
its entities (and reports the new value back) and prints every event it gets: its settings, show requests (show_view,
notifications, alerts) and commands. Without an entry for the panel it asks again every 10 s, as a panel does, so the
PanelOS page offers its name under "Add a panel".

It has no ESPHome device: no voice satellite, no Home Assistant timers, no Panel user sensor.

    pip install websocket-client
    python3 tools/fake_panel.py --url http://<home assistant>:8123 --token-file <file with a long-lived token> \
        --panel-id kitchen [--mac 02:00:00:00:00:01] [--seconds 600]

A token of a user that is not an administrator tries the panel user binding too (the first such user that subscribes
for an entry becomes its user). Ctrl+C ends it; Home Assistant then shows the panel offline after its usual wait.
"""

from __future__ import annotations

import argparse
import json
import time

import websocket  # websocket-client


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--url", required=True, help="Home Assistant's address, e.g. http://192.168.1.10:8123")
    ap.add_argument("--token-file", required=True, help="a file holding a long-lived access token (never printed)")
    ap.add_argument("--panel-id", required=True, help="the stand-in panel's name, e.g. kitchen")
    ap.add_argument("--mac", default="02:00:00:00:00:01", help="a made-up MAC address (locally administered)")
    ap.add_argument("--seconds", type=float, default=0, help="end after this long (0: until Ctrl+C)")
    args = ap.parse_args()

    token = open(args.token_file, encoding="utf-8").read().strip()
    ws_url = args.url.rstrip("/").replace("https://", "wss://").replace("http://", "ws://") + "/api/websocket"
    ws = websocket.create_connection(ws_url, timeout=10)
    if json.loads(ws.recv()).get("type") != "auth_required":
        raise SystemExit("not Home Assistant's websocket")
    ws.send(json.dumps({"type": "auth", "access_token": token}))
    if json.loads(ws.recv()).get("type") != "auth_ok":
        raise SystemExit("login refused")

    next_id = 0

    def send(msg: dict) -> int:
        nonlocal next_id
        next_id += 1
        ws.send(json.dumps({"id": next_id, **msg}))
        return next_id

    pid = args.panel_id
    user_req = send({"type": "auth/current_user"})
    sub_req = send({"type": "panelos/subscribe", "panel_id": pid})
    states = {"screen": True, "brightness": 50, "auto_brightness": False, "theme": "Dark", "app_version": "fake"}
    device = {
        "name": f"NSPanel Pro ({pid})",
        "manufacturer": "PanelOS",
        "model": "NSPanel Pro 86 Gen1",
        "sw_version": "fake",
        "mac": args.mac.lower(),
        "caps": ["touch", "images", "web", "video", "mic", "speaker", "proximity", "light"],
    }
    linked = False
    reported = False  # the full report was accepted (an entry exists)
    report_req = 0
    retry_at: float | None = None
    alive_at = 0.0
    started = time.monotonic()
    ws.settimeout(0.2)
    print(f"fake panel '{pid}' ({args.mac}) connected; Ctrl+C ends it")
    try:
        while not args.seconds or time.monotonic() - started < args.seconds:
            now = time.monotonic()
            if retry_at is not None and now >= retry_at:
                retry_at = None
                sub_req = send({"type": "panelos/subscribe", "panel_id": pid})
            if linked and now - alive_at >= 1:
                alive_at = now
                send({"type": "panelos/alive", "panel_id": pid})
            try:
                msg = json.loads(ws.recv())
            except websocket.WebSocketTimeoutException:
                continue
            if msg.get("id") == user_req and msg.get("type") == "result":
                user = msg.get("result") or {}
                print(f"logged in as '{user.get('name')}' (administrator: {user.get('is_admin')})")
            elif msg.get("id") == sub_req and msg.get("type") == "result":
                if msg.get("success"):
                    linked = True
                    print("subscribed")
                else:
                    print(f"refused: {msg.get('error', {}).get('message')} (asking again in 10 s)")
                    linked, retry_at = False, time.monotonic() + 10
            elif msg.get("id") == sub_req and msg.get("type") == "event":
                event = msg.get("event") or {}
                if "config" in event:
                    config = event["config"]
                    if config:
                        pages = len(config.get("pages", []))
                        print(f"settings: panel_id={config.get('panel_id')}, {pages} pages, room={config.get('room')!r}")
                        if not reported:
                            # the device and the states, once there is an entry to take them
                            report_req = send({"type": "panelos/report", "panel_id": pid, "device": device, "states": states,
                                               "options": {"theme": ["Dark", "Light", "Auto"]}})
                    else:
                        print("settings: none (no entry for this panel)")
                for key in ("show", "timers"):
                    if key in event:
                        print(f"{key}: {json.dumps(event[key], ensure_ascii=False)[:300]}")
                if "command" in event:
                    command = event["command"]
                    print(f"command: {command}")
                    if command.get("key") in states:
                        states[command["key"]] = command.get("value")
                        send({"type": "panelos/report", "panel_id": pid, "states": {command["key"]: command.get("value")}})
            elif msg.get("id") == report_req and msg.get("type") == "result":
                reported = bool(msg.get("success"))
                print("device reported" if reported else f"report refused: {msg.get('error')}")
            elif msg.get("type") == "result" and not msg.get("success"):
                print(f"request {msg.get('id')} failed: {msg.get('error')}")
    except KeyboardInterrupt:
        pass
    finally:
        ws.close()
        print("fake panel gone")


if __name__ == "__main__":
    main()
