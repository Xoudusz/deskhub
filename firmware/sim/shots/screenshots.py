#!/usr/bin/env python3
"""README screenshots and the tour animation, made with the simulator (the firmware's own UI).

Needs the simulator built (bash firmware/sim/build.sh) and, in any Python env:
    pip install playwright pillow && playwright install chromium
Run from anywhere:
    python3 firmware/sim/shots/screenshots.py
Writes docs/screenshots/<name>.png (800x480) and docs/tour.gif.

All data below is made up. The browser clock is fixed (NOW), so the output only changes
when the firmware's UI does.
"""
import base64
import functools
import http.server
import io
import json
import math
import struct
import sys
import threading
from datetime import datetime, timedelta
from pathlib import Path
from zoneinfo import ZoneInfo

from PIL import Image, ImageDraw, ImageFilter
from playwright.sync_api import sync_playwright

ROOT = Path(__file__).resolve().parents[3]
OUT = ROOT / "docs" / "screenshots"
GIF = ROOT / "docs" / "tour.gif"
TZ = ZoneInfo("Europe/Vienna")
NOW_DT = datetime(2026, 11, 26, 10, 24, 0, tzinfo=TZ)
NOW = int(NOW_DT.timestamp())

# PAGE_* order (firmware/main/ui_internal.h).
PAGES = ["home", "stats", "ai", "calendar", "weather", "media", "printer", "shortcuts", "timer",
         "games", "network", "github", "plex"]


def at(days=0, hour=None, minute=0):
    """Unix time `days` from NOW, at hour:minute local time (or NOW's time)."""
    d = NOW_DT + timedelta(days=days)
    if hour is not None:
        d = d.replace(hour=hour, minute=minute, second=0)
    return int(d.timestamp())


def midnight(days=0):
    return at(days, 0, 0)


# ---- made-up pictures (RGB565 frames) -------------------------------------------------------

def art(w, h, c1, c2, seed):
    """An abstract cover: a diagonal gradient with soft circles."""
    img = Image.new("RGB", (w, h))
    px = img.load()
    for y in range(h):
        for x in range(w):
            t = (x / w + y / h) / 2
            px[x, y] = tuple(int(a + (b - a) * t) for a, b in zip(c1, c2))
    d = ImageDraw.Draw(img, "RGBA")
    for i in range(4):
        r = int(min(w, h) * (0.18 + 0.1 * ((seed + i) % 3)))
        cx = int(w * (0.2 + 0.6 * ((seed * 7 + i * 3) % 10) / 10))
        cy = int(h * (0.2 + 0.6 * ((seed * 3 + i * 5) % 10) / 10))
        d.ellipse((cx - r, cy - r, cx + r, cy + r), fill=(255, 255, 255, 40 + 15 * i))
    return img.filter(ImageFilter.GaussianBlur(min(w, h) / 60))


def rgb565(img):
    out = bytearray()
    raw = img.tobytes()
    for r, g, b in zip(raw[0::3], raw[1::3], raw[2::3]):
        out += struct.pack("<H", ((r >> 3) << 11) | ((g >> 2) << 5) | (b >> 3))
    return bytes(out)


def cover(magic, art_id, img):
    w, h = img.size
    return {"bin": base64.b64encode(magic + struct.pack("<IHH", art_id, w, h) + rgb565(img)).decode()}


def steam_art(appid, kind, img):
    w, h = img.size
    return {"bin": base64.b64encode(b"STM1" + struct.pack("<IHHHH", appid, w, h, kind, 0) + rgb565(img)).decode()}


# ---- sample board messages ------------------------------------------------------------------

