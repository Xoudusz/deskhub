#include "net.h"

#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>

#include "esp_event.h"
#include "esp_log.h"
#include "esp_netif.h"
#include "esp_netif_sntp.h"
#include "esp_timer.h"
#include "esp_wifi.h"
#include "hubcfg.h"
#include "freertos/FreeRTOS.h"
#include "freertos/semphr.h"
#include "freertos/task.h"
#include "mdns.h"
#include "sdkconfig.h"
#include "wifi_store.h"

static const char *TAG = "net";

#define JOIN_TRIES 3                // association attempts for a network picked on the Wi-Fi screen
#define JOIN_TIMEOUT_US (30 * 1000000LL)
#define SCAN_TIMEOUT_US (15 * 1000000LL)
#define BACKOFF_MIN_MS 2000
#define BACKOFF_MAX_MS 60000

// Requests from the UI and the timers. They run on the default event loop like the Wi-Fi
// events, so every esp_wifi_* call and every change to the state below happens on that one
// task. s_lock only guards the state against the UI's reads: it is never held while waiting
// for anything else.
ESP_EVENT_DEFINE_BASE(NET_CMD_EVENT);
enum { CMD_RETRY, CMD_TICK, CMD_SCAN, CMD_JOIN, CMD_FORGET };

typedef struct {
    char ssid[33];
    char pass[65];
} join_req_t;

static SemaphoreHandle_t s_lock;
static volatile bool s_connected;
static char s_ip[16] = "";
static char s_target[33];   // the network being joined, or the one the board is on
static net_ap_t s_aps[NET_SCAN_MAX];
static int s_nap;
static uint32_t s_gen;
static bool s_scanning;
static int64_t s_scan_started;
static bool s_auto;   // join a saved network when the scan ends
static int s_cand[WIFI_STORE_MAX], s_ncand, s_icand;
static bool s_user;   // a join from the Wi-Fi screen is running
static char s_user_pass[65];
static char s_pend_ssid[33];   // that join, waiting for the old network to let go
static bool s_pending;
static int s_tries;
static int64_t s_join_started;
static net_join_t s_join = NET_JOIN_IDLE;
static esp_timer_handle_t s_retry, s_tick;
static int s_backoff_ms = BACKOFF_MIN_MS;

#define LOCK() xSemaphoreTake(s_lock, portMAX_DELAY)
#define UNLOCK() xSemaphoreGive(s_lock)

// ---- on the event loop ------------------------------------------------------------

static void connect_to(const char *ssid, const char *pass)
{
    wifi_config_t cfg = {0};
    // Not strlcpy: a 32-byte name or a 64-digit key fills the field without a terminator.
    memcpy(cfg.sta.ssid, ssid, strnlen(ssid, sizeof(cfg.sta.ssid)));
    memcpy(cfg.sta.password, pass, strnlen(pass, sizeof(cfg.sta.password)));
    cfg.sta.threshold.authmode = WIFI_AUTH_OPEN;
    cfg.sta.pmf_cfg.capable = true;
    cfg.sta.sae_pwe_h2e = WPA3_SAE_PWE_BOTH;
    LOCK();
    strlcpy(s_target, ssid, sizeof(s_target));
    UNLOCK();
    esp_err_t err = esp_wifi_set_config(WIFI_IF_STA, &cfg);
    if (err == ESP_OK) err = esp_wifi_connect();
    if (err != ESP_OK) ESP_LOGW(TAG, "joining %s did not start: %s", ssid, esp_err_to_name(err));
}

static void set_target(const char *ssid)
{
    LOCK();
    strlcpy(s_target, ssid, sizeof(s_target));
    UNLOCK();
}

static void retry_later(int ms)
{
    esp_timer_stop(s_retry);
    esp_timer_start_once(s_retry, (uint64_t)ms * 1000);
}

static void retry_backoff(void)
{
    retry_later(s_backoff_ms);
    s_backoff_ms = s_backoff_ms * 2 > BACKOFF_MAX_MS ? BACKOFF_MAX_MS : s_backoff_ms * 2;
}

