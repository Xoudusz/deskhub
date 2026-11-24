#include "hubcfg.h"

#include <ctype.h>
#include <stddef.h>
#include <stdlib.h>
#include <string.h>

#include "esp_log.h"
#include "esp_random.h"
#include "esp_rom_crc.h"
#include "nvs.h"
#include "sdkconfig.h"
#include "wifi_store.h"

// Set by build.sh through CMake (firmware/main/CMakeLists.txt).
#ifndef HUB_VARIANT
#define HUB_VARIANT "unknown"
#endif

// Kconfig bools are undefined when off.
#ifdef CONFIG_HUB_FAHRENHEIT
#define SEED_FAHRENHEIT true
#else
#define SEED_FAHRENHEIT false
#endif
#ifdef CONFIG_HUB_24H
#define SEED_24H true
#else
#define SEED_24H false
#endif

static const char *TAG = "hubcfg";
static hub_cfg_t s_cfg;

#define FIELD(name) offsetof(hub_cfg_t, name), sizeof(((hub_cfg_t *)0)->name)
static const struct {
    const char *key;
    size_t off;
    size_t len;
} STRS[] = {
    {"h_ssid", FIELD(ssid)}, {"h_pass", FIELD(pass)}, {"h_token", FIELD(token)},
    {"h_host", FIELD(host)}, {"h_tz", FIELD(tz)},     {"h_city", FIELD(city)},
    {"h_lat", FIELD(lat)},   {"h_lon", FIELD(lon)},
};
#define NSTRS (sizeof(STRS) / sizeof(STRS[0]))

static char *str_at(hub_cfg_t *c, size_t i)
{
    return (char *)c + STRS[i].off;
}

// The values compiled into this image (sdkconfig.local, or the Kconfig defaults).
static void seed(hub_cfg_t *c)
{
    memset(c, 0, sizeof(*c));
    strlcpy(c->ssid, CONFIG_HUB_WIFI_SSID, sizeof(c->ssid));
    strlcpy(c->pass, CONFIG_HUB_WIFI_PASSWORD, sizeof(c->pass));
    // Only a local build (one with Wi-Fi settings) brings a token; a release image's Kconfig
    // default ("change-me") must never become the board's token.
    if (c->ssid[0]) strlcpy(c->token, CONFIG_HUB_TOKEN, sizeof(c->token));
    strlcpy(c->host, CONFIG_HUB_HOSTNAME, sizeof(c->host));
    strlcpy(c->tz, CONFIG_HUB_TZ, sizeof(c->tz));
    strlcpy(c->city, CONFIG_HUB_CITY, sizeof(c->city));
    strlcpy(c->lat, CONFIG_HUB_LATITUDE, sizeof(c->lat));
    strlcpy(c->lon, CONFIG_HUB_LONGITUDE, sizeof(c->lon));
    c->fahrenheit = SEED_FAHRENHEIT;
    c->h24 = SEED_24H;
    c->screen_off_min = CONFIG_HUB_SCREEN_OFF_MIN;
}

static esp_err_t save_all(nvs_handle_t h, hub_cfg_t *c)
{
    esp_err_t err = ESP_OK;
    for (size_t i = 0; i < NSTRS && err == ESP_OK; i++) err = nvs_set_str(h, STRS[i].key, str_at(c, i));
    if (err == ESP_OK) err = nvs_set_u8(h, "h_f", c->fahrenheit);
    if (err == ESP_OK) err = nvs_set_u8(h, "h_24", c->h24);
    if (err == ESP_OK) err = nvs_set_u16(h, "h_off", (uint16_t)c->screen_off_min);
    return err;
}

static void read_all(nvs_handle_t h, hub_cfg_t *c)
{
    char buf[80];
    for (size_t i = 0; i < NSTRS; i++) {
        size_t len = sizeof(buf);
        if (nvs_get_str(h, STRS[i].key, buf, &len) == ESP_OK) strlcpy(str_at(c, i), buf, STRS[i].len);
    }
    uint8_t u8;
    uint16_t u16;
    if (nvs_get_u8(h, "h_f", &u8) == ESP_OK) c->fahrenheit = u8 != 0;
    if (nvs_get_u8(h, "h_24", &u8) == ESP_OK) c->h24 = u8 != 0;
    if (nvs_get_u16(h, "h_off", &u16) == ESP_OK && u16 <= 240) c->screen_off_min = u16;
}

static bool nvs_has(nvs_handle_t h, const char *key)
{
    char buf[80];
    size_t len = sizeof(buf);
    return nvs_get_str(h, key, buf, &len) == ESP_OK && buf[0];
}

// A new random token for a board that has none (a release image set up on its Wi-Fi screen).
// 16 characters from 32 easy-to-type ones (no 0/o, 1/l): 80 bits.
static void make_token(char *out, size_t n)
{
    static const char ABC[] = "abcdefghijkmnpqrstuvwxyz23456789";
    size_t len = n - 1 < 16 ? n - 1 : 16;
    for (size_t i = 0; i < len; i++) out[i] = ABC[esp_random() % 32];
    out[len] = 0;
}