def github():
    days = 371
    first = NOW_DT.date() - timedelta(days=days - 1)
    start_wday = (first.weekday() + 1) % 7   # 0 = Sunday, as GitHub's graph
    levels = ""
    for i in range(days):
        d = first + timedelta(days=i)
        v = (math.sin(i / 9) + math.sin(i / 3.7) + (i % 11) / 5) * 1.3
        if d.weekday() >= 5:
            v -= 1.5
        levels += str(max(0, min(4, int(v + 1.2))))
    months, seen = [], set()
    for i in range(days):
        d = first + timedelta(days=i)
        if d.day == 1 and (d.year, d.month) not in seen:
            seen.add((d.year, d.month))
            months.append([(start_wday + i) // 7, d.strftime("%b")])
    return {"type": "github", "accent": True, "login": "octo-dev", "total": 1873, "today": 6, "week": 31,
            "last_week": 24, "streak": 17, "best": 21, "longest": 46, "best_date": "Mar 12",
            "start_wday": start_wday, "levels": levels, "months": months,
            "week_days": [3, 7, 5, 6, 4, 0, 0], "last_week_days": [4, 5, 2, 6, 3, 2, 2],
            "kinds": {"commits": 1520, "prs": 188},
            "repos": [{"name": "desk-widgets", "count": 412}, {"name": "garden-api", "count": 236},
                      {"name": "dotfiles", "count": 118}, {"name": "recipe-box", "count": 77}]}


def weather_body():
    hours = [at(0, 10) + 3600 * i for i in range(24)]
    temps = [4 + 3 * math.sin((i - 3) / 24 * 2 * math.pi) for i in range(24)]
    codes = [2, 2, 3, 3, 61, 61, 3, 2, 1, 0, 0, 0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 3, 2, 2]
    precip = [5, 10, 20, 35, 60, 55, 30, 10, 5, 0, 0, 0, 0, 0, 0, 0, 5, 5, 10, 10, 15, 15, 10, 5]
    return json.dumps({
        "current": {"temperature_2m": 5.8, "apparent_temperature": 2.9, "relative_humidity_2m": 71,
                    "weather_code": 2, "is_day": 1, "wind_speed_10m": 14},
        "hourly": {"time": hours, "temperature_2m": temps, "weather_code": codes,
                   "precipitation_probability": precip},
        "daily": {"time": [midnight(d) for d in range(6)],
                  "weather_code": [2, 61, 3, 0, 71, 1],
                  "temperature_2m_max": [7.1, 6.2, 5.0, 8.4, 2.3, 4.9],
                  "temperature_2m_min": [1.2, 2.4, 0.3, -1.8, -3.5, -2.0],
                  "precipitation_probability_max": [60, 85, 25, 5, 70, 10]},
    })


def frames():
    j = lambda m: {"json": m}
    art_media = art(280, 280, (40, 30, 90), (230, 90, 120), 1)
    art_plex = art(280, 280, (10, 60, 70), (230, 180, 60), 4)
    games = [(9001, "Starfall Tactics", (30, 40, 110), (240, 120, 60)),
             (9002, "Hollow Pines", (20, 70, 40), (190, 220, 140)),
             (9003, "Neon Drift", (90, 20, 100), (40, 200, 230)),
             (9004, "Tidecaller", (10, 50, 90), (120, 210, 220))]
    out = [
        j({"type": "location", "city": "Vienna", "lat": "48.2082", "lon": "16.3738",
           "tz": "CET-1CEST,M3.5.0,M10.5.0/3", "fahrenheit": False, "h24": True}),
        j({"type": "layout", "pages": PAGES, "ordered": True, "nav": "bottom", "seconds": False}),
        j({"type": "home", "header": True, "grid": [
            {"widget": "claude", "x": 0, "y": 0, "w": 3, "h": 1},
            {"widget": "events", "x": 0, "y": 1, "w": 3, "h": 2},
            {"widget": "weather", "x": 3, "y": 0, "w": 2, "h": 1},
            {"widget": "media", "x": 3, "y": 1, "w": 2, "h": 1},
            {"widget": "stats", "x": 3, "y": 2, "w": 2, "h": 1}]}),
        j({"type": "ai", "provider": "claude",
           "limits": {"five_hour": {"pct": 42, "reset": NOW + 5400}, "seven_day": {"pct": 28, "reset": at(3, 9)},
                      "updated": NOW - 60},
           "block": {"active": True, "reset": NOW + 5400, "tokens": 182000, "messages": 23},
           "today": {"tokens": 640000, "output": 52000, "messages": 88}, "last_hour": 41000,
           "days": [12, 30, 22, 41, 18, 9, 33], "last_active": NOW - 120,
           "models": [{"name": "Opus", "tokens": 410000}, {"name": "Sonnet", "tokens": 190000},
                      {"name": "Haiku", "tokens": 40000}]}),
        j({"type": "events", "days": 7, "events": [
            {"start": at(0, 9, 30), "end": at(0, 9, 45), "title": "Team stand-up", "cal": "Work", "color": "#7c9cff"},
            {"start": at(0, 12, 30), "end": at(0, 13, 30), "title": "Lunch with Zoë", "loc": "Café Grünwald",
             "cal": "Personal", "color": "#4ade80"},
            {"start": at(0, 15), "end": at(0, 16), "title": "Design review – new dashboard", "loc": "Room 2.14",
             "cal": "Work", "color": "#7c9cff"},
            {"start": midnight(1), "end": midnight(2), "all_day": True, "title": "Jürgen's birthday",
             "cal": "Family", "color": "#f472b6"},
            {"start": at(1, 8), "end": at(1, 8, 45), "title": "Dentist check-up", "loc": "Hauptstraße 12",
             "cal": "Personal", "color": "#4ade80"},
            {"start": at(2, 18, 30), "end": at(2, 20), "title": "Climbing", "loc": "Boulderhalle Süd",
             "cal": "Personal", "color": "#4ade80"},
            {"start": at(4, 10), "end": at(4, 11), "title": "Sprint planning", "cal": "Work", "color": "#7c9cff"}]}),
        j({"type": "media", "active": True, "playing": True, "title": "Glass Harbour", "artist": "The Night Arcade",
           "album": "Low Tide Lights", "source": "Spotify", "pos": 95, "dur": 243, "can_seek": True, "art_id": 1}),
        cover(b"ART1", 1, art_media),
        j({"type": "media_server", "configured": True, "name": "Home", "kind": "plex", "online": True,
           "version": "Plex Media Server 1.41.2", "library": "312 movies, 45 shows, 2,104 episodes",
           "streams": 2, "transcoding": 1, "active": True, "playing": True, "title": "The Long Orbit",
           "artist": "Season 1 - Episode 2", "album": "Living Room TV - sam (+1 more)", "source": "Home",
           "pos": 1260, "dur": 2880, "can_seek": False, "controls": False, "art_id": 2}),
        cover(b"ARS1", 2, art_plex),
        j({"type": "printer", "printers": [{
            "online": True, "name": "Workshop", "model": "X1 Carbon", "state": "RUNNING",
            "file": "desk_cable_clip_v3.3mf", "speed": "standard", "pct": 64, "remaining_min": 38,
            "layer": 112, "layers": 176, "nozzle": 219, "nozzle_target": 220, "bed": 55, "bed_target": 55,
            "chamber": 31, "ams": [{"color": "#e8e8e8", "type": "PLA", "active": False},
                                   {"color": "#1f1f1f", "type": "PLA", "active": True},
                                   {"color": "#3b82f6", "type": "PETG", "active": False},
                                   {"color": "#f97316", "type": "PLA", "active": False}]}]}),
        j({"type": "tiles", "tiles": [
            {"label": "Mute mic", "icon": "mic_off", "color": "#b4443c"},
            {"label": "Play / pause", "icon": "play_arrow", "color": "#2e7d5b"},
            {"label": "Screenshot", "icon": "screenshot_monitor", "color": "#2f6db5"},
            {"label": "Terminal", "icon": "terminal", "color": "#3a3f4b"},
            {"label": "VS Code", "icon": "code", "color": "#1f6fb2"},
            {"label": "Browser", "icon": "public", "color": "#c2702a"},
            {"label": "Dark mode", "icon": "dark_mode", "color": "#4b3f8f"},
            {"label": "Lock PC", "icon": "lock", "color": "#5b3fa0"}]}),
        j({"type": "steam", "found": True,
           "playing": {"appid": 9001, "name": "Starfall Tactics", "session_s": 2760, "total_min": 4210, "last": NOW},
           "recent": [{"appid": a, "name": n, "total_min": 600 + 900 * i, "last": NOW - 86400 * (i + 1)}
                      for i, (a, n, _, _) in enumerate(games[1:])],
           "friends": [{"name": "pixelfox", "game": "Neon Drift"}, {"name": "mara_k", "game": "Tidecaller"},
                       {"name": "Benedikt", "game": ""}]}),
        j({"type": "net", "online": True, "latency_ms": 11.4, "jitter_ms": 1.8, "loss_pct": 0,
           "hist": [10 + (i * 7) % 6 + (25 if i in (101, 102) else 0) for i in range(144)],
           "hist_label": "LAST 24 HOURS",
           "up_since": NOW - 3 * 86400 - 7200, "last_drop": {"at": NOW - 3 * 86400 - 7200, "dur_s": 42},
           "today_bytes": 6.3e9, "month_bytes": 412e9,
           "speed": {"down_mbps": 482.6, "up_mbps": 51.3, "ping_ms": 9.7, "at": NOW - 5400},
           "testing": False, "ip": "203.0.113.42"}),
        j(github()),
    ]
    # PC stats with a saved 24 h history (144 points of 10 minutes, as the app sends it).
    out.append(j({"type": "stats", "cpu": 23, "cpu_temp": 54, "gpu": 41, "gpu_temp": 49,
                  "ram_used": 14.6, "ram_total": 32, "vram_used": 5.2, "vram_total": 16,
                  "net_down": 9000, "net_up": 420, "disk": 61,
                  "cpu_name": "Ryzen 7 7800X3D", "gpu_name": "Radeon RX 7900 XT",
                  "cpu_hist": [int(12 + 30 * max(0, math.sin((i - 40) / 25)) + 6 * math.sin(i / 3)) for i in range(144)],
                  "gpu_hist": [int(8 + 55 * max(0, math.sin((i - 90) / 12))) for i in range(144)],
                  "hist_label": "LAST 24 HOURS"}))
    # Header and thumbnail: the game being played (one slot each); portraits: recent games.
    appid, _, c1, c2 = games[0]
    out.append(steam_art(appid, 0, art(340, 159, c1, c2, 0)))
    out.append(steam_art(appid, 1, art(64, 96, c1, c2, 0)))
    for i, (appid, _, c1, c2) in enumerate(games[1:], 1):
        out.append(steam_art(appid, 2, art(96, 144, c1, c2, i)))
    return out


ALERT = {"type": "alert", "id": "shot-1", "kind": "permission", "agent": "claude", "project": "desk-widgets",
         "title": "Permission needed", "detail": "Bash: npm test", "decide": True}
CODEX = {"type": "ai", "provider": "codex",
         "limits": {"five_hour": {"pct": 17, "reset": NOW + 9000}, "seven_day": {"pct": 35, "reset": at(5, 14)},
                    "updated": NOW - 90},
         "today": {"tokens": 310000, "output": 21000, "messages": 37}, "last_hour": 12000,
         "days": [8, 14, 0, 22, 19, 5, 11],
         "models": [{"name": "gpt-5-codex", "tokens": 250000}, {"name": "gpt-5", "tokens": 60000}],
         "last_active": NOW - 600}

# Screenshot name -> page shown. The tour animation uses the same order.
SHOTS = ["home", "stats", "ai", "calendar", "weather", "media", "plex", "shortcuts", "games", "network",
         "github", "timer", "printer"]


def serve():
    class Quiet(http.server.SimpleHTTPRequestHandler):
        def log_message(self, *_):
            pass

    handler = functools.partial(Quiet, directory=str(ROOT))
    srv = http.server.ThreadingHTTPServer(("127.0.0.1", 0), handler)
    threading.Thread(target=srv.serve_forever, daemon=True).start()
    return srv


def png(page):
    return Image.open(io.BytesIO(page.locator("#c").screenshot())).convert("RGB")


def main():
    if not (ROOT / "desktop/public/sim/deskhub_sim.js").exists():
        sys.exit("simulator missing: run bash firmware/sim/build.sh first")
    OUT.mkdir(parents=True, exist_ok=True)
    srv = serve()
    shots = {}
    with sync_playwright() as p:
        browser = p.chromium.launch()
        page = browser.new_page(viewport={"width": 800, "height": 480}, timezone_id="Europe/Vienna",
                                locale="en-GB")
        page.on("console", lambda m: m.type in ("error", "warning") and print("page:", m.text))
        page.clock.set_fixed_time(NOW_DT)
        page.goto(f"http://127.0.0.1:{srv.server_port}/firmware/sim/shots/shots.html")
        page.wait_for_function("!!(window.shots && window.createDeskhubSim)")
        page.evaluate("f => shots.boot(f)", frames())
        if not page.evaluate("b => shots.weather(b)", weather_body()):
            sys.exit("weather sample rejected")

        def shoot(name, page_name):
            page.evaluate("i => shots.show(i)", PAGES.index(page_name))
            page.evaluate("shots.settle(900)")
            shots[name] = png(page)

        for name in SHOTS:
            shoot(name, name)
        page.evaluate("f => shots.feed(f)", [{"json": ALERT}])
        shoot("alert", "home")
        page.evaluate("f => shots.feed(f)", [{"json": {"type": "alert", "clear": True}}, {"json": CODEX}])
        shoot("codex", "ai")
        browser.close()
    srv.shutdown()

    for name, img in shots.items():
        img.save(OUT / f"{name}.png", optimize=True)
    tour = [shots[n] for n in ["home", "alert", "stats", "ai", "calendar", "weather", "media", "plex",
                               "shortcuts", "games", "network", "github"]]
    pal = [im.quantize(colors=256, method=Image.Quantize.MEDIANCUT, dither=Image.Dither.NONE) for im in tour]
    pal[0].save(GIF, save_all=True, append_images=pal[1:], duration=1800, loop=0, optimize=True)
    print(f"{len(shots)} screenshots in {OUT}, tour {GIF.stat().st_size // 1024} KB")


if __name__ == "__main__":
    main()