// Tries the next network in s_cand; after the last one, starts over later.
static void next_candidate(void)
{
    while (s_icand < s_ncand) {
        wifi_net_t n;
        LOCK();
        bool ok = wifi_store_get(s_cand[s_icand++], &n);
        UNLOCK();
        if (ok) {
            ESP_LOGI(TAG, "joining %s", n.ssid);
            connect_to(n.ssid, n.pass);
            memset(&n, 0, sizeof(n));
            return;
        }
    }
    set_target("");
    retry_backoff();
}

// Every saved network in saved order: when there is no scan to go by (a hidden network,
// or the scan failed), each one gets a try.
static void try_all_saved(void)
{
    LOCK();
    s_ncand = wifi_store_count();
    UNLOCK();
    for (int i = 0; i < s_ncand; i++) s_cand[i] = i;
    s_icand = 0;
    next_candidate();
}

static bool scan_begin(void)
{
    if (s_scanning) return true;
    if (esp_wifi_scan_start(NULL, false) != ESP_OK) return false;
    LOCK();
    s_scanning = true;
    s_scan_started = esp_timer_get_time();
    UNLOCK();
    return true;
}

// Scan, then join the strongest saved network in range.
static void auto_join(void)
{
    LOCK();
    int saved = wifi_store_count();
    UNLOCK();
    if (s_user || s_connected || saved == 0) return;
    if (saved == 1) {   // nothing to choose: no scan, no delay
        try_all_saved();
        return;
    }
    s_auto = true;
    if (!scan_begin()) {
        s_auto = false;
        try_all_saved();
    }
}

static void scan_done(const wifi_event_sta_scan_done_t *ev)
{
    static wifi_ap_record_t recs[NET_SCAN_MAX];
    uint16_t n = NET_SCAN_MAX;
    bool got = ev->status == 0 && esp_wifi_scan_get_ap_records(&n, recs) == ESP_OK;
    LOCK();
    s_scanning = false;
    if (got) {   // a stopped or failed scan keeps the last list
        s_nap = 0;
        for (int i = 0; i < n; i++) {
            const char *ssid = (const char *)recs[i].ssid;
            if (!ssid[0]) continue;   // hidden
            int j = 0;
            while (j < s_nap && strcmp(s_aps[j].ssid, ssid) != 0) j++;
            if (j < s_nap) {   // the same network from another access point
                if (recs[i].rssi > s_aps[j].rssi) s_aps[j].rssi = recs[i].rssi;
                continue;
            }
            net_ap_t *a = &s_aps[s_nap++];
            strlcpy(a->ssid, ssid, sizeof(a->ssid));
            a->rssi = recs[i].rssi;
            a->secure = recs[i].authmode != WIFI_AUTH_OPEN && recs[i].authmode != WIFI_AUTH_OWE;
        }
        for (int i = 1; i < s_nap; i++) {   // strongest first
            net_ap_t a = s_aps[i];
            int j = i;
            while (j > 0 && s_aps[j - 1].rssi < a.rssi) {
                s_aps[j] = s_aps[j - 1];
                j--;
            }
            s_aps[j] = a;
        }
        s_gen++;
        s_ncand = wifi_store_rank(s_aps, s_nap, s_cand);
    }
    UNLOCK();
    if (!s_auto) return;
    s_auto = false;
    if (s_user || s_connected) return;
    s_icand = 0;
    if (!got || s_ncand == 0) {
        ESP_LOGW(TAG, "no saved network in range");
        try_all_saved();
    } else {
        next_candidate();
    }
}

static void join_failed(void)
{
    s_pending = false;
    LOCK();
    s_user = false;
    s_join = NET_JOIN_FAIL;
    memset(s_user_pass, 0, sizeof(s_user_pass));
    s_target[0] = 0;
    UNLOCK();
    s_backoff_ms = BACKOFF_MIN_MS;
    retry_later(1000);   // back to a saved network
}

