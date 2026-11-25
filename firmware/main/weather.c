#include "weather.h"

#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "cJSON.h"
#include "display.h"
#include "esp_crt_bundle.h"
#include "esp_heap_caps.h"
#include "esp_http_client.h"
#include "esp_log.h"
#include "freertos/FreeRTOS.h"
#include "freertos/task.h"
#include "hub_model.h"
#include "hubcfg.h"
#include "net.h"
#include "sdkconfig.h"
#include "ui.h"

static const char *TAG = "weather";

#define BODY_MAX (24 * 1024)

// Built once at start from the board's location (hubcfg); a new location restarts the board.
static char s_url[512];

static bool fetch(char *body, hub_weather_t *w)
{
    esp_http_client_config_t cfg = {
        .url = s_url,
        .crt_bundle_attach = esp_crt_bundle_attach,
        .timeout_ms = 10000,
        .buffer_size = 2048,
    };
    esp_http_client_handle_t c = esp_http_client_init(&cfg);
    if (!c) return false;
    bool ok = false;
    if (esp_http_client_open(c, 0) == ESP_OK) {
        esp_http_client_fetch_headers(c);
        int status = esp_http_client_get_status_code(c);
        int total = 0, r;
        while (total < BODY_MAX - 1 && (r = esp_http_client_read(c, body + total, BODY_MAX - 1 - total)) > 0) {
            total += r;
        }
        body[total] = 0;
        if (status == 200 && total > 0) {
            ok = weather_parse(body, w);
        } else {
            ESP_LOGW(TAG, "http %d, %d bytes", status, total);
        }
    }
    esp_http_client_cleanup(c);
    return ok;
}

static void weather_task(void *arg)
{
    char *body = heap_caps_malloc(BODY_MAX, MALLOC_CAP_SPIRAM);
    hub_weather_t *w = heap_caps_calloc(1, sizeof(hub_weather_t), MALLOC_CAP_SPIRAM);
    if (!body || !w) {
        ESP_LOGE(TAG, "out of memory");
        free(body);
        free(w);
        vTaskDelete(NULL);
        return;
    }
    while (true) {
        // Wait for SNTP too, so the "Updated" time is right.
        if (!net_connected() || !net_time_valid()) {
            vTaskDelay(pdMS_TO_TICKS(2000));
            continue;
        }
        bool ok = fetch(body, w);
        if (ok) {
            ESP_LOGI(TAG, "%.1f, code %d", w->temp, w->code);
            display_lock();
            ui_set_weather(w);
            display_unlock();
        }
        vTaskDelay(pdMS_TO_TICKS(ok ? 15 * 60 * 1000 : 60 * 1000));
    }
}

void weather_start(void)
{
    weather_build_url(s_url, sizeof(s_url));
    xTaskCreatePinnedToCore(weather_task, "weather", 8192, NULL, 3, NULL, 0);
}
