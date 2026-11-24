// Firmware updates over Wi-Fi: POST /firmware?token=... with the app image as the body.
//
// The board first asks on screen (Install / Deny, 60 s). Only then is the image
// streamed into the spare OTA slot, checked, and made the boot image. The
// bootloader rolls back to the previous image if the new one never starts
// properly (see ota_mark_valid).
#include "hubcfg.h"
#include "ota.h"

#include <stdlib.h>
#include <string.h>

#include "display.h"
#include "esp_log.h"
#include "esp_ota_ops.h"
#include "esp_system.h"
#include "freertos/FreeRTOS.h"
#include "freertos/task.h"
#include "sdkconfig.h"
#include "ui.h"

static const char *TAG = "ota";

#define ANSWER_TIMEOUT_MS 60000
#define CHUNK 4096

enum { ANSWER_NONE, ANSWER_ALLOW, ANSWER_DENY };
static volatile int s_answer;
static volatile bool s_busy;

void ota_answer(bool allow)
{
    s_answer = allow ? ANSWER_ALLOW : ANSWER_DENY;
}

void ota_mark_valid(void)
{
    const esp_partition_t *running = esp_ota_get_running_partition();
    esp_ota_img_states_t state;
    if (esp_ota_get_state_partition(running, &state) == ESP_OK && state == ESP_OTA_IMG_PENDING_VERIFY) {
        ESP_LOGI(TAG, "new firmware started fine; keeping it");
        esp_ota_mark_app_valid_cancel_rollback();
    }
}

static void ui_call(void (*fn)(void))
{
    display_lock();
    fn();
    display_unlock();
}

static void show_prompt(void) { display_wake(); ui_update_show(); }

static void finish(httpd_req_t *req, int status, const char *msg)
{
    if (status == 200) {
        httpd_resp_sendstr(req, msg);
    } else {
        httpd_resp_set_status(req, status == 403 ? "403 Forbidden" : status == 408 ? "408 Request Timeout"
                                   : status == 400 ? "400 Bad Request" : "500 Internal Server Error");
        httpd_resp_sendstr(req, msg);
    }
    httpd_req_async_handler_complete(req);
}

static void fail_ui(const char *text)
{
    display_lock();
    ui_update_message(text);
    display_unlock();
    vTaskDelay(pdMS_TO_TICKS(3000));
    ui_call(ui_update_hide);
}

static void ota_task(void *arg)
{
    httpd_req_t *req = arg;
    s_answer = ANSWER_NONE;
    ui_call(show_prompt);
    int waited = 0;
    while (s_answer == ANSWER_NONE && waited < ANSWER_TIMEOUT_MS) {
        vTaskDelay(pdMS_TO_TICKS(100));
        waited += 100;
    }
    if (s_answer != ANSWER_ALLOW) {
        bool denied = s_answer == ANSWER_DENY;
        ESP_LOGW(TAG, "update %s", denied ? "denied on the board" : "not answered");
        ui_call(ui_update_hide);
        finish(req, denied ? 403 : 408, denied ? "denied on the board" : "nobody answered on the board");
        goto done;
    }

    const esp_partition_t *part = esp_ota_get_next_update_partition(NULL);
    esp_ota_handle_t h = 0;
    char *buf = malloc(CHUNK);
    if (!part || !buf || esp_ota_begin(part, OTA_WITH_SEQUENTIAL_WRITES, &h) != ESP_OK) {
        free(buf);
        fail_ui("Update failed: no space for it");
        finish(req, 500, "can't start the update");
        goto done;
    }
    ESP_LOGI(TAG, "writing %d bytes to %s", req->content_len, part->label);
    size_t left = req->content_len, total = req->content_len;
    int last_pct = -1, timeouts = 0;
    esp_err_t err = ESP_OK;
    while (left > 0) {
        int n = httpd_req_recv(req, buf, left < CHUNK ? left : CHUNK);
        if (n == HTTPD_SOCK_ERR_TIMEOUT && ++timeouts < 5) continue;
        if (n <= 0) { err = ESP_FAIL; break; }
        timeouts = 0;
        if ((err = esp_ota_write(h, buf, n)) != ESP_OK) break;
        left -= n;
        int pct = (int)((total - left) * 100 / total);
        if (pct / 5 != last_pct / 5) {
            last_pct = pct;
            display_lock();
            ui_update_progress(pct);
            display_unlock();
        }
    }
    free(buf);
    if (err == ESP_OK) err = esp_ota_end(h);   // checks the image
    else esp_ota_abort(h);
    if (err == ESP_OK) err = esp_ota_set_boot_partition(part);
    if (err != ESP_OK) {
        ESP_LOGE(TAG, "update failed: %s", esp_err_to_name(err));
        fail_ui(err == ESP_ERR_OTA_VALIDATE_FAILED ? "Update failed: not a valid image" : "Update failed");
        finish(req, err == ESP_ERR_OTA_VALIDATE_FAILED ? 400 : 500, esp_err_to_name(err));
        goto done;
    }
    display_lock();
    ui_update_message("Restarting with the new firmware");
    display_unlock();
    finish(req, 200, "installed; restarting");
    vTaskDelay(pdMS_TO_TICKS(1500));
    esp_restart();

done:
    s_busy = false;
    vTaskDelete(NULL);
}

static esp_err_t firmware_handler(httpd_req_t *req)
{
    char query[128], token[64];
    if (httpd_req_get_url_query_str(req, query, sizeof(query)) != ESP_OK ||
        httpd_query_key_value(query, "token", token, sizeof(token)) != ESP_OK ||
        !hubcfg_token_ok(token)) {
        return httpd_resp_send_err(req, HTTPD_403_FORBIDDEN, "bad token");
    }
    const esp_partition_t *part = esp_ota_get_next_update_partition(NULL);
    if (!part) return httpd_resp_send_err(req, HTTPD_500_INTERNAL_SERVER_ERROR, "no OTA slot");
    if (req->content_len < 64 * 1024 || req->content_len > part->size) {
        return httpd_resp_send_err(req, HTTPD_400_BAD_REQUEST, "image size looks wrong");
    }
    if (s_busy) return httpd_resp_send_err(req, HTTPD_400_BAD_REQUEST, "an update is already running");
    // The prompt can wait a minute: hand the request to a task of its own so the
    // server keeps answering everything else (the companion's socket included).
    httpd_req_t *copy = NULL;
    if (httpd_req_async_handler_begin(req, &copy) != ESP_OK) {
        return httpd_resp_send_err(req, HTTPD_500_INTERNAL_SERVER_ERROR, "busy");
    }
    s_busy = true;
    if (xTaskCreate(ota_task, "ota", 8192, copy, 5, NULL) != pdPASS) {
        s_busy = false;
        httpd_resp_send_err(copy, HTTPD_500_INTERNAL_SERVER_ERROR, "no memory");
        httpd_req_async_handler_complete(copy);
    }
    return ESP_OK;
}

void ota_register(httpd_handle_t server)
{
    static const httpd_uri_t fw = {.uri = "/firmware", .method = HTTP_POST, .handler = firmware_handler};
    httpd_register_uri_handler(server, &fw);
}
