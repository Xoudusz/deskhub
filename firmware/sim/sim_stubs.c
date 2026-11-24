// The board's platform modules, for the browser: display, net, hub_server, ota, hubcfg, NVS,
// restart. Single-threaded, so display_lock() is a no-op.
#include <stdio.h>
#include <string.h>

#include <emscripten.h>

#include "display.h"
#include "esp_system.h"
#include "hub_server.h"
#include "hubcfg.h"
#include "net.h"
#include "nvs.h"
#include "ota.h"

// ---- restart ----------------------------------------------------------------
void esp_restart(void)
{
    EM_ASM({ throw new Error("deskhub-sim-restart"); });
    __builtin_unreachable();
}

// ---- display ----------------------------------------------------------------
esp_err_t display_start(void) { return ESP_OK; }
void display_lock(void) {}
void display_unlock(void) {}
void display_wake(void) {}
void display_set_pc_sleep_mode(int mode) { (void)mode; }
void display_set_pc_asleep(bool asleep) { (void)asleep; }
void display_set_night(int start_min, int end_min, int mode) { (void)start_min; (void)end_min; (void)mode; }
void display_set_brightness(int day_pct, int night_pct) { (void)day_pct; (void)night_pct; }
void display_inject_tap(uint16_t x, uint16_t y) { (void)x; (void)y; }
void display_inject_drag(uint16_t x, uint16_t y, uint16_t x2, int ms) { (void)x; (void)y; (void)x2; (void)ms; }

// ---- net: always online with a valid clock ------------------------------------
void net_start(void) {}
bool net_connected(void) { return true; }
bool net_needs_setup(void) { return false; }
bool net_time_valid(void) { return true; }
const char *net_ip(void) { return "simulator"; }

// ---- hub_server: the PC is the app itself; what the board would send is dropped ----
void hub_server_start(void) {}
bool hub_server_pc_online(void) { return true; }
void hub_server_send(const char *json) { (void)json; }

// ---- ota ----------------------------------------------------------------------
void ota_register(httpd_handle_t server) { (void)server; }
void ota_answer(bool allow) { (void)allow; }
void ota_mark_valid(void) {}

// ---- NVS: values in globalThis.__deskhubSimNvs, keyed "namespace/key" ----------
static char s_ns[8][16];
static int s_ns_count;

EM_JS(int, js_nvs_get, (const char *key, void *out, int cap), {
    const store = globalThis.__deskhubSimNvs || (globalThis.__deskhubSimNvs = {});
    const v = store[UTF8ToString(key)];
    if (!v) return -1;
    if (out && v.length <= cap) HEAPU8.set(v, out);
    return v.length;
});

EM_JS(void, js_nvs_set, (const char *key, const void *val, int len), {
    const store = globalThis.__deskhubSimNvs || (globalThis.__deskhubSimNvs = {});
    store[UTF8ToString(key)] = HEAPU8.slice(val, val + len);
});

static void full_key(nvs_handle_t h, const char *key, char *out, size_t n)
{
    snprintf(out, n, "%s/%s", h < (nvs_handle_t)s_ns_count ? s_ns[h] : "?", key);
}

esp_err_t nvs_open(const char *ns, nvs_open_mode_t mode, nvs_handle_t *h)
{
    (void)mode;
    for (int i = 0; i < s_ns_count; i++) {
        if (!strcmp(s_ns[i], ns)) { *h = (nvs_handle_t)i; return ESP_OK; }
    }
    if (s_ns_count == 8) return ESP_FAIL;
    snprintf(s_ns[s_ns_count], sizeof(s_ns[0]), "%s", ns);
    *h = (nvs_handle_t)s_ns_count++;
    return ESP_OK;
}

void nvs_close(nvs_handle_t h) { (void)h; }
esp_err_t nvs_commit(nvs_handle_t h) { (void)h; return ESP_OK; }

esp_err_t nvs_get_blob(nvs_handle_t h, const char *key, void *out, size_t *len)
{
    char k[64];
    full_key(h, key, k, sizeof(k));
    int n = js_nvs_get(k, NULL, 0);
    if (n < 0) return ESP_ERR_NVS_NOT_FOUND;
    if (!out) { *len = (size_t)n; return ESP_OK; }
    if ((size_t)n > *len) return ESP_ERR_NVS_INVALID_LENGTH;
    js_nvs_get(k, out, (int)*len);
    *len = (size_t)n;
    return ESP_OK;
}

