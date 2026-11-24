#pragma once

// Runtime settings that used to be compiled in: Wi-Fi, token, hostname and location.
// They live in NVS (namespace "hub", keys h_*), so release images carry no secrets.
// The Kconfig HUB_* values (sdkconfig.local) are only a seed: a local build copies them
// into NVS on first boot, and again whenever they change. A public build has no seed and
// keeps whatever the board already stores.

#include <stdbool.h>

typedef struct {
    char ssid[33];
    char pass[65];
    char token[64];
    char host[32];
    char tz[64];
    char city[40];
    char lat[16];
    char lon[16];
    bool fahrenheit;
    bool h24;
    int screen_off_min;
    // ssid/pass: the single network of older firmware and local builds; the board now joins
    // the saved networks in wifi_store.h (this one is the first of them).
    // Wi-Fi and token are stored in NVS (not only compiled in): GitHub images will work.
    bool creds_in_nvs;
} hub_cfg_t;

// Once, after nvs_flash_init() and before anything reads the settings.
void hubcfg_load(void);
const hub_cfg_t *hubcfg(void);

// True when `token` is non-empty and equals the board's token.
bool hubcfg_token_ok(const char *token);

// "s3", "p4rev1", "p4rev3" (build.sh) or "unknown": picks the release image.
const char *hubcfg_variant(void);

// "nvs", "seed" (compiled in only) or "none".
const char *hubcfg_creds(void);

// Location from the app. Returns true when it differs and was saved to NVS; the caller then
// restarts (the running settings are not changed). False when identical or invalid.
bool hubcfg_set_location(const char *city, const char *lat, const char *lon, const char *tz,
                         bool fahrenheit, bool h24);
