"""Bambu printer support without a printer: report parsing, the local MQTT path against a
tiny in-process broker, the cloud sign-in against a mocked HTTP API, pages and the CLI.

    uv run python tests/test_printer.py
Prints OK or the problems found. Never touches the OS credential store.
"""
import asyncio
import builtins
import getpass
import json
import pathlib
import sys
import tempfile
import threading
import time
import tomllib
import types

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent.parent))

import httpx  # noqa: E402
import tomlkit  # noqa: E402

from deskhub import bambu_cloud as bc  # noqa: E402
from deskhub import printer as pr  # noqa: E402
from deskhub import printers_cli  # noqa: E402
from deskhub.pages import enabled_pages, layout_message  # noqa: E402

problems = []


def check(cond, msg):
    if not cond:
        problems.append(msg)


# ---- keyring stub ------------------------------------------------------------
store: dict = {}
kr = types.ModuleType("keyring")
kr.errors = types.SimpleNamespace(PasswordDeleteError=type("PasswordDeleteError", (Exception,), {}))
kr.set_password = lambda s, k, v: store.__setitem__((s, k), v)
kr.get_password = lambda s, k: store.get((s, k))


def _del(s, k):
    if (s, k) not in store:
        raise kr.errors.PasswordDeleteError()
    del store[(s, k)]


kr.delete_password = _del
sys.modules["keyring"] = kr


# ---- sample reports --------------------------------------------------------------
def rep(**kw):
    return {"print": {"command": "push_status", **kw}}


def tray(i, color=None, kind=None):
    t = {"id": str(i)}
    if color:
        t.update(tray_color=color, tray_type=kind)
    return t


P1S_FULL = rep(gcode_state="RUNNING", mc_percent=42, mc_remaining_time=83, layer_num=120,
               total_layer_num=300, subtask_name="Benchy.gcode.3mf",
               gcode_file="/data/Metadata/plate_1.gcode", nozzle_temper=219.5,
               nozzle_target_temper=220, bed_temper=55, bed_target_temper=55, spd_lvl=2,
               print_error=0, wifi_signal="-50dBm",
               ams={"tray_now": "0", "ams": [{"id": "0", "tray": [tray(0, "FF0000FF", "PLA")]}]})
P1S_DELTA = rep(mc_percent=43, layer_num=125, nozzle_temper=220.1)
A1_AMS_LITE = rep(gcode_state="PAUSE", mc_percent=10, mc_remaining_time=240, layer_num=8,
                  total_layer_num=90, subtask_name="Café stand.3mf", nozzle_temper=200,
                  nozzle_target_temper=200, bed_temper=60, bed_target_temper=60, spd_lvl=3,
                  ams={"tray_now": "2", "ams": [{"id": "0", "tray": [
                      tray(0, "FFFFFFFF", "PLA"), tray(1, "000000FF", "PETG"),
                      tray(2, "0000FFFF", "PLA"), tray(3, "FFA500FF", "TPU")]}]})
X1C = rep(gcode_state="FINISH", mc_percent=100, mc_remaining_time=0, layer_num=300,
          total_layer_num=300, subtask_name="Gear", nozzle_temper=30, nozzle_target_temper=0,
          bed_temper=28, bed_target_temper=0, chamber_temper=38, spd_lvl=4, print_error=0x03008000,
          ams={"tray_now": "255", "ams": [{"id": "0", "tray": [
              tray(0, "11223344", "ABS"), tray(1), tray(2), tray(3)]}]})
H2D = rep(gcode_state="RUNNING", mc_percent=7, mc_remaining_time=500, layer_num=3,
          total_layer_num=400, subtask_name="Vase", spd_lvl=1,
          device={"extruder": {"state": 0x10, "info": [
                      {"id": 0, "temp": (210 << 16) | 100}, {"id": 1, "temp": (255 << 16) | 250}]},
                  "bed": {"info": {"temp": (60 << 16) | 58}}, "ctc": {"info": {"temp": 41}}},
          ams={"tray_now": "255", "ams": []})


