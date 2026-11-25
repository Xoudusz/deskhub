# Board simulator (WASM)

The board UI (`main/ui*.c`, LVGL 9.2.2) compiled to WebAssembly for the desktop app's
simulated preview. Same code as the board: only the platform underneath is replaced.

- `shim/`: the few ESP-IDF headers the UI includes (log, heap_caps, timer, random,
  restart, NVS, FreeRTOS delay), mapped to libc/JS.
- `sim_stubs.c`: display/net/hub_server/ota/hubcfg for the browser. Anything the board
  would send to the PC (shortcuts, media keys, Steam launch, speedtest) is dropped.
- `sim_main.c`: 800x480 RGB565 display flushed into an RGBA buffer, pointer input, and the
  exports the app calls (`sim_json`, `sim_binary`, `sim_pointer`, `sim_weather*`, ...).
- `build.sh`: builds in the pinned `emscripten/emsdk` image; fetches LVGL and cJSON by tag;
  writes `desktop/public/sim/deskhub_sim.{js,wasm}` (not committed).

    bash firmware/sim/build.sh

A restart on the board (new theme, nav side or location) throws `deskhub-sim-restart` out of
the call; the app then starts a fresh instance and feeds it the current state again. NVS
lives in a JS map (`globalThis.__deskhubSimNvs`) so it survives that.
