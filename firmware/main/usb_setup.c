#include "usb_setup.h"

#include <stdio.h>
#include <string.h>

#include "cJSON.h"
#include "esp_app_desc.h"
#include "esp_log.h"
#include "freertos/FreeRTOS.h"
#include "freertos/task.h"
#include "hubcfg.h"
#include "net.h"
#include "sdkconfig.h"

#if CONFIG_ESP_CONSOLE_USB_SERIAL_JTAG
#include "driver/usb_serial_jtag.h"
#include "driver/usb_serial_jtag_vfs.h"
#elif CONFIG_ESP_CONSOLE_UART
#include "driver/uart.h"
#include "driver/uart_vfs.h"
#endif

static const char *TAG = "usb_setup";

#define PREFIX "@deskhub "
#define CMD_LINE_MAX 384
#define SCAN_WAIT_MS 10000
#define JOIN_WAIT_MS 40000

static bool port_init(void)
{
#if CONFIG_ESP_CONSOLE_USB_SERIAL_JTAG
    usb_serial_jtag_driver_config_t cfg = USB_SERIAL_JTAG_DRIVER_CONFIG_DEFAULT();
    if (usb_serial_jtag_driver_install(&cfg) != ESP_OK) return false;
    usb_serial_jtag_vfs_use_driver();   // logs and replies through the driver too
    return true;
#elif CONFIG_ESP_CONSOLE_UART
    if (uart_driver_install(CONFIG_ESP_CONSOLE_UART_NUM, 1024, 0, 0, NULL, 0) != ESP_OK) return false;
    uart_vfs_dev_use_driver(CONFIG_ESP_CONSOLE_UART_NUM);
    return true;
#else
    return false;
#endif
}

static int port_read(uint8_t *buf, size_t n)
{
#if CONFIG_ESP_CONSOLE_USB_SERIAL_JTAG
    return usb_serial_jtag_read_bytes(buf, n, pdMS_TO_TICKS(1000));
#elif CONFIG_ESP_CONSOLE_UART
    return uart_read_bytes(CONFIG_ESP_CONSOLE_UART_NUM, buf, n, pdMS_TO_TICKS(1000));
#else
    return 0;
#endif
}

// Zero a string item before its tree is freed (token, password).
static void wipe(cJSON *m, const char *key)
{
    cJSON *v = cJSON_GetObjectItemCaseSensitive(m, key);
    if (cJSON_IsString(v) && v->valuestring) memset(v->valuestring, 0, strlen(v->valuestring));
}

// One reply line, written in one call so log lines can't split it.
static void reply(cJSON *m)
{
    char *text = cJSON_PrintUnformatted(m);
    if (text) {
        printf(PREFIX "%s\n", text);
        fflush(stdout);
        memset(text, 0, strlen(text));   // may hold the token
        cJSON_free(text);
    }
    wipe(m, "token");
    cJSON_Delete(m);
}

static cJSON *answer(const char *type)
{
    cJSON *m = cJSON_CreateObject();
    cJSON_AddStringToObject(m, "type", type);
    return m;
}

static void do_info(void)
{
    const hub_cfg_t *c = hubcfg();
    char ssid[33];
    net_ssid(ssid, sizeof(ssid));
    char saved[5][33];
    cJSON *m = answer("info");
    cJSON_AddStringToObject(m, "version", esp_app_get_description()->version);
    cJSON_AddStringToObject(m, "variant", hubcfg_variant());
    cJSON_AddStringToObject(m, "host", c->host);
    cJSON_AddStringToObject(m, "token", c->token);
    cJSON_AddStringToObject(m, "ssid", ssid);
    cJSON_AddStringToObject(m, "ip", net_ip());
    cJSON_AddNumberToObject(m, "saved", net_saved(saved, 5));
    reply(m);
}

static void do_scan(void)
{
    uint32_t gen0 = 0, gen = 0;
    net_ap_t aps[NET_SCAN_MAX];
    net_scan_results(aps, 0, &gen0);
    int waited = 0;
    bool started = false;
    // Busy (joining) at first: keep asking. Then wait for a new result.
    while (waited < SCAN_WAIT_MS) {
        if (!started) started = net_scan_start();
        vTaskDelay(pdMS_TO_TICKS(250));
        waited += 250;
        net_scan_results(aps, 0, &gen);
        if (started && gen != gen0 && !net_scanning()) break;
    }
    int n = net_scan_results(aps, NET_SCAN_MAX, &gen);
    cJSON *m = answer("scan");
    if (gen == gen0) cJSON_AddStringToObject(m, "error", "the scan did not finish");
    cJSON *list = cJSON_AddArrayToObject(m, "networks");
    for (int i = 0; i < n; i++) {
        cJSON *a = cJSON_CreateObject();
        cJSON_AddStringToObject(a, "ssid", aps[i].ssid);
        cJSON_AddNumberToObject(a, "rssi", aps[i].rssi);
        cJSON_AddBoolToObject(a, "secure", aps[i].secure);
        cJSON_AddItemToArray(list, a);
    }
    reply(m);
}