static void on_disconnected(const wifi_event_sta_disconnected_t *ev)
{
    char ssid[33] = {0};
    memcpy(ssid, ev->ssid, ev->ssid_len < 32 ? ev->ssid_len : 32);
    bool was = s_connected;
    LOCK();
    s_connected = false;
    s_ip[0] = 0;
    bool ours = s_target[0] && strcmp(ssid, s_target) == 0;   // not a network it already left
    UNLOCK();
    if (s_user && s_pending) {   // the old network let go: now join the new one
        s_pending = false;
        connect_to(s_pend_ssid, s_user_pass);
        return;
    }
    if (s_user) {
        if (!ours) return;
        if (++s_tries < JOIN_TRIES) {
            esp_wifi_connect();
        } else {
            ESP_LOGW(TAG, "joining %s failed (reason %d)", ssid, ev->reason);
            join_failed();
        }
    } else if (was) {
        ESP_LOGW(TAG, "wifi lost (reason %d), reconnecting", ev->reason);
        set_target("");
        s_backoff_ms = BACKOFF_MIN_MS;
        retry_later(1000);
    } else if (ours) {
        ESP_LOGW(TAG, "joining %s failed (reason %d)", ssid, ev->reason);
        next_candidate();
    }
}

static void on_got_ip(const ip_event_got_ip_t *ev)
{
    wifi_net_t n;
    LOCK();
    snprintf(s_ip, sizeof(s_ip), IPSTR, IP2STR(&ev->ip_info.ip));
    s_connected = true;
    ESP_LOGI(TAG, "connected to %s, ip %s", s_target, s_ip);
    if (s_user) {
        wifi_store_add(s_target, s_user_pass);
        memset(s_user_pass, 0, sizeof(s_user_pass));
        s_user = false;
        s_join = NET_JOIN_OK;
    } else if (wifi_store_find(s_target, &n)) {
        wifi_store_add(n.ssid, n.pass);   // most recently joined first
        memset(&n, 0, sizeof(n));
    }
    UNLOCK();
    wifi_store_flush();   // only this task changes the store, so no lock needed to write it
    s_backoff_ms = BACKOFF_MIN_MS;
    esp_timer_stop(s_retry);
    // Ask for the time now that there is a network. Starting SNTP before
    // Wi-Fi is up leaves it in a long retry backoff after the first failure.
    if (esp_netif_sntp_start() != ESP_OK) {
        ESP_LOGW(TAG, "sntp start failed");
    }
}

static void on_join(const join_req_t *req)
{
    esp_timer_stop(s_retry);
    s_auto = false;
    s_tries = 0;
    LOCK();
    s_user = true;
    s_join = NET_JOIN_BUSY;
    s_join_started = esp_timer_get_time();
    strlcpy(s_user_pass, req->pass, sizeof(s_user_pass));
    bool scanning = s_scanning;
    bool leave = s_connected || s_target[0];
    s_connected = false;   // from now on the UI shows the network being joined as not connected
    s_target[0] = 0;
    UNLOCK();
    if (scanning) esp_wifi_scan_stop();   // its SCAN_DONE has a failure status: ignored
    ESP_LOGI(TAG, "joining %s (from the Wi-Fi screen)", req->ssid);
    strlcpy(s_pend_ssid, req->ssid, sizeof(s_pend_ssid));
    // Setting a new network while still on the old one can be refused: leave first, and
    // join from its DISCONNECTED event (or from the tick, if that never comes).
    s_pending = leave && esp_wifi_disconnect() == ESP_OK;
    if (!s_pending) connect_to(req->ssid, req->pass);
}

static void on_forget(const join_req_t *req)
{
    LOCK();
    wifi_store_forget(req->ssid);
    bool current = strcmp(req->ssid, s_target) == 0;   // on it, or joining it
    UNLOCK();
    wifi_store_flush();
    // Leaving it: the disconnect event joins another saved network.
    if (current) esp_wifi_disconnect();
}