def msg_for(*reports, name="P", model="P1S"):
    st = pr.PrinterState()
    for r in reports:
        st.merge(r["print"], now=100)
    st.connected = True
    return st.message(name, model, now=101)


m = msg_for(P1S_FULL, name="P1S")
check(m == {"name": "P1S", "model": "P1S", "online": True, "state": "RUNNING", "pct": 42,
            "remaining_min": 83, "layer": 120, "layers": 300, "file": "Benchy", "nozzle": 219.5,
            "nozzle_target": 220, "bed": 55, "bed_target": 55, "chamber": None, "speed": "Standard",
            "error": "", "ams": [{"color": "#FF0000", "type": "PLA", "active": True}]}, f"P1S full: {m}")
json.dumps(m)

m = msg_for(P1S_FULL, P1S_DELTA)
check((m["pct"], m["layer"], m["nozzle"], m["bed"], m["file"], m["ams"][0]["type"]) ==
      (43, 125, 220.1, 55, "Benchy", "PLA"), f"delta merge: {m}")
m = msg_for(P1S_FULL, rep(ams={"ams": [{"id": "0", "tray": [tray(1, "00FF00FF", "PETG")]}]}))
check([a["type"] for a in m["ams"]] == ["PLA", "PETG"], f"ams delta merge: {m['ams']}")

m = msg_for(A1_AMS_LITE, model="A1")
check(m["state"] == "PAUSE" and m["speed"] == "Sport" and m["file"] == "Cafe stand", f"A1: {m}")
check([a["active"] for a in m["ams"]] == [False, False, True, False] and len(m["ams"]) == 4, "A1 active slot")
check(m["ams"][3] == {"color": "#FFA500", "type": "TPU", "active": False}, "A1 slot colour")

m = msg_for(X1C, model="X1C")
check(m["chamber"] == 38 and m["state"] == "FINISH" and m["speed"] == "Ludicrous", f"X1C: {m}")
check(m["error"] == "Error 0300-8000", f"X1C error: {m['error']}")
check(len(m["ams"]) == 1 and not m["ams"][0]["active"], "X1C empty slots skipped")

m = msg_for(H2D, model="H2D")
check((m["nozzle"], m["nozzle_target"], m["bed"], m["bed_target"], m["chamber"]) ==
      (250, 255, 58, 60, 41), f"H2D temps: {m}")
check(m["ams"] == [] and m["speed"] == "Silent", "H2D ams/speed")
big = rep(ams={"tray_now": "255", "ams": [{"id": str(i), "tray": [tray(j, "FFFFFFFF", "PLA") for j in range(4)]}
                                           for i in range(4)]})
check(len(msg_for(big)["ams"]) == 8, "AMS capped at 8")

st = pr.PrinterState()
st.merge(P1S_FULL["print"], now=100)
check(st.message("x", "", now=101)["state"] == "OFFLINE", "no connection -> OFFLINE")
st.connected = True
check(st.message("x", "", now=160)["state"] == "RUNNING", "fresh within 60s")
check(st.message("x", "", now=160.5)["state"] == "OFFLINE" and not st.message("x", "", now=170)["online"],
      "stale -> OFFLINE")
check(pr.model_of({"serial": "01P00A1"}) == "P1S" and pr.model_of({"serial": "094X"}) == "H2D", "model from serial")

# ---- pages ----------------------------------------------------------------------
import deskhub.pages as _pg  # noqa: E402
_pg.steam_installed = lambda: False
check(enabled_pages({}) == ["home", "stats", "ai", "calendar", "weather", "media", "timer", "network", "shortcuts"], "default pages")
_pg.steam_installed = lambda: True
check(enabled_pages({}) == ["home", "stats", "ai", "calendar", "weather", "media", "timer", "games", "network", "shortcuts"], "games auto-added")
_pg.steam_installed = lambda: False
check("printer" in enabled_pages({"printers": [{"serial": "x"}]}), "printer auto-added")
check(enabled_pages({"pages": {"enabled": ["stats"]}, "printers": [{"serial": "x"}]}) == ["home", "stats"],
      "explicit pages win; home forced")