static void do_wifi(const cJSON *req)
{
    const cJSON *s = cJSON_GetObjectItemCaseSensitive(req, "ssid");
    const cJSON *p = cJSON_GetObjectItemCaseSensitive(req, "pass");
    const char *ssid = cJSON_IsString(s) ? s->valuestring : "";
    const char *pass = cJSON_IsString(p) ? p->valuestring : "";
    size_t sl = strlen(ssid), pl = strlen(pass);
    cJSON *m = answer("wifi");
    if (sl == 0 || sl > 32 || (pl != 0 && (pl < 8 || pl > 64))) {
        cJSON_AddBoolToObject(m, "ok", false);
        cJSON_AddStringToObject(m, "error", "Wi-Fi names have 1 to 32 characters, passwords 8 to 64 (or none)");
        reply(m);
        return;
    }
    ESP_LOGI(TAG, "joining %s (from USB)", ssid);
    net_join(ssid, pass);
    net_join_t st = NET_JOIN_BUSY;
    for (int waited = 0; waited < JOIN_WAIT_MS; waited += 250) {
        vTaskDelay(pdMS_TO_TICKS(250));
        st = net_join_state();
        if (st == NET_JOIN_OK || st == NET_JOIN_FAIL) break;
    }
    cJSON_AddBoolToObject(m, "ok", st == NET_JOIN_OK);
    if (st == NET_JOIN_OK) {
        // The address can lag the join by a moment.
        for (int i = 0; i < 20 && !net_ip()[0]; i++) vTaskDelay(pdMS_TO_TICKS(100));
        cJSON_AddStringToObject(m, "ip", net_ip());
        cJSON_AddStringToObject(m, "host", hubcfg()->host);
    } else {
        cJSON_AddStringToObject(m, "error", st == NET_JOIN_FAIL ? "couldn't join: check the password and that the network is in range"
                                                                 : "joining took too long");
    }
    reply(m);
}

static void handle(const char *json)
{
    cJSON *req = cJSON_Parse(json);
    const cJSON *t = req ? cJSON_GetObjectItemCaseSensitive(req, "type") : NULL;
    const char *type = cJSON_IsString(t) ? t->valuestring : "";
    if (strcmp(type, "info") == 0) {
        do_info();
    } else if (strcmp(type, "scan") == 0) {
        do_scan();
    } else if (strcmp(type, "wifi") == 0) {
        do_wifi(req);
    } else {
        cJSON *m = answer("error");
        cJSON_AddStringToObject(m, "error", req ? "unknown command" : "not JSON");
        reply(m);
    }
    if (req) wipe(req, "pass");
    cJSON_Delete(req);   // holds the password for "wifi"
}

static void usb_task(void *arg)
{
    static char line[CMD_LINE_MAX];
    size_t len = 0;
    bool overflow = false;
    uint8_t buf[64];
    for (;;) {
        int n = port_read(buf, sizeof(buf));
        if (n < 0) {   // driver error: don't spin
            vTaskDelay(pdMS_TO_TICKS(100));
            continue;
        }
        for (int i = 0; i < n; i++) {
            char c = (char)buf[i];
            if (c == '\r') continue;
            if (c != '\n') {
                if (len + 1 < sizeof(line)) line[len++] = c;
                else overflow = true;
                continue;
            }
            line[len] = 0;
            if (!overflow && strncmp(line, PREFIX, strlen(PREFIX)) == 0) handle(line + strlen(PREFIX));
            memset(line, 0, len);
            len = 0;
            overflow = false;
        }
    }
}

void usb_setup_start(void)
{
    if (!port_init()) {
        ESP_LOGW(TAG, "console port not available: no setup over USB");
        return;
    }
    if (xTaskCreate(usb_task, "usb_setup", 6144, NULL, 3, NULL) != pdPASS) {
        ESP_LOGW(TAG, "no memory for the setup task: no setup over USB");
        return;
    }
    ESP_LOGI(TAG, "listening for setup commands on the USB port");
}