// Once a second: a join or a scan that never ends.
static void on_tick(void)
{
    int64_t now = esp_timer_get_time();
    if (s_user && s_pending && now - s_join_started > 3 * 1000000LL) {   // no DISCONNECTED came
        s_pending = false;
        connect_to(s_pend_ssid, s_user_pass);
    }
    if (s_user && now - s_join_started > JOIN_TIMEOUT_US) {
        ESP_LOGW(TAG, "joining %s timed out", s_target);
        esp_wifi_disconnect();
        join_failed();
    }
    if (s_scanning && now - s_scan_started > SCAN_TIMEOUT_US) {
        ESP_LOGW(TAG, "scan did not finish");
        esp_wifi_scan_stop();
        LOCK();
        s_scanning = false;
        UNLOCK();
        if (s_auto) {
            s_auto = false;
            if (!s_user && !s_connected) try_all_saved();
        }
    }
}

static void on_event(void *arg, esp_event_base_t base, int32_t id, void *data)
{
    if (base == WIFI_EVENT) {
        if (id == WIFI_EVENT_STA_START) auto_join();
        else if (id == WIFI_EVENT_SCAN_DONE) scan_done(data);
        else if (id == WIFI_EVENT_STA_DISCONNECTED) on_disconnected(data);
    } else if (base == IP_EVENT && id == IP_EVENT_STA_GOT_IP) {
        on_got_ip(data);
    } else if (base == NET_CMD_EVENT) {
        switch (id) {
        case CMD_RETRY: auto_join(); break;
        case CMD_TICK: on_tick(); break;
        case CMD_SCAN:
            if (!s_user) scan_begin();
            break;
        case CMD_JOIN: on_join(data); break;
        case CMD_FORGET: on_forget(data); break;
        }
    }
}

// ---- timers and UI: they only post ----------------------------------------------------

static void retry_cb(void *arg)
{
    if (esp_event_post(NET_CMD_EVENT, CMD_RETRY, NULL, 0, 0) != ESP_OK) retry_later(1000);
}

static void tick_cb(void *arg)
{
    esp_event_post(NET_CMD_EVENT, CMD_TICK, NULL, 0, 0);   // a full queue: the next tick
}

static bool post(int32_t cmd, const char *ssid, const char *pass)
{
    join_req_t req = {0};
    if (ssid) strlcpy(req.ssid, ssid, sizeof(req.ssid));
    if (pass) strlcpy(req.pass, pass, sizeof(req.pass));
    bool ok = esp_event_post(NET_CMD_EVENT, cmd, &req, sizeof(req), pdMS_TO_TICKS(100)) == ESP_OK;
    memset(&req, 0, sizeof(req));
    return ok;
}

static void mdns_start(void)
{
    if (mdns_init() != ESP_OK) {
        ESP_LOGW(TAG, "mdns init failed");
        return;
    }
    mdns_hostname_set(hubcfg()->host);
    mdns_instance_name_set("Desk Hub");
    mdns_service_add(NULL, "_deskhub", "_tcp", 80, NULL, 0);
}