check(layout_message({"pages": {"enabled": ["media", "home"]}}) == {"type": "layout", "pages": ["home", "media"], "ordered": True, "nav": "left",
                                                                      "pc_sleep": "off", "night_start": -1, "night_end": -1,
                                                                      "night_style": "off", "brightness": 100, "night_brightness": 0},
      "layout message")


# ---- a tiny MQTT broker --------------------------------------------------------
class Broker:
    """Just enough MQTT 3.1.1 (CONNECT, SUBSCRIBE, PUBLISH at QoS 0) for one printer."""

    def __init__(self, user, password, serial, full):
        self.user, self.password, self.serial, self.full = user, password, serial, full
        self.requests, self.subs, self.writers = [], [], []
        self.ready = threading.Event()
        threading.Thread(target=self._run, daemon=True).start()
        self.ready.wait(5)

    def _run(self):
        self.loop = asyncio.new_event_loop()
        asyncio.set_event_loop(self.loop)
        server = self.loop.run_until_complete(asyncio.start_server(self._client, "127.0.0.1", 0))
        self.port = server.sockets[0].getsockname()[1]
        self.ready.set()
        self.loop.run_forever()

    @staticmethod
    async def _packet(r):
        head = await r.readexactly(1)
        n, mult = 0, 1
        while True:
            b = (await r.readexactly(1))[0]
            n += (b & 0x7F) * mult
            mult *= 128
            if not b & 0x80:
                break
        return head[0], await r.readexactly(n)

    @staticmethod
    def _pub(topic, payload):
        body = len(topic).to_bytes(2, "big") + topic.encode() + payload
        n, out = len(body), bytearray()
        while True:
            b, n = n % 128, n // 128
            out.append(b | (0x80 if n else 0))
            if not n:
                break
        return b"\x30" + bytes(out) + body

    async def _client(self, r, w):
        try:
            while True:
                head, body = await self._packet(r)
                kind = head >> 4
                if kind == 1:
                    flags = body[7]
                    i = 10
                    fields = []
                    for _ in range(1 + bool(flags & 0x80) + bool(flags & 0x40)):
                        n = int.from_bytes(body[i:i + 2], "big")
                        fields.append(body[i + 2:i + 2 + n].decode())
                        i += 2 + n
                    ok = fields[1:] == [self.user, self.password]
                    w.write(bytes([0x20, 2, 0, 0 if ok else 4]))
                    if not ok:
                        await w.drain()
                        return
                    self.writers.append(w)
                elif kind == 8:
                    n = int.from_bytes(body[2:4], "big")
                    self.subs.append(body[4:4 + n].decode())
                    w.write(bytes([0x90, 3]) + body[:2] + b"\x00")
                elif kind == 3:
                    n = int.from_bytes(body[:2], "big")
                    topic, payload = body[2:2 + n].decode(), body[2 + n:]
                    self.requests.append((topic, payload))
                    if topic == f"device/{self.serial}/request" and b"pushall" in payload:
                        w.write(self._pub(f"device/{self.serial}/report", json.dumps(self.full).encode()))
                elif kind == 12:
                    w.write(b"\xd0\x00")
                elif kind == 14:
                    return
                await w.drain()
        except (asyncio.IncompleteReadError, ConnectionError):
            pass

    def push(self, report):
        data = self._pub(f"device/{self.serial}/report", json.dumps(report).encode())
        for w in self.writers:
            self.loop.call_soon_threadsafe(w.write, data)


def wait_for(cond, secs=8):
    end = time.time() + secs
    while time.time() < end:
        if cond():
            return True
        time.sleep(0.05)
    return False


SERIAL = "01P00A123456789"
CODE = "12345678"
broker = Broker("bblp", CODE, SERIAL, P1S_FULL)

