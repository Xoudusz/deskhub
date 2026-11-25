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
#include "wifi_store.h"

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

// ---- net: fake networks, a valid clock --------------------------------------------
// The saved list is the real wifi_store (in the JS NVS). A scan takes a second and finds
// FAKE_APS; joining takes 1.5 s and fails for a password starting with "wrong" (or "Far Away").
static const net_ap_t FAKE_APS[] = {
    {"Home Wi-Fi", -45, true}, {"Office", -58, true}, {"Cafe Guest", -67, false},
    {"Neighbour 5G", -76, true}, {"Far Away", -88, true},
};
#define N_FAKE ((int)(sizeof(FAKE_APS) / sizeof(FAKE_APS[0])))
static char s_cur[33];
static double s_scan_at = -1, s_join_at = -1;
static uint32_t s_gen;
static int s_nap;
static char s_join_ssid[33], s_join_pass[65];
static net_join_t s_join = NET_JOIN_IDLE;

static bool fake_in_range(const char *ssid)
{
    for (int i = 0; i < N_FAKE; i++) {
        if (!strcmp(FAKE_APS[i].ssid, ssid) && strcmp(ssid, "Far Away")) return true;
    }
    return false;
}

// The first saved network in range, as the board would pick (by recency, not signal).
static void auto_join(void)
{
    wifi_net_t n;
    s_cur[0] = 0;
    for (int i = 0; wifi_store_get(i, &n); i++) {
        if (fake_in_range(n.ssid)) { snprintf(s_cur, sizeof(s_cur), "%s", n.ssid); return; }
    }
}

static void poll(void)
{
    double now = emscripten_get_now();
    if (s_scan_at >= 0 && now >= s_scan_at) { s_scan_at = -1; s_nap = N_FAKE; s_gen++; }
    if (s_join_at >= 0 && now >= s_join_at) {
        s_join_at = -1;
        if (!strncmp(s_join_pass, "wrong", 5) || !fake_in_range(s_join_ssid)) {
            s_join = NET_JOIN_FAIL;
            auto_join();
        } else {
            wifi_store_add(s_join_ssid, s_join_pass);
            wifi_store_flush();
            snprintf(s_cur, sizeof(s_cur), "%s", s_join_ssid);
            s_join = NET_JOIN_OK;
        }
    }
}

void net_start(void) { auto_join(); }
bool net_connected(void) { poll(); return s_cur[0] != 0; }
bool net_needs_setup(void) { return wifi_store_count() == 0; }
bool net_time_valid(void) { return true; }
const char *net_ip(void) { poll(); return s_cur[0] ? "192.168.1.50" : ""; }
void net_ssid(char *out, size_t n) { poll(); snprintf(out, n, "%s", s_cur); }
bool net_scan_start(void) { if (s_scan_at < 0) s_scan_at = emscripten_get_now() + 1000; return true; }
bool net_scanning(void) { poll(); return s_scan_at >= 0; }

int net_scan_results(net_ap_t *out, int max, uint32_t *gen)
{
    poll();
    int n = s_nap < max ? s_nap : max;
    memcpy(out, FAKE_APS, (size_t)n * sizeof(net_ap_t));
    if (gen) *gen = s_gen;
    return n;
}

void net_join(const char *ssid, const char *pass)
{
    snprintf(s_join_ssid, sizeof(s_join_ssid), "%s", ssid);
    snprintf(s_join_pass, sizeof(s_join_pass), "%s", pass);
    s_cur[0] = 0;
    s_join = NET_JOIN_BUSY;
    s_join_at = emscripten_get_now() + 1500;
}

net_join_t net_join_state(void) { poll(); return s_join; }

bool net_join_saved(const char *ssid)
{
    wifi_net_t n;
    if (!wifi_store_find(ssid, &n)) return false;
    net_join(n.ssid, n.pass);
    return true;
}

void net_forget(const char *ssid)
{
    wifi_store_forget(ssid);
    wifi_store_flush();
    if (!strcmp(ssid, s_cur)) auto_join();
}

int net_saved(char (*out)[33], int max)
{
    wifi_net_t n;
    int i = 0;
    while (i < max && wifi_store_get(i, &n)) { snprintf(out[i], 33, "%s", n.ssid); i++; }
    return i;
}

// ---- hub_server: the PC is the app itself; what the board would send is dropped ----
void hub_server_start(void) {}
bool hub_server_pc_online(void) { return true; }
void hub_server_send(const char *json) { (void)json; }

// ---- ota ----------------------------------------------------------------------
void ota_register(httpd_handle_t server) { (void)server; }
void ota_answer(bool allow) { (void)allow; }
void ota_mark_valid(void) {}

// ---- pair: the prompt only (sim_main.c's sim_pair shows it) ------------------------------
void ui_pair_hide(void);
void pair_answer(bool allow)
{
    printf("I pair: %s\n", allow ? "allowed" : "denied");
    ui_pair_hide();
}

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
    snprintf(s_cfg.host, sizeof(s_cfg.host), "deskhub");
    // Not a real token: the simulator never talks to the app over the network.
    snprintf(s_cfg.token, sizeof(s_cfg.token), "simulator");
    wifi_store_init("Home Wi-Fi", "simulator", false);   // a first network, like a set-up board
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
