#!/usr/bin/env bash
# Build the firmware in Docker, without installing ESP-IDF.
#
#   ./build.sh s3        Waveshare ESP32-S3-Touch-LCD-4.3B
#   ./build.sh p4rev3    Waveshare ESP32-P4-WIFI6-Touch-LCD-4.3, chip revision v3.x
#   ./build.sh p4rev1    the same board with chip revision v1.x
#
# Read the chip revision with: esptool --chip esp32p4 chip_id
#
# Wi-Fi, token, city and the other Desk Hub settings come from sdkconfig.local
# if it exists (git-ignored; start from sdkconfig.local.example).
# Output: out/<variant>/ with deskhub.bin (for Wi-Fi updates) and
# deskhub-merged.bin (flash at 0x0 over USB).
set -euo pipefail

IDF_IMAGE="${IDF_IMAGE:-espressif/idf:v5.5.4}"
variant="${1:-}"
case "$variant" in
    s3)     target=esp32s3; defaults="sdkconfig.defaults" ;;
    p4rev1) target=esp32p4; defaults="sdkconfig.defaults;sdkconfig.p4rev1" ;;
    p4rev3) target=esp32p4; defaults="sdkconfig.defaults;sdkconfig.p4rev3" ;;
    *) echo "usage: $0 s3|p4rev1|p4rev3" >&2; exit 2 ;;
esac

cd "$(dirname "$0")"
if [[ -f sdkconfig.local ]]; then
    defaults="$defaults;sdkconfig.local"
fi

build="build/$variant"
# Firmware version shown by the board and the desktop app: git describe of this
# checkout, computed here because the container has no access to .git.
version=$(git describe --tags --always --dirty 2>/dev/null || echo dev)
# Start each build from the defaults files, so edits to sdkconfig.local (or the
# defaults) always take effect. The old sdkconfig is removed inside the
# container because the container owns the build directory.
docker run --rm \
    -v "$PWD":/project -w /project \
    -e IDF_TARGET="$target" -e HOME=/tmp \
    "$IDF_IMAGE" \
    bash -c 'rm -f "$1/sdkconfig" && idf.py -B "$1" -D SDKCONFIG="$1/sdkconfig" -D SDKCONFIG_DEFAULTS="$2" -D PROJECT_VER="$3" build merge-bin -o deskhub-merged.bin' \
        _ "$build" "$defaults" "$version"

mkdir -p "out/$variant"
cp "$build/deskhub.bin" "$build/deskhub-merged.bin" "out/$variant/"
echo "built $variant: out/$variant/"