# End to end over real paho: connect, subscribe, pushall, merge a delta.
store[(bc.SERVICE, pr.lan_key(SERIAL))] = CODE
svc = pr.PrinterService([{"name": "P1S", "type": "local", "serial": SERIAL, "host": "127.0.0.1"}],
                        secret_getter=lambda s, k: store.get((s, k)), host="127.0.0.1",
                        port=broker.port, tls=False)
svc.start()
check(wait_for(lambda: svc.message()["printers"][0]["state"] == "RUNNING"), "local: full state arrives")
msg = svc.message()
check(msg["type"] == "printer" and msg["printers"][0]["pct"] == 42 and msg["printers"][0]["model"] == "P1S",
      f"local message: {msg}")
check(f"device/{SERIAL}/report" in broker.subs, "subscribed to report topic")
check(any(t == f"device/{SERIAL}/request" and json.loads(p) ==
          {"pushing": {"sequence_id": "0", "command": "pushall"}} for t, p in broker.requests), "pushall published")
broker.push(P1S_DELTA)
check(wait_for(lambda: svc.message()["printers"][0]["pct"] == 43), "local: delta merged")
svc.stop()

# Wrong access code: connection refused, worker stops, one clear log line, no crash.
logs = []
handler = type("H", (pr.logging.Handler,), {"emit": lambda self, rec: logs.append(rec.getMessage())})()
pr.log.addHandler(handler)
store[(bc.SERVICE, pr.lan_key(SERIAL))] = "WRONGPW1"
bad = pr.PrinterService([{"name": "P1S", "type": "local", "serial": SERIAL, "host": "127.0.0.1"}],
                        secret_getter=lambda s, k: store.get((s, k)), host="127.0.0.1",
                        port=broker.port, tls=False)
bad.start()
check(wait_for(lambda: bad.workers[0].stop_event.is_set()), "bad code stops the worker")
check(any("deskhub-printers" in line for line in logs) and not any("WRONGPW1" in line for line in logs),
      f"auth failure logs one clear line: {logs}")
check(bad.message()["printers"][0]["state"] == "OFFLINE", "bad code -> OFFLINE")
pr.log.removeHandler(handler)
store[(bc.SERVICE, pr.lan_key(SERIAL))] = CODE

# The setup probe.
ok, text = pr.probe_local("127.0.0.1", SERIAL, CODE, timeout=5, port=broker.port, tls=False)
check(ok and "reporting" in text, f"probe ok: {text}")
ok, text = pr.probe_local("127.0.0.1", SERIAL, "WRONGPW1", timeout=5, port=broker.port, tls=False)
check(not ok and "access code" in text, f"probe bad code: {text}")
ok, text = pr.probe_local("127.0.0.1", SERIAL, CODE, timeout=1, port=1, tls=False)
check(not ok, f"probe unreachable: {text}")

# ---- cloud sign-in with a mocked API ---------------------------------------------
calls = []


def api(verify_code: bool):
    def handler(req: httpx.Request) -> httpx.Response:
        body = json.loads(req.content) if req.content else {}
        calls.append((req.method, req.url.path, body))
        path = req.url.path
        if path == "/v1/user-service/user/login":
            if "code" in body:
                return httpx.Response(200, json={"accessToken": "TOKEN-2", "loginType": "verifyCode"} if body["code"] == "123456"
                                      else {"accessToken": "", "message": "bad code"})
            if body["password"] != "pw":
                return httpx.Response(400, json={"code": 1, "error": "wrong password"})
            if verify_code:
                return httpx.Response(200, json={"accessToken": "", "loginType": "verifyCode"})
            return httpx.Response(200, json={"accessToken": "TOKEN-1"})
        if path == "/v1/user-service/user/sendemail/code":
            return httpx.Response(200, json={})
        auth = req.headers.get("authorization")
        if auth not in ("Bearer TOKEN-1", "Bearer TOKEN-2"):
            return httpx.Response(401, json={"message": "unauthorized"})
        if path == "/v1/design-user-service/my/preference":
            return httpx.Response(200, json={"uid": 4242})
        if path == "/v1/iot-service/api/user/bind":
            return httpx.Response(200, json={"devices": [
                {"dev_id": "01P00A111", "name": "Desk P1S", "dev_product_name": "P1S", "online": True},
                {"dev_id": "030000222", "name": "Mini", "dev_product_name": "A1 mini", "online": False}]})
        return httpx.Response(404, text="<html>nope</html>")
    return handler


