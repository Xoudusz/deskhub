"""Screen brightness: [display] brightness / night_brightness defaults, clamping,
garbage values, and the setup prompts that write them.

    uv run python tests/test_brightness.py
"""
import builtins
import pathlib
import shutil
import tempfile
import tomllib

from deskhub import setup_cli as s
import deskhub.pages as _pg
from deskhub.pages import brightness, layout_message, night_brightness

_pg.steam_installed = lambda: False

problems = []


def check(cond, msg):
    if not cond:
        problems.append(msg)


# Defaults: full brightness by day, dark at night (the old behaviour).
check(brightness({}) == 100 and night_brightness({}) == 0, "defaults 100 / 0")
check(brightness({"display": {}}) == 100 and night_brightness({"display": {}}) == 0, "defaults with empty [display]")

# Clamping: out-of-range ints are pulled into range.
check(brightness({"display": {"brightness": 150}}) == 100, "day 150 -> 100")
check(brightness({"display": {"brightness": -5}}) == 1, "day -5 -> 1")
check(night_brightness({"display": {"night_brightness": 150}}) == 100, "night 150 -> 100")
check(night_brightness({"display": {"night_brightness": -5}}) == 0, "night -5 -> 0")

# Garbage (not an int) falls back to the default.
check(brightness({"display": {"brightness": "bright"}}) == 100, "day garbage string -> 100")
check(brightness({"display": {"brightness": "50"}}) == 100, "day numeric string -> 100")
check(brightness({"display": {"brightness": 50.5}}) == 100, "day float -> 100")
check(brightness({"display": {"brightness": True}}) == 100, "day bool -> 100")
check(night_brightness({"display": {"night_brightness": "dim"}}) == 0, "night garbage string -> 0")

# Valid values pass through, and layout_message always carries both keys.
check(brightness({"display": {"brightness": 40}}) == 40, "day 40 kept")
check(night_brightness({"display": {"night_brightness": 15}}) == 15, "night 15 kept")
msg = layout_message({})
check(msg["brightness"] == 100 and msg["night_brightness"] == 0, f"layout defaults: {msg}")
msg = layout_message({"display": {"brightness": 30, "night_brightness": 200}})
check(msg["brightness"] == 30 and msg["night_brightness"] == 100, f"layout values: {msg}")

# Setup: both prompts asked when night mode is on, and both keys written.
root = pathlib.Path(tempfile.mkdtemp())
conf = root / "config.toml"
shutil.copy(pathlib.Path(__file__).parent.parent / "config.example.toml", conf)
prompts = []
answers = iter(["none", "22:00-06:00", "off", "60", "10"])


def fake_input(prompt=""):
    prompts.append(prompt)
    return next(answers)


builtins.input = fake_input
s.step_display(conf)
display = tomllib.loads(conf.read_text())["display"]
check(display.get("brightness") == 60 and display.get("night_brightness") == 10,
      f"setup writes both keys: {display}")
check(any(p.startswith("Day brightness (1-100)") for p in prompts), f"day prompt asked: {prompts}")
check(any(p.startswith("Night brightness (0-100, 0 = screen off)") for p in prompts),
      f"night prompt asked with night on: {prompts}")

# Setup: night brightness is not asked when night mode is off; Enter keeps the current values.
prompts.clear()
answers = iter(["none", "off", ""])
s.step_display(conf)
display = tomllib.loads(conf.read_text())["display"]
check(not any(p.startswith("Night brightness") for p in prompts), f"no night brightness prompt: {prompts}")
check(display.get("brightness") == 60 and display.get("night_brightness") == 10,
      f"Enter keeps current values: {display}")
shutil.rmtree(root)

print("\nPROBLEMS:" if problems else "\nOK", problems if problems else "")