void hubcfg_load(void)
{
    seed(&s_cfg);
    const bool seed_creds = s_cfg.ssid[0] != 0;
    bool wifi_fresh = false;   // a local build with Wi-Fi settings it hasn't stored before

    nvs_handle_t h;
    if (nvs_open("hub", NVS_READWRITE, &h) != ESP_OK) {
        ESP_LOGE(TAG, "nvs open failed; using the compiled-in settings");
        wifi_store_init(s_cfg.ssid, s_cfg.pass, true);
        return;
    }
    bool use_nvs = true;
    if (seed_creds) {
        // Its own checksum for the Wi-Fi part: a new city in sdkconfig.local must not bring
        // back a network that was forgotten on the board.
        uint32_t wcrc = esp_rom_crc32_le(0, (const uint8_t *)s_cfg.ssid, sizeof(s_cfg.ssid));
        wcrc = esp_rom_crc32_le(wcrc, (const uint8_t *)s_cfg.pass, sizeof(s_cfg.pass));
        uint32_t old_w = 0;
        if (nvs_get_u32(h, "seed_wcrc", &old_w) != ESP_OK || old_w != wcrc) {
            if (nvs_set_u32(h, "seed_wcrc", wcrc) == ESP_OK && nvs_commit(h) == ESP_OK) wifi_fresh = true;
        }
        // A local build: copy its settings in on first boot and whenever they change.
        const uint32_t crc = esp_rom_crc32_le(0, (const uint8_t *)&s_cfg, sizeof(s_cfg));
        uint32_t old = 0;
        if (nvs_get_u32(h, "seed_crc", &old) != ESP_OK || old != crc) {
            esp_err_t err = save_all(h, &s_cfg);
            if (err == ESP_OK) err = nvs_set_u32(h, "seed_crc", crc);
            if (err == ESP_OK) err = nvs_commit(h);
            if (err == ESP_OK) {
                ESP_LOGI(TAG, "settings from this build saved to nvs");
            } else {
                ESP_LOGE(TAG, "saving settings to nvs failed (%s); using the compiled-in ones",
                         esp_err_to_name(err));
                use_nvs = false;
            }
        }
    }
    if (use_nvs) read_all(h, &s_cfg);
    if (use_nvs && !s_cfg.token[0]) {
        make_token(s_cfg.token, sizeof(s_cfg.token));
        esp_err_t err = nvs_set_str(h, "h_token", s_cfg.token);
        if (err == ESP_OK) err = nvs_commit(h);
        if (err == ESP_OK) {
            ESP_LOGI(TAG, "made a new token (shown on the Wi-Fi screen)");
        } else {
            ESP_LOGE(TAG, "saving the new token failed: %s", esp_err_to_name(err));
        }
    }
    const bool token_in_nvs = nvs_has(h, "h_token");
    nvs_close(h);
    // The single network of older firmware (h_ssid) becomes the first saved one.
    wifi_store_init(s_cfg.ssid, s_cfg.pass, wifi_fresh || !use_nvs);
    s_cfg.creds_in_nvs = wifi_store_count() > 0 && token_in_nvs;
    ESP_LOGI(TAG, "variant %s, credentials %s", HUB_VARIANT, hubcfg_creds());
}

const hub_cfg_t *hubcfg(void)
{
    return &s_cfg;
}

bool hubcfg_token_ok(const char *token)
{
    return token && token[0] && s_cfg.token[0] && strcmp(token, s_cfg.token) == 0;
}

const char *hubcfg_variant(void)
{
    return HUB_VARIANT;
}

const char *hubcfg_creds(void)
{
    if (s_cfg.creds_in_nvs) return "nvs";
    return s_cfg.ssid[0] && s_cfg.token[0] ? "seed" : "none";
}

// A plain decimal ("-48.2082"): it goes into the weather URL as is.
static bool number_in(const char *s, double lo, double hi)
{
    if (!s || !s[0]) return false;
    for (const char *p = s; *p; p++) {
        if (!(isdigit((unsigned char)*p) || *p == '.' || (*p == '-' && p == s))) return false;
    }
    char *end = NULL;
    double v = strtod(s, &end);
    return end && *end == 0 && v >= lo && v <= hi;
}

// Non-empty, fits, and no control characters (UTF-8 such as "Zürich" is fine).
static bool printable(const char *s, size_t max)
{
    if (!s || !s[0] || strlen(s) >= max) return false;
    for (; *s; s++) {
        unsigned char c = (unsigned char)*s;
        if (c < 0x20 || c == 0x7f) return false;
    }
    return true;
}

bool hubcfg_set_location(const char *city, const char *lat, const char *lon, const char *tz,
                         bool fahrenheit, bool h24)
{
    if (!printable(city, sizeof(s_cfg.city)) || !printable(tz, sizeof(s_cfg.tz)) ||
        strlen(lat) >= sizeof(s_cfg.lat) || strlen(lon) >= sizeof(s_cfg.lon) ||
        !number_in(lat, -90, 90) || !number_in(lon, -180, 180)) {
        ESP_LOGW(TAG, "location rejected");
        return false;
    }
    if (strcmp(city, s_cfg.city) == 0 && strcmp(lat, s_cfg.lat) == 0 && strcmp(lon, s_cfg.lon) == 0 &&
        strcmp(tz, s_cfg.tz) == 0 && fahrenheit == s_cfg.fahrenheit && h24 == s_cfg.h24) {
        return false;
    }
    nvs_handle_t h;
    if (nvs_open("hub", NVS_READWRITE, &h) != ESP_OK) return false;
    esp_err_t err = nvs_set_str(h, "h_city", city);
    if (err == ESP_OK) err = nvs_set_str(h, "h_lat", lat);
    if (err == ESP_OK) err = nvs_set_str(h, "h_lon", lon);
    if (err == ESP_OK) err = nvs_set_str(h, "h_tz", tz);
    if (err == ESP_OK) err = nvs_set_u8(h, "h_f", fahrenheit);
    if (err == ESP_OK) err = nvs_set_u8(h, "h_24", h24);
    if (err == ESP_OK) err = nvs_commit(h);
    nvs_close(h);
    if (err != ESP_OK) {
        ESP_LOGE(TAG, "saving location failed: %s", esp_err_to_name(err));
        return false;
    }
    // s_cfg stays as it is: other tasks read it, and the restart loads the new values.
    return true;
}