REAL_MAKE_CLIENT = bc.make_client


def client(verify):
    return REAL_MAKE_CLIENT(transport=httpx.MockTransport(api(verify)))


with client(False) as http:
    check(bc.login(http, "us", "a@b.c", "pw") == {"status": "ok", "token": "TOKEN-1"}, "cloud login")
    check(bc.get_uid(http, "us", "TOKEN-1") == "4242", "uid")
    devs = bc.list_printers(http, "us", "TOKEN-1")
    check([d["serial"] for d in devs] == ["01P00A111", "030000222"] and devs[0]["model"] == "P1S", f"bind list {devs}")
    try:
        bc.login(http, "us", "a@b.c", "nope")
        check(False, "bad password should raise")
    except bc.AuthError:
        pass
    try:
        bc.get_uid(http, "us", "stale")
        check(False, "stale token should raise AuthError")
    except bc.AuthError:
        pass
with client(True) as http:
    check(bc.login(http, "us", "a@b.c", "pw") == {"status": "code"}, "verify-code branch")
    bc.send_code(http, "us", "a@b.c")
    check(("POST", "/v1/user-service/user/sendemail/code", {"email": "a@b.c", "type": "codeLogin"}) in calls,
          "sendemail request body")
    check(bc.login_with_code(http, "us", "a@b.c", "123456") == "TOKEN-2", "login with code")
    check(("POST", "/v1/user-service/user/login", {"account": "a@b.c", "code": "123456"}) in calls, "code login body")
    try:
        bc.login_with_code(http, "us", "a@b.c", "000000")
        check(False, "bad code should raise")
    except bc.AuthError:
        pass
html = REAL_MAKE_CLIENT(transport=httpx.MockTransport(lambda r: httpx.Response(403, text="<html>cf</html>")))
try:
    bc.login(html, "us", "a@b.c", "pw")
    check(False, "html reply should raise")
except bc.BambuError as e:
    check("HTML" not in str(e) and "<" not in str(e), "html reply handled")
cookie = REAL_MAKE_CLIENT(transport=httpx.MockTransport(
    lambda r: httpx.Response(200, headers={"set-cookie": "token=TOKEN-C; Path=/"})))
check(bc.login(cookie, "us", "a@b.c", "pw") == {"status": "ok", "token": "TOKEN-C"}, "token from cookie")
empty = REAL_MAKE_CLIENT(transport=httpx.MockTransport(lambda r: httpx.Response(200, text="")))
check(bc.login(empty, "us", "a@b.c", "pw") == {"status": "code"}, "empty reply asks for a code")
plain = REAL_MAKE_CLIENT(transport=httpx.MockTransport(lambda r: httpx.Response(200, text="hi")))
try:
    bc.login(plain, "us", "a@b.c", "pw")
    check(False, "non-JSON reply should raise")
except bc.BambuError as e:
    check("2 bytes" in str(e), f"non-JSON reply detail: {e}")
check(bc.mqtt_host("cn") == "cn.mqtt.bambulab.com" and bc.mqtt_host("us") == "us.mqtt.bambulab.com", "mqtt hosts")

# Cloud worker credentials: user u_<uid>, token as password, verified TLS host.
store[(bc.SERVICE, "a@b.c")] = "TOKEN-1"
bc_make = pr.make_client
pr.make_client = lambda: client(False)
w = pr.Worker({"name": "c", "type": "cloud", "serial": "01P00A111", "account": "a@b.c", "region": "us"},
              lambda s, k: store.get((s, k)))
