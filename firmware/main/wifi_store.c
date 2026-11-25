#include "wifi_store.h"

#include <stddef.h>
#include <stdint.h>
#include <string.h>

#include "esp_log.h"
#include "nvs.h"

static const char *TAG = "wifi_store";

#define KEY "h_nets"
#define VERSION 1

typedef struct {
    uint8_t version;
    uint8_t count;
    wifi_net_t nets[WIFI_STORE_MAX];
} blob_t;

static blob_t s;
static bool s_dirty;   // changed since the last flush

static size_t blob_len(int count)
{
    return offsetof(blob_t, nets) + (size_t)count * sizeof(wifi_net_t);
}

void wifi_store_flush(void)
{
    if (!s_dirty) return;
    s_dirty = false;
    nvs_handle_t h;
    if (nvs_open("hub", NVS_READWRITE, &h) != ESP_OK) {
        ESP_LOGE(TAG, "nvs open failed: saved networks last until restart");
        return;
    }
    esp_err_t err = nvs_set_blob(h, KEY, &s, blob_len(s.count));
    if (err == ESP_OK) err = nvs_commit(h);
    if (err != ESP_OK) ESP_LOGE(TAG, "saving networks failed: %s", esp_err_to_name(err));
    nvs_close(h);
}

static int find(const char *ssid)
{
    for (int i = 0; i < s.count; i++) {
        if (strcmp(s.nets[i].ssid, ssid) == 0) return i;
    }
    return -1;
}

static void remove_at(int i)
{
    memmove(&s.nets[i], &s.nets[i + 1], (size_t)(s.count - i - 1) * sizeof(wifi_net_t));
    s.count--;
    memset(&s.nets[s.count], 0, sizeof(wifi_net_t));
}

void wifi_store_init(const char *seed_ssid, const char *seed_pass, bool seed_fresh)
{
    memset(&s, 0, sizeof(s));
    s.version = VERSION;
    bool have = false;
    nvs_handle_t h;
    if (nvs_open("hub", NVS_READONLY, &h) == ESP_OK) {
        blob_t b;
        size_t len = sizeof(b);
        if (nvs_get_blob(h, KEY, &b, &len) == ESP_OK && len >= offsetof(blob_t, nets) &&
            b.version == VERSION && b.count <= WIFI_STORE_MAX && len == blob_len(b.count)) {
            memcpy(&s, &b, len);
            for (int i = 0; i < s.count; i++) {   // stored terminated; never trust it
                s.nets[i].ssid[sizeof(s.nets[i].ssid) - 1] = 0;
                s.nets[i].pass[sizeof(s.nets[i].pass) - 1] = 0;
            }
            for (int i = s.count - 1; i >= 0; i--) {   // no empty names, no repeats
                bool repeat = false;
                for (int j = 0; j < i; j++) repeat |= strcmp(s.nets[j].ssid, s.nets[i].ssid) == 0;
                if (!s.nets[i].ssid[0] || repeat) remove_at(i);
            }
            have = true;
        }
        nvs_close(h);
    }
    if (seed_ssid && seed_ssid[0] && (!have || seed_fresh)) wifi_store_add(seed_ssid, seed_pass ? seed_pass : "");
    wifi_store_flush();
    ESP_LOGI(TAG, "%d saved network(s)", s.count);
}

int wifi_store_count(void)
{
    return s.count;
}

bool wifi_store_get(int i, wifi_net_t *out)
{
    if (i < 0 || i >= s.count) return false;
    *out = s.nets[i];
    return true;
}

bool wifi_store_find(const char *ssid, wifi_net_t *out)
{
    int i = ssid ? find(ssid) : -1;
    if (i < 0) return false;
    if (out) *out = s.nets[i];
    return true;
}

bool wifi_store_add(const char *ssid, const char *pass)
{
    if (!ssid || !pass) return false;
    size_t sl = strlen(ssid), pl = strlen(pass);
    if (sl == 0 || sl >= sizeof(s.nets[0].ssid) || pl >= sizeof(s.nets[0].pass)) return false;
    int i = find(ssid);
    if (i == 0 && strcmp(s.nets[0].pass, pass) == 0) return true;   // nothing changes
    if (i >= 0) remove_at(i);
    if (s.count == WIFI_STORE_MAX) s.count--;   // the oldest drops off
    memmove(&s.nets[1], &s.nets[0], (size_t)s.count * sizeof(wifi_net_t));
    memset(&s.nets[0], 0, sizeof(wifi_net_t));
    memcpy(s.nets[0].ssid, ssid, sl);
    memcpy(s.nets[0].pass, pass, pl);
    s.count++;
    s_dirty = true;
    return true;
}

bool wifi_store_forget(const char *ssid)
{
    int i = ssid ? find(ssid) : -1;
    if (i < 0) return false;
    remove_at(i);
    s_dirty = true;
    return true;
}

int wifi_store_rank(const net_ap_t *aps, int n, int out[WIFI_STORE_MAX])
{
    int rssi[WIFI_STORE_MAX];
    int count = 0;
    for (int i = 0; i < s.count; i++) {
        int best = -1000;
        for (int a = 0; a < n; a++) {
            if (strcmp(aps[a].ssid, s.nets[i].ssid) == 0 && aps[a].rssi > best) best = aps[a].rssi;
        }
        if (best == -1000) continue;
        // Insert by signal, strongest first; equal signal keeps the saved order.
        int at = count;
        while (at > 0 && rssi[at - 1] < best) {
            out[at] = out[at - 1];
            rssi[at] = rssi[at - 1];
            at--;
        }
        out[at] = i;
        rssi[at] = best;
        count++;
    }
    return count;
}