esp_err_t nvs_set_blob(nvs_handle_t h, const char *key, const void *val, size_t len)
{
    char k[64];
    full_key(h, key, k, sizeof(k));
    js_nvs_set(k, val, (int)len);
    return ESP_OK;
}

esp_err_t nvs_get_str(nvs_handle_t h, const char *key, char *out, size_t *len)
{
    return nvs_get_blob(h, key, out, len);   // stored with its terminating 0
}

esp_err_t nvs_set_str(nvs_handle_t h, const char *key, const char *val)
{
    return nvs_set_blob(h, key, val, strlen(val) + 1);
}

esp_err_t nvs_get_u8(nvs_handle_t h, const char *key, uint8_t *out)
{
    size_t n = 1;
    return nvs_get_blob(h, key, out, &n);
}

esp_err_t nvs_set_u8(nvs_handle_t h, const char *key, uint8_t val) { return nvs_set_blob(h, key, &val, 1); }

esp_err_t nvs_get_u32(nvs_handle_t h, const char *key, uint32_t *out)
{
    size_t n = 4;
    return nvs_get_blob(h, key, out, &n);
}

esp_err_t nvs_set_u32(nvs_handle_t h, const char *key, uint32_t val) { return nvs_set_blob(h, key, &val, 4); }

// ---- hubcfg: location from the app's "location" message, kept in NVS ----------
static hub_cfg_t s_cfg;

static void load_str(nvs_handle_t h, const char *key, char *out, size_t n, const char *fallback)
{
    size_t len = n;
    if (nvs_get_str(h, key, out, &len) != ESP_OK) snprintf(out, n, "%s", fallback);
}

void hubcfg_load(void)
{
    nvs_handle_t h;
    nvs_open("hub", NVS_READWRITE, &h);
    memset(&s_cfg, 0, sizeof(s_cfg));
    load_str(h, "h_city", s_cfg.city, sizeof(s_cfg.city), "New York");
    load_str(h, "h_lat", s_cfg.lat, sizeof(s_cfg.lat), "40.7128");
    load_str(h, "h_lon", s_cfg.lon, sizeof(s_cfg.lon), "-74.0060");
    load_str(h, "h_tz", s_cfg.tz, sizeof(s_cfg.tz), "");
    uint8_t b = 0;
    s_cfg.fahrenheit = nvs_get_u8(h, "h_f", &b) == ESP_OK && b;
    b = 1;
    nvs_get_u8(h, "h_24", &b);
    s_cfg.h24 = b;
    s_cfg.screen_off_min = 0;
    s_cfg.creds_in_nvs = true;
}

const hub_cfg_t *hubcfg(void) { return &s_cfg; }
bool hubcfg_token_ok(const char *token) { (void)token; return false; }
const char *hubcfg_variant(void) { return "sim"; }
const char *hubcfg_creds(void) { return "nvs"; }

// Unlike the board (hubcfg.c), no validation: the app only sends locations the board accepts,
// and the simulator only needs the values for the clock, units and weather URL.
bool hubcfg_set_location(const char *city, const char *lat, const char *lon, const char *tz, bool fahrenheit, bool h24)
{
    if (!strcmp(city, s_cfg.city) && !strcmp(lat, s_cfg.lat) && !strcmp(lon, s_cfg.lon) && !strcmp(tz, s_cfg.tz) &&
        fahrenheit == s_cfg.fahrenheit && h24 == s_cfg.h24) {
        return false;
    }
    nvs_handle_t h;
    nvs_open("hub", NVS_READWRITE, &h);
    nvs_set_str(h, "h_city", city);
    nvs_set_str(h, "h_lat", lat);
    nvs_set_str(h, "h_lon", lon);
    nvs_set_str(h, "h_tz", tz);
    nvs_set_u8(h, "h_f", fahrenheit);
    nvs_set_u8(h, "h_24", h24);
    return true;   // the caller restarts: the next instance loads these
}
