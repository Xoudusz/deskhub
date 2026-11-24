#include "net.h"

#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>

#include "esp_event.h"
#include "esp_log.h"
#include "esp_netif.h"
#include "esp_netif_sntp.h"
#include "esp_wifi.h"
#include "hubcfg.h"
#include "freertos/FreeRTOS.h"
#include "freertos/task.h"
#include "mdns.h"
#include "sdkconfig.h"

static const char *TAG = "net";

static volatile bool s_connected;
static bool s_needs_setup;
static char s_ip[16] = "";

static void on_event(void *arg, esp_event_base_t base, int32_t id, void *data)
{
    if (s_needs_setup) return;
    if (base == WIFI_EVENT && id == WIFI_EVENT_STA_START) {
        esp_wifi_connect();
    } else if (base == WIFI_EVENT && id == WIFI_EVENT_STA_DISCONNECTED) {
        s_connected = false;
        s_ip[0] = 0;
        ESP_LOGW(TAG, "wifi disconnected, retrying");
        esp_wifi_connect();
    } else if (base == IP_EVENT && id == IP_EVENT_STA_GOT_IP) {
        ip_event_got_ip_t *ev = data;
        snprintf(s_ip, sizeof(s_ip), IPSTR, IP2STR(&ev->ip_info.ip));
        s_connected = true;
        ESP_LOGI(TAG, "connected, ip %s", s_ip);
        // Ask for the time now that there is a network. Starting SNTP before
        // Wi-Fi is up leaves it in a long retry backoff after the first failure.
        if (esp_netif_sntp_start() != ESP_OK) {
            ESP_LOGW(TAG, "sntp start failed");
        }
    }
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

    ESP_ERROR_CHECK(esp_netif_init());
    ESP_ERROR_CHECK(esp_event_loop_create_default());
    esp_netif_t *netif = esp_netif_create_default_wifi_sta();
    esp_netif_set_hostname(netif, hc->host);

    wifi_init_config_t init = WIFI_INIT_CONFIG_DEFAULT();
    ESP_ERROR_CHECK(esp_wifi_init(&init));
    ESP_ERROR_CHECK(esp_event_handler_register(WIFI_EVENT, ESP_EVENT_ANY_ID, on_event, NULL));
    ESP_ERROR_CHECK(esp_event_handler_register(IP_EVENT, IP_EVENT_STA_GOT_IP, on_event, NULL));

    wifi_config_t cfg = {0};
    strlcpy((char *)cfg.sta.ssid, hc->ssid, sizeof(cfg.sta.ssid));
    strlcpy((char *)cfg.sta.password, hc->pass, sizeof(cfg.sta.password));
    // A release image on a board that never ran a local build: no network to join.
    s_needs_setup = hc->ssid[0] == 0;
    if (s_needs_setup) ESP_LOGE(TAG, "no Wi-Fi settings: flash a local build once (see README)");
    cfg.sta.threshold.authmode = WIFI_AUTH_OPEN;
    cfg.sta.pmf_cfg.capable = true;
    cfg.sta.sae_pwe_h2e = WPA3_SAE_PWE_BOTH;

    // Set up SNTP now but start it from the GOT_IP handler, once the network exists.
    esp_sntp_config_t sntp = ESP_NETIF_SNTP_DEFAULT_CONFIG_MULTIPLE(2, ESP_SNTP_SERVER_LIST("time.google.com", "pool.ntp.org"));
    sntp.start = false;
    esp_netif_sntp_init(&sntp);

    ESP_ERROR_CHECK(esp_wifi_set_mode(WIFI_MODE_STA));
    ESP_ERROR_CHECK(esp_wifi_set_config(WIFI_IF_STA, &cfg));
    ESP_ERROR_CHECK(esp_wifi_start());
    // Power save adds latency to every web socket frame; the hub is mains powered.
    esp_wifi_set_ps(WIFI_PS_NONE);

    mdns_start();
}

bool net_needs_setup(void)
{
    return s_needs_setup;
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
