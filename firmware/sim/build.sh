#!/usr/bin/env bash
# Builds the board simulator (WASM) into desktop/public/sim/ with the pinned emsdk image.
# LVGL and cJSON are fetched by tag into firmware/sim/.deps (not committed).
# Without docker (CI with setup-emsdk): SIM_NO_DOCKER=1 bash firmware/sim/build.sh
set -euo pipefail

EMSDK_IMAGE="emscripten/emsdk:7.0.1"
LVGL_TAG="v9.2.2"
CJSON_TAG="v1.7.18"

here="$(cd "$(dirname "$0")" && pwd)"
repo="$(cd "$here/../.." && pwd)"
deps="$here/.deps"
out="$repo/desktop/public/sim"

mkdir -p "$deps" "$out"
[ -d "$deps/lvgl" ] || git clone -q --depth 1 --branch "$LVGL_TAG" https://github.com/lvgl/lvgl.git "$deps/lvgl"
[ -d "$deps/cJSON" ] || git clone -q --depth 1 --branch "$CJSON_TAG" https://github.com/DaveGamble/cJSON.git "$deps/cJSON"

build='emcmake cmake -S firmware/sim -B firmware/sim/.build -DCMAKE_BUILD_TYPE=Release >/dev/null &&
       cmake --build firmware/sim/.build -j"$(nproc)" &&
       cp firmware/sim/.build/deskhub_sim.js firmware/sim/.build/deskhub_sim.wasm desktop/public/sim/'

if [ "${SIM_NO_DOCKER:-}" = "1" ]; then
    (cd "$repo" && bash -c "$build")
else
    docker run --rm -u "$(id -u):$(id -g)" -v "$repo:/src" -w /src "$EMSDK_IMAGE" bash -c "$build"
fi
ls -l "$out"
