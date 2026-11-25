#include "board.h"
#include "display.h"
#include "esp_heap_caps.h"
#include "esp_log.h"
#include "hub_server.h"
#include "hubcfg.h"
#include "net.h"
#include "nvs_flash.h"
#include "ota.h"
#include "ui.h"
#include "usb_setup.h"
#include "weather.h"

void app_main(void)
{
    esp_err_t err = nvs_flash_init();
    if (err == ESP_ERR_NVS_NO_FREE_PAGES || err == ESP_ERR_NVS_NEW_VERSION_FOUND) {
        ESP_ERROR_CHECK(nvs_flash_erase());
        err = nvs_flash_init();
    }
    ESP_ERROR_CHECK(err);
    hubcfg_load();   // Wi-Fi networks, token and location: NVS, seeded from a local build

    ESP_ERROR_CHECK(board_init());
    // Wi-Fi first, so its internal-RAM buffers are allocated before the UI grows.
    net_start();
    ESP_ERROR_CHECK(display_start());

    display_lock();
    ui_init();
    if (net_needs_setup()) ui_wifi_show();   // no saved network: set one up on screen
    display_unlock();

    hub_server_start();
    usb_setup_start();   // Wi-Fi and token from the desktop app over the cable
    ota_mark_valid();   // got this far: keep this firmware (no rollback)
    weather_start();
    ESP_LOGI("main", "free internal %u, psram %u",
             (unsigned)heap_caps_get_free_size(MALLOC_CAP_INTERNAL),
             (unsigned)heap_caps_get_free_size(MALLOC_CAP_SPIRAM));
}