void net_start(void)
{
    const hub_cfg_t *hc = hubcfg();
    setenv("TZ", hc->tz, 1);
    tzset();

    s_lock = xSemaphoreCreateMutex();
    const esp_timer_create_args_t retry = {.callback = retry_cb, .name = "wifi_retry"};
    ESP_ERROR_CHECK(esp_timer_create(&retry, &s_retry));
    const esp_timer_create_args_t tick = {.callback = tick_cb, .name = "wifi_tick"};
    ESP_ERROR_CHECK(esp_timer_create(&tick, &s_tick));

    ESP_ERROR_CHECK(esp_netif_init());
    ESP_ERROR_CHECK(esp_event_loop_create_default());
    esp_netif_t *netif = esp_netif_create_default_wifi_sta();
    esp_netif_set_hostname(netif, hc->host);

    wifi_init_config_t init = WIFI_INIT_CONFIG_DEFAULT();
    ESP_ERROR_CHECK(esp_wifi_init(&init));
    ESP_ERROR_CHECK(esp_event_handler_register(WIFI_EVENT, ESP_EVENT_ANY_ID, on_event, NULL));
    ESP_ERROR_CHECK(esp_event_handler_register(IP_EVENT, IP_EVENT_STA_GOT_IP, on_event, NULL));
    ESP_ERROR_CHECK(esp_event_handler_register(NET_CMD_EVENT, ESP_EVENT_ANY_ID, on_event, NULL));
    if (wifi_store_count() == 0) ESP_LOGW(TAG, "no saved Wi-Fi network: set one up on the Wi-Fi screen");

    // Set up SNTP now but start it from the GOT_IP handler, once the network exists.
    esp_sntp_config_t sntp = ESP_NETIF_SNTP_DEFAULT_CONFIG_MULTIPLE(2, ESP_SNTP_SERVER_LIST("time.google.com", "pool.ntp.org"));
    sntp.start = false;
    esp_netif_sntp_init(&sntp);

    ESP_ERROR_CHECK(esp_wifi_set_mode(WIFI_MODE_STA));
    ESP_ERROR_CHECK(esp_wifi_start());   // STA_START: join a saved network
    // Power save adds latency to every web socket frame; the hub is mains powered.
    esp_wifi_set_ps(WIFI_PS_NONE);
    esp_timer_start_periodic(s_tick, 1000 * 1000);

    mdns_start();
}

bool net_needs_setup(void)
{
    LOCK();
    bool none = wifi_store_count() == 0;
    UNLOCK();
    return none;
}

bool net_connected(void)
{
    return s_connected;
}

bool net_time_valid(void)
{
    return time(NULL) > 1700000000;
}

const char *net_ip(void)
{
    return s_ip;
}

void net_ssid(char *out, size_t n)
{
    LOCK();
    strlcpy(out, s_connected ? s_target : "", n);
    UNLOCK();
}

bool net_scan_start(void)
{
    LOCK();
    bool busy = s_user;
    UNLOCK();
    return !busy && post(CMD_SCAN, NULL, NULL);
}

bool net_scanning(void)
{
    LOCK();
    bool scanning = s_scanning;
    UNLOCK();
    return scanning;
}

int net_scan_results(net_ap_t *out, int max, uint32_t *gen)
{
    LOCK();
    int n = s_nap < max ? s_nap : max;
    memcpy(out, s_aps, (size_t)n * sizeof(net_ap_t));
    if (gen) *gen = s_gen;
    UNLOCK();
    return n;
}

void net_join(const char *ssid, const char *pass)
{
    LOCK();
    s_join = NET_JOIN_BUSY;   // at once, so the caller's next poll doesn't see the last result
    UNLOCK();
    if (!post(CMD_JOIN, ssid, pass)) {
        LOCK();
        s_join = NET_JOIN_FAIL;
        UNLOCK();
    }
}

net_join_t net_join_state(void)
{
    LOCK();
    net_join_t st = s_join;
    UNLOCK();
    return st;
}

bool net_join_saved(const char *ssid)
{
    wifi_net_t n;
    LOCK();
    bool found = wifi_store_find(ssid, &n);
    UNLOCK();
    if (found) net_join(n.ssid, n.pass);
    memset(&n, 0, sizeof(n));
    return found;
}

int net_saved(char (*out)[33], int max)
{
    LOCK();
    int n = 0;
    wifi_net_t w;
    while (n < max && wifi_store_get(n, &w)) {
        strlcpy(out[n], w.ssid, sizeof(out[n]));
        n++;
    }
    memset(&w, 0, sizeof(w));
    UNLOCK();
    return n;
}

void net_forget(const char *ssid)
{
    post(CMD_FORGET, ssid, NULL);
}