check(w._credentials() == ("us.mqtt.bambulab.com", "u_4242", "TOKEN-1"), "cloud credentials")
store[(bc.SERVICE, "a@b.c")] = "stale"
try:
    w._credentials()
    check(False, "stale token should fail")
except bc.AuthError:
    pass
pr.make_client = bc_make

# ---- the CLI, end to end with scripted answers --------------------------------------
tmp = pathlib.Path(tempfile.mkdtemp()) / "config.toml"
tmp.write_text('# my config\n[hub]\nhost = "x"\n\n[pages]\nenabled = ["home", "stats"]\n', encoding="utf-8")


def run(cmd, answers, passwords):
    a, p = iter(answers), iter(passwords)
    builtins.input = lambda prompt="": next(a)
    getpass.getpass = lambda prompt="": next(p)
    cmd(tmp)


bc_make_client = bc.make_client
bc.make_client = lambda **kw: client(True)
store.clear()
run(printers_cli.cmd_add_cloud, ["a@b.c", "us", "123456", "all"], ["pw"])
bc.make_client = bc_make_client
cfg = tomllib.loads(tmp.read_text())
check([p["serial"] for p in cfg["printers"]] == ["01P00A111", "030000222"], f"cloud printers saved: {cfg.get('printers')}")
check(store.get((bc.SERVICE, "a@b.c")) == "TOKEN-2" and "TOKEN" not in tmp.read_text(), "token in keyring only")
check(cfg["pages"]["enabled"] == ["home", "stats", "printer"] and "# my config" in tmp.read_text(),
      "printer page added, comments kept")

printers_cli.probe_local = lambda *a, **k: (False, "No answer (stubbed)")
run(printers_cli.cmd_add_local, ["127.0.0.1", "01P00A999", "Shop", "y"], [CODE])
cfg = tomllib.loads(tmp.read_text())
check(any(p["serial"] == "01P00A999" and p["type"] == "local" and p["host"] == "127.0.0.1" for p in cfg["printers"]),
      "local printer saved (anyway)")
check(store.get((bc.SERVICE, "lan:01P00A999")) == CODE and CODE not in tmp.read_text(), "access code in keyring only")

run(printers_cli.cmd_remove, ["3", "y"], [])
cfg = tomllib.loads(tmp.read_text())
check([p["serial"] for p in cfg["printers"]] == ["01P00A111", "030000222"], "local removed")
check((bc.SERVICE, "lan:01P00A999") not in store, "local credential deleted")
run(printers_cli.cmd_remove, ["1", "y"], [])
check((bc.SERVICE, "a@b.c") in store, "token kept while another printer uses the account")
run(printers_cli.cmd_remove, ["1", "y"], [])
check((bc.SERVICE, "a@b.c") not in store and "printers" not in tomllib.loads(tmp.read_text()) or
      tomllib.loads(tmp.read_text()).get("printers") == [], "last cloud printer removes the token")
tomlkit.parse(tmp.read_text())

# Real X2D report (captured 2026-11-13): classic fields describe idle nozzle 0,
# the active nozzle (id 1, from extruder.state bits 4-7) is printing at 220/220.
_x2d = pr.PrinterState._temps({"nozzle_temper": 65.0, "nozzle_target_temper": 25.0, "bed_temper": 55.0,
    "bed_target_temper": 55.0, "device": {"extruder": {"info": [{"id": 0, "temp": 1638465},
    {"id": 1, "temp": 14418140}], "state": 33042}, "ctc": {"info": {"temp": 33}}}})
check(_x2d["nozzle"] == 220 and _x2d["nozzle_target"] == 220 and _x2d["chamber"] == 33, f"X2D active nozzle: {_x2d}")

print("\nPROBLEMS:" if problems else "\nOK", problems if problems else "")
